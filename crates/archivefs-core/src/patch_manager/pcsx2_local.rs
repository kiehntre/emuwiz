//! Bounded, read-only discovery and inspection of local PCSX2 profiles.
//!
//! This module has no write, process-execution, or network capability. It
//! validates configuration and patch roots without following symlinks, opens
//! PNACH files read-only (with `O_NOFOLLOW` on Unix), and applies fixed limits
//! before retaining any parsed metadata.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::emulator_environment::EncodedPath;

use super::destination_safety::{
    DestinationRootState, DestinationSafetyFailureReason, validate_destination_root,
};
use super::pcsx2::{normalize_crc, parse_patch_identity};

pub const PCSX2_MAX_PROFILES: usize = 16;
pub const PCSX2_MAX_PATCH_DIRECTORIES_PER_PROFILE: usize = 4;
pub const PCSX2_MAX_DIRECTORIES_TRAVERSED: usize = 256;
pub const PCSX2_MAX_ENTRIES_VISITED: usize = 10_000;
pub const PCSX2_MAX_DIRECTORY_DEPTH: usize = 4;
pub const PCSX2_MAX_PNACH_FILES: usize = 2_048;
pub const PCSX2_MAX_PNACH_FILE_BYTES: u64 = 256 * 1024;
pub const PCSX2_MAX_TOTAL_PNACH_BYTES: u64 = 16 * 1024 * 1024;
pub const PCSX2_MAX_LINES_PER_FILE: usize = 8_192;
pub const PCSX2_MAX_LINE_BYTES: usize = 8 * 1024;

const FLATPAK_APP_ID: &str = "net.pcsx2.PCSX2";
const PCSX2_MAX_RETAINED_COMMENTS_PER_FILE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2InstallationType {
    Native,
    /// A documented but non-primary native data location observed in the
    /// wild for Linux AppImage builds (for example `~/.local/share/PCSX2`
    /// or `~/Documents/PCSX2`), reported alongside the primary XDG
    /// configuration directory rather than in place of it.
    NativeAlternate,
    FlatpakUser,
    FlatpakSystem,
    Portable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2ProfileScope {
    User,
    SystemInstallationUserProfile,
    Portable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2ProfileBlockerKind {
    PathNotAbsolute,
    FilesystemRoot,
    MissingConfiguration,
    UnsafePath,
    NotDirectory,
    Unreadable,
    MissingPcsx2Evidence,
    ProfileLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pcsx2ProfileBlocker {
    pub kind: Pcsx2ProfileBlockerKind,
    pub path: EncodedPath,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2PatchCategory {
    Cheats,
    WidescreenPatches,
    OtherPatches,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2PatchDirectoryState {
    Available,
    Missing,
    UnsafePath,
    NotDirectory,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2PatchDirectory {
    pub path: PathBuf,
    pub category: Pcsx2PatchCategory,
    pub state: Pcsx2PatchDirectoryState,
    pub warning: Option<String>,
    pub identity: Option<Pcsx2DirectoryIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pcsx2DirectoryIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2Profile {
    pub profile_id: String,
    pub installation_type: Pcsx2InstallationType,
    pub scope: Pcsx2ProfileScope,
    pub configuration_path: PathBuf,
    pub provenance: &'static str,
    pub eligible: bool,
    pub blockers: Vec<Pcsx2ProfileBlocker>,
    pub patch_directories: Vec<Pcsx2PatchDirectory>,
    pub configuration_identity: Option<Pcsx2DirectoryIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2ProfileDiscovery {
    pub profiles: Vec<Pcsx2Profile>,
    pub warnings: Vec<Pcsx2ProfileBlocker>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2ProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    /// `~/Documents/PCSX2` on Linux, observed as a data location chosen by
    /// some manually configured or migrated PCSX2 AppImage setups.
    pub documents_home: PathBuf,
    pub flatpak_system_root: PathBuf,
    /// The directory containing the currently running AppImage, taken only
    /// from the `APPIMAGE` environment variable that the AppImage runtime
    /// itself sets. This is the documented location for PCSX2 "portable
    /// mode" (a `portable.ini` file placed beside the executable), so it is
    /// checked as a candidate profile directly, never guessed by searching.
    pub appimage_directory: Option<PathBuf>,
    /// Portable roots must come from an already known PCSX2 configuration,
    /// never from blind filesystem searching.
    pub portable_configuration_roots: Vec<PathBuf>,
}

impl Pcsx2ProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, Pcsx2DiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(Pcsx2DiscoveryError::HomeUnavailable)?;
        let xdg_config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let xdg_data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let documents_home = home.join("Documents");
        let appimage_directory = env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .and_then(|appimage| appimage.parent().map(Path::to_path_buf));
        Ok(Self {
            home,
            xdg_config_home,
            xdg_data_home,
            documents_home,
            flatpak_system_root: PathBuf::from("/var/lib/flatpak"),
            appimage_directory,
            portable_configuration_roots: Vec::new(),
        })
    }
}

#[derive(Debug)]
pub enum Pcsx2DiscoveryError {
    HomeUnavailable,
    Inspection { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for Pcsx2DiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable => formatter.write_str("HOME is not set"),
            Self::Inspection { path, source } => {
                write!(formatter, "failed to inspect {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for Pcsx2DiscoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inspection { source, .. } => Some(source),
            Self::HomeUnavailable => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2InspectionWarningKind {
    UnsafePath,
    UnreadablePath,
    SymlinkSkipped,
    SpecialFileSkipped,
    EntryLimitReached,
    DirectoryLimitReached,
    DepthLimitReached,
    FileCountLimitReached,
    FileTooLarge,
    TotalBytesLimitReached,
    LineCountLimitReached,
    LineTooLong,
    MalformedPnach,
    InvalidUtf8,
    DuplicateCrc,
    DuplicateFilename,
    DuplicateContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2InspectionWarning {
    pub kind: Pcsx2InspectionWarningKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2PnachFile {
    pub path: PathBuf,
    pub filename_stem: OsString,
    pub category: Pcsx2PatchCategory,
    pub crc_candidate: Option<String>,
    pub serial_candidate: Option<String>,
    pub title_candidates: Vec<String>,
    pub region_candidates: Vec<String>,
    pub comments: Vec<String>,
    pub patch_entry_count: usize,
    pub enabled_patch_count: usize,
    pub disabled_patch_count: usize,
    pub unknown_patch_count: usize,
    pub size_bytes: u64,
    pub sha256: String,
    pub duplicate_crc: bool,
    pub duplicate_filename: bool,
    pub duplicate_content: bool,
    pub warnings: Vec<Pcsx2InspectionWarningKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2PnachInventory {
    pub profile_id: String,
    pub files: Vec<Pcsx2PnachFile>,
    pub warnings: Vec<Pcsx2InspectionWarning>,
    pub directories_traversed: usize,
    pub entries_visited: usize,
    pub bytes_inspected: u64,
    pub complete: bool,
}

#[derive(Debug)]
pub enum Pcsx2InspectionError {
    IneligibleProfile { profile_id: String },
    ProfileChanged { path: PathBuf },
    UnsafeProfile { path: PathBuf },
}

impl std::fmt::Display for Pcsx2InspectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IneligibleProfile { profile_id } => {
                write!(formatter, "PCSX2 profile {profile_id} is not eligible")
            }
            Self::ProfileChanged { path } => {
                write!(
                    formatter,
                    "PCSX2 profile changed before inspection: {}",
                    path.display()
                )
            }
            Self::UnsafeProfile { path } => {
                write!(
                    formatter,
                    "PCSX2 profile path is unsafe: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for Pcsx2InspectionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2MatchState {
    ExactCrcMatch,
    MultiplePnachFilesForSameCrc,
    CandidateByFilenameOrTitleOnly,
    NoVerifiedGameCrcAvailable,
    NoMatchingPnachFound,
    InvalidVerifiedGameCrc,
    IdentityExtractionDeferred,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2MatchResult {
    pub state: Pcsx2MatchState,
    pub verified_crc: Option<String>,
    pub matching_files: Vec<PathBuf>,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct ProfileCandidate {
    installation_type: Pcsx2InstallationType,
    scope: Pcsx2ProfileScope,
    configuration_path: PathBuf,
    provenance: &'static str,
    report_missing: bool,
}

/// Discovers only documented paths plus explicitly supplied portable roots.
/// Missing standard paths are ignored; existing but unsafe or unproven paths
/// remain visible as blocked profiles.
pub fn discover_pcsx2_profiles(
    roots: &Pcsx2ProfileDiscoveryRoots,
) -> Result<Pcsx2ProfileDiscovery, Pcsx2DiscoveryError> {
    let flatpak_config = roots
        .home
        .join(".var/app")
        .join(FLATPAK_APP_ID)
        .join("config/PCSX2");
    let user_flatpak_install = roots.xdg_data_home.join("flatpak/app").join(FLATPAK_APP_ID);
    let system_flatpak_install = roots.flatpak_system_root.join("app").join(FLATPAK_APP_ID);
    let user_installed = is_real_directory_no_follow(&user_flatpak_install).unwrap_or(false);
    let system_installed = is_real_directory_no_follow(&system_flatpak_install).unwrap_or(false);
    let flatpak_kind = if system_installed && !user_installed {
        Pcsx2InstallationType::FlatpakSystem
    } else {
        Pcsx2InstallationType::FlatpakUser
    };
    let flatpak_scope = if flatpak_kind == Pcsx2InstallationType::FlatpakSystem {
        Pcsx2ProfileScope::SystemInstallationUserProfile
    } else {
        Pcsx2ProfileScope::User
    };
    let mut candidates = vec![
        ProfileCandidate {
            installation_type: Pcsx2InstallationType::Native,
            scope: Pcsx2ProfileScope::User,
            configuration_path: roots.xdg_config_home.join("PCSX2"),
            provenance: "XDG PCSX2 configuration directory",
            report_missing: false,
        },
        ProfileCandidate {
            installation_type: Pcsx2InstallationType::NativeAlternate,
            scope: Pcsx2ProfileScope::User,
            configuration_path: roots.xdg_data_home.join("PCSX2"),
            provenance: "XDG data-home PCSX2 directory (observed with some AppImage builds)",
            report_missing: false,
        },
        ProfileCandidate {
            installation_type: Pcsx2InstallationType::NativeAlternate,
            scope: Pcsx2ProfileScope::User,
            configuration_path: roots.documents_home.join("PCSX2"),
            provenance: "Documents PCSX2 directory (observed with some AppImage builds)",
            report_missing: false,
        },
        ProfileCandidate {
            installation_type: flatpak_kind,
            scope: flatpak_scope,
            configuration_path: flatpak_config,
            provenance: "Flatpak net.pcsx2.PCSX2 user configuration directory",
            report_missing: false,
        },
    ];
    if let Some(appimage_directory) = &roots.appimage_directory {
        candidates.push(ProfileCandidate {
            installation_type: Pcsx2InstallationType::Portable,
            scope: Pcsx2ProfileScope::Portable,
            configuration_path: appimage_directory.clone(),
            provenance: "Portable mode beside the running AppImage (from $APPIMAGE)",
            report_missing: false,
        });
    }
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|path| ProfileCandidate {
                installation_type: Pcsx2InstallationType::Portable,
                scope: Pcsx2ProfileScope::Portable,
                configuration_path: path,
                provenance: "Explicitly known PCSX2 portable configuration directory",
                report_missing: true,
            }),
    );
    candidates.sort_by(|left, right| left.configuration_path.cmp(&right.configuration_path));
    candidates.dedup_by(|left, right| left.configuration_path == right.configuration_path);

    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for candidate in candidates {
        if profiles.len() >= PCSX2_MAX_PROFILES {
            warnings.push(blocker(
                Pcsx2ProfileBlockerKind::ProfileLimitReached,
                &candidate.configuration_path,
                format!("profile discovery stopped at the {PCSX2_MAX_PROFILES}-profile limit"),
            ));
            break;
        }
        if !candidate.configuration_path.is_absolute() {
            profiles.push(blocked_profile(
                candidate,
                Pcsx2ProfileBlockerKind::PathNotAbsolute,
                "configuration path is not absolute",
            ));
            continue;
        }
        if candidate.configuration_path.parent().is_none() {
            profiles.push(blocked_profile(
                candidate,
                Pcsx2ProfileBlockerKind::FilesystemRoot,
                "a filesystem root cannot be a PCSX2 profile",
            ));
            continue;
        }
        let validated = match validate_destination_root(&candidate.configuration_path) {
            Ok(validated) => validated,
            Err(error) => {
                let kind = match error.reason {
                    DestinationSafetyFailureReason::RootNotDirectory
                    | DestinationSafetyFailureReason::NonDirectoryParent => {
                        Pcsx2ProfileBlockerKind::NotDirectory
                    }
                    DestinationSafetyFailureReason::InspectionFailed => {
                        Pcsx2ProfileBlockerKind::Unreadable
                    }
                    _ => Pcsx2ProfileBlockerKind::UnsafePath,
                };
                profiles.push(blocked_profile(
                    candidate,
                    kind,
                    format!("configuration path rejected: {:?}", error.reason),
                ));
                continue;
            }
        };
        if validated.state() == DestinationRootState::Absent {
            if candidate.report_missing {
                profiles.push(blocked_profile(
                    candidate,
                    Pcsx2ProfileBlockerKind::MissingConfiguration,
                    "configuration directory does not exist",
                ));
            }
            continue;
        }
        let marker_state = inspect_pcsx2_marker(&candidate.configuration_path);
        if let Err((kind, detail)) = marker_state {
            profiles.push(blocked_profile(candidate, kind, detail));
            continue;
        }
        let configuration_identity = fs::symlink_metadata(&candidate.configuration_path)
            .ok()
            .and_then(|metadata| directory_identity(&metadata));
        let patch_directories = known_patch_directories(&candidate.configuration_path);
        profiles.push(Pcsx2Profile {
            profile_id: profile_id(candidate.installation_type, &candidate.configuration_path),
            installation_type: candidate.installation_type,
            scope: candidate.scope,
            configuration_path: candidate.configuration_path,
            provenance: candidate.provenance,
            eligible: true,
            blockers: Vec::new(),
            patch_directories,
            configuration_identity,
        });
    }
    profiles.sort_by(|left, right| {
        left.installation_type
            .cmp(&right.installation_type)
            .then_with(|| left.configuration_path.cmp(&right.configuration_path))
    });
    let complete = warnings.is_empty();
    Ok(Pcsx2ProfileDiscovery {
        profiles,
        warnings,
        complete,
    })
}

fn inspect_pcsx2_marker(root: &Path) -> Result<(), (Pcsx2ProfileBlockerKind, &'static str)> {
    let markers = [root.join("inis"), root.join("PCSX2.ini")];
    for marker in markers {
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err((
                    Pcsx2ProfileBlockerKind::UnsafePath,
                    "PCSX2 evidence path is a symlink",
                ));
            }
            Ok(metadata) if metadata.is_dir() || metadata.is_file() => return Ok(()),
            Ok(_) => {
                return Err((
                    Pcsx2ProfileBlockerKind::MissingPcsx2Evidence,
                    "PCSX2 evidence path has an unsupported file type",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err((
                    Pcsx2ProfileBlockerKind::Unreadable,
                    "PCSX2 evidence path is unreadable",
                ));
            }
        }
    }
    Err((
        Pcsx2ProfileBlockerKind::MissingPcsx2Evidence,
        "no PCSX2.ini file or inis directory was found",
    ))
}

fn known_patch_directories(root: &Path) -> Vec<Pcsx2PatchDirectory> {
    [
        ("cheats", Pcsx2PatchCategory::Cheats, true),
        ("cheats_ws", Pcsx2PatchCategory::WidescreenPatches, true),
        ("patches", Pcsx2PatchCategory::OtherPatches, false),
    ]
    .into_iter()
    .filter_map(|(name, category, report_missing)| {
        let path = root.join(name);
        let (state, warning, identity) = inspect_patch_directory(&path);
        (report_missing || state != Pcsx2PatchDirectoryState::Missing).then_some(
            Pcsx2PatchDirectory {
                path,
                category,
                state,
                warning,
                identity,
            },
        )
    })
    .take(PCSX2_MAX_PATCH_DIRECTORIES_PER_PROFILE)
    .collect()
}

fn inspect_patch_directory(
    path: &Path,
) -> (
    Pcsx2PatchDirectoryState,
    Option<String>,
    Option<Pcsx2DirectoryIdentity>,
) {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => (
            Pcsx2PatchDirectoryState::UnsafePath,
            Some("directory is a symlink and will not be followed".to_string()),
            None,
        ),
        Ok(metadata) if metadata.is_dir() => (
            Pcsx2PatchDirectoryState::Available,
            None,
            directory_identity(&metadata),
        ),
        Ok(_) => (
            Pcsx2PatchDirectoryState::NotDirectory,
            Some("path is not a directory".to_string()),
            None,
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            (Pcsx2PatchDirectoryState::Missing, None, None)
        }
        Err(error) => (
            Pcsx2PatchDirectoryState::Unreadable,
            Some(format!("directory cannot be inspected: {error}")),
            None,
        ),
    }
}

fn blocked_profile(
    candidate: ProfileCandidate,
    kind: Pcsx2ProfileBlockerKind,
    detail: impl Into<String>,
) -> Pcsx2Profile {
    Pcsx2Profile {
        profile_id: profile_id(candidate.installation_type, &candidate.configuration_path),
        installation_type: candidate.installation_type,
        scope: candidate.scope,
        blockers: vec![blocker(kind, &candidate.configuration_path, detail)],
        configuration_path: candidate.configuration_path,
        provenance: candidate.provenance,
        eligible: false,
        patch_directories: Vec::new(),
        configuration_identity: None,
    }
}

fn blocker(
    kind: Pcsx2ProfileBlockerKind,
    path: &Path,
    detail: impl Into<String>,
) -> Pcsx2ProfileBlocker {
    Pcsx2ProfileBlocker {
        kind,
        path: EncodedPath::from_path(path),
        detail: detail.into(),
    }
}

fn is_real_directory_no_follow(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn profile_id(kind: Pcsx2InstallationType, path: &Path) -> String {
    let mut digest = Sha256::new();
    #[cfg(unix)]
    digest.update(path.as_os_str().as_bytes());
    #[cfg(not(unix))]
    digest.update(path.as_os_str().to_string_lossy().as_bytes());
    let kind = match kind {
        Pcsx2InstallationType::Native => "native",
        Pcsx2InstallationType::NativeAlternate => "native-alt",
        Pcsx2InstallationType::FlatpakUser => "flatpak-user",
        Pcsx2InstallationType::FlatpakSystem => "flatpak-system",
        Pcsx2InstallationType::Portable => "portable",
    };
    format!(
        "pcsx2-{kind}-{:016x}",
        u64::from_be_bytes(digest.finalize()[..8].try_into().unwrap())
    )
}

/// Inspects the currently discovered directories again. This rejects a
/// profile whose root became unsafe or disappeared after discovery.
pub fn inspect_pcsx2_profile(
    profile: &Pcsx2Profile,
) -> Result<Pcsx2PnachInventory, Pcsx2InspectionError> {
    inspect_pcsx2_profile_with_limits(profile, PCSX2_MAX_PNACH_FILES, PCSX2_MAX_DIRECTORY_DEPTH)
}

fn inspect_pcsx2_profile_with_limits(
    profile: &Pcsx2Profile,
    max_pnach_files: usize,
    max_directory_depth: usize,
) -> Result<Pcsx2PnachInventory, Pcsx2InspectionError> {
    if !profile.eligible {
        return Err(Pcsx2InspectionError::IneligibleProfile {
            profile_id: profile.profile_id.clone(),
        });
    }
    let validated = validate_destination_root(&profile.configuration_path).map_err(|_| {
        Pcsx2InspectionError::UnsafeProfile {
            path: profile.configuration_path.clone(),
        }
    })?;
    if validated.state() != DestinationRootState::ExistingDirectory
        || inspect_pcsx2_marker(&profile.configuration_path).is_err()
    {
        return Err(Pcsx2InspectionError::ProfileChanged {
            path: profile.configuration_path.clone(),
        });
    }
    let current_identity = fs::symlink_metadata(&profile.configuration_path)
        .ok()
        .and_then(|metadata| directory_identity(&metadata));
    if profile.configuration_identity.is_some()
        && current_identity != profile.configuration_identity
    {
        return Err(Pcsx2InspectionError::ProfileChanged {
            path: profile.configuration_path.clone(),
        });
    }

    let mut inventory = Pcsx2PnachInventory {
        profile_id: profile.profile_id.clone(),
        files: Vec::new(),
        warnings: Vec::new(),
        directories_traversed: 0,
        entries_visited: 0,
        bytes_inspected: 0,
        complete: true,
    };
    for directory in profile
        .patch_directories
        .iter()
        .filter(|directory| directory.state == Pcsx2PatchDirectoryState::Available)
    {
        if inventory.directories_traversed >= PCSX2_MAX_DIRECTORIES_TRAVERSED {
            limit_warning(
                &mut inventory,
                Pcsx2InspectionWarningKind::DirectoryLimitReached,
                &directory.path,
                format!("directory traversal stopped at {PCSX2_MAX_DIRECTORIES_TRAVERSED}"),
            );
            break;
        }
        if inventory.entries_visited >= PCSX2_MAX_ENTRIES_VISITED {
            limit_warning(
                &mut inventory,
                Pcsx2InspectionWarningKind::EntryLimitReached,
                &directory.path,
                format!("entry inspection stopped at {PCSX2_MAX_ENTRIES_VISITED}"),
            );
            break;
        }
        if inventory.files.len() >= max_pnach_files {
            limit_warning(
                &mut inventory,
                Pcsx2InspectionWarningKind::FileCountLimitReached,
                &directory.path,
                format!("PNACH parsing stopped at {max_pnach_files} files"),
            );
            break;
        }
        if inventory.bytes_inspected >= PCSX2_MAX_TOTAL_PNACH_BYTES {
            limit_warning(
                &mut inventory,
                Pcsx2InspectionWarningKind::TotalBytesLimitReached,
                &directory.path,
                format!("total input reached {PCSX2_MAX_TOTAL_PNACH_BYTES} bytes"),
            );
            break;
        }
        inspect_patch_tree(
            directory,
            &mut inventory,
            max_pnach_files,
            max_directory_depth,
        )?;
    }
    mark_duplicates(&mut inventory);
    inventory
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    inventory.warnings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| format!("{:?}", left.kind).cmp(&format!("{:?}", right.kind)))
    });
    Ok(inventory)
}

fn inspect_patch_tree(
    directory: &Pcsx2PatchDirectory,
    inventory: &mut Pcsx2PnachInventory,
    max_pnach_files: usize,
    max_directory_depth: usize,
) -> Result<(), Pcsx2InspectionError> {
    match validate_destination_root(&directory.path) {
        Ok(root) if root.state() == DestinationRootState::ExistingDirectory => {}
        Ok(_) => return Ok(()),
        Err(_) => {
            inventory.complete = false;
            inventory.warnings.push(warning(
                Pcsx2InspectionWarningKind::UnsafePath,
                &directory.path,
                "patch directory became unsafe after profile discovery",
            ));
            return Ok(());
        }
    }
    let current_identity = fs::symlink_metadata(&directory.path)
        .ok()
        .and_then(|metadata| directory_identity(&metadata));
    if directory.identity.is_some() && current_identity != directory.identity {
        inventory.complete = false;
        inventory.warnings.push(warning(
            Pcsx2InspectionWarningKind::UnsafePath,
            &directory.path,
            "patch directory identity changed after profile discovery",
        ));
        return Ok(());
    }
    let mut pending = VecDeque::from([(directory.path.clone(), 0_usize, directory.identity)]);
    while let Some((path, depth, expected_identity)) = pending.pop_front() {
        if inventory.directories_traversed >= PCSX2_MAX_DIRECTORIES_TRAVERSED {
            limit_warning(
                inventory,
                Pcsx2InspectionWarningKind::DirectoryLimitReached,
                &path,
                format!("directory traversal stopped at {PCSX2_MAX_DIRECTORIES_TRAVERSED}"),
            );
            break;
        }
        inventory.directories_traversed += 1;
        let validated = validate_destination_root(&path);
        let current_identity = fs::symlink_metadata(&path)
            .ok()
            .and_then(|metadata| directory_identity(&metadata));
        if !matches!(validated, Ok(root) if root.state() == DestinationRootState::ExistingDirectory)
            || (expected_identity.is_some() && current_identity != expected_identity)
        {
            inventory.complete = false;
            inventory.warnings.push(warning(
                Pcsx2InspectionWarningKind::UnsafePath,
                &path,
                "directory path or identity changed before traversal",
            ));
            continue;
        }
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) => {
                inventory.complete = false;
                inventory.warnings.push(warning(
                    Pcsx2InspectionWarningKind::UnreadablePath,
                    &path,
                    format!("directory cannot be read: {error}"),
                ));
                continue;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            if inventory.entries_visited >= PCSX2_MAX_ENTRIES_VISITED {
                limit_warning(
                    inventory,
                    Pcsx2InspectionWarningKind::EntryLimitReached,
                    &path,
                    format!("entry inspection stopped at {PCSX2_MAX_ENTRIES_VISITED}"),
                );
                return Ok(());
            }
            inventory.entries_visited += 1;
            match entry {
                Ok(entry) => children.push(entry.path()),
                Err(error) => {
                    inventory.complete = false;
                    inventory.warnings.push(warning(
                        Pcsx2InspectionWarningKind::UnreadablePath,
                        &path,
                        format!("directory entry cannot be read: {error}"),
                    ));
                }
            }
        }
        children.sort();
        for child in children {
            let metadata = match fs::symlink_metadata(&child) {
                Ok(metadata) => metadata,
                Err(error) => {
                    inventory.complete = false;
                    inventory.warnings.push(warning(
                        Pcsx2InspectionWarningKind::UnreadablePath,
                        &child,
                        format!("entry metadata cannot be read: {error}"),
                    ));
                    continue;
                }
            };
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                inventory.complete = false;
                inventory.warnings.push(warning(
                    Pcsx2InspectionWarningKind::SymlinkSkipped,
                    &child,
                    "symlink entry was not followed",
                ));
            } else if metadata.is_dir() {
                if depth >= max_directory_depth {
                    inventory.complete = false;
                    inventory.warnings.push(warning(
                        Pcsx2InspectionWarningKind::DepthLimitReached,
                        &child,
                        format!("directory depth exceeds {max_directory_depth}"),
                    ));
                } else {
                    pending.push_back((child, depth + 1, directory_identity(&metadata)));
                }
            } else if metadata.is_file() {
                if is_pnach_path(&child) {
                    inspect_pnach_file(
                        &child,
                        directory.category,
                        inventory,
                        max_pnach_files,
                        current_identity,
                    );
                }
            } else {
                inventory.complete = false;
                inventory.warnings.push(warning(
                    Pcsx2InspectionWarningKind::SpecialFileSkipped,
                    &child,
                    "special filesystem entry was not opened",
                ));
            }
        }
    }
    Ok(())
}

fn inspect_pnach_file(
    path: &Path,
    category: Pcsx2PatchCategory,
    inventory: &mut Pcsx2PnachInventory,
    max_pnach_files: usize,
    expected_parent_identity: Option<Pcsx2DirectoryIdentity>,
) {
    if inventory.files.len() >= max_pnach_files {
        limit_warning(
            inventory,
            Pcsx2InspectionWarningKind::FileCountLimitReached,
            path,
            format!("PNACH parsing stopped at {max_pnach_files} files"),
        );
        return;
    }
    let parent_is_stable = path.parent().is_some_and(|parent| {
        matches!(
            validate_destination_root(parent),
            Ok(root) if root.state() == DestinationRootState::ExistingDirectory
        ) && (expected_parent_identity.is_none()
            || fs::symlink_metadata(parent)
                .ok()
                .and_then(|metadata| directory_identity(&metadata))
                == expected_parent_identity)
    });
    if !parent_is_stable {
        inventory.complete = false;
        inventory.warnings.push(warning(
            Pcsx2InspectionWarningKind::UnsafePath,
            path,
            "PNACH parent path or identity changed before file open",
        ));
        return;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) => {
            inventory.complete = false;
            inventory.warnings.push(warning(
                Pcsx2InspectionWarningKind::UnreadablePath,
                path,
                format!("PNACH file cannot be opened safely: {error}"),
            ));
            return;
        }
    };
    let metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            inventory.complete = false;
            inventory.warnings.push(warning(
                Pcsx2InspectionWarningKind::SpecialFileSkipped,
                path,
                "opened entry is not a regular file",
            ));
            return;
        }
        Err(error) => {
            inventory.complete = false;
            inventory.warnings.push(warning(
                Pcsx2InspectionWarningKind::UnreadablePath,
                path,
                format!("opened PNACH metadata cannot be read: {error}"),
            ));
            return;
        }
    };
    if metadata.len() > PCSX2_MAX_PNACH_FILE_BYTES {
        inventory.complete = false;
        inventory.warnings.push(warning(
            Pcsx2InspectionWarningKind::FileTooLarge,
            path,
            format!("file exceeds the {PCSX2_MAX_PNACH_FILE_BYTES}-byte limit"),
        ));
        return;
    }
    if inventory.bytes_inspected.saturating_add(metadata.len()) > PCSX2_MAX_TOTAL_PNACH_BYTES {
        limit_warning(
            inventory,
            Pcsx2InspectionWarningKind::TotalBytesLimitReached,
            path,
            format!("total input exceeds the {PCSX2_MAX_TOTAL_PNACH_BYTES}-byte limit"),
        );
        return;
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if let Err(error) = file
        .by_ref()
        .take(PCSX2_MAX_PNACH_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        inventory.complete = false;
        inventory.warnings.push(warning(
            Pcsx2InspectionWarningKind::UnreadablePath,
            path,
            format!("PNACH file cannot be read: {error}"),
        ));
        return;
    }
    if bytes.len() as u64 > PCSX2_MAX_PNACH_FILE_BYTES {
        inventory.complete = false;
        inventory.warnings.push(warning(
            Pcsx2InspectionWarningKind::FileTooLarge,
            path,
            "file grew beyond the per-file limit while being read",
        ));
        return;
    }
    let parsed = match parse_pnach(path, category, &bytes) {
        Ok(parsed) => parsed,
        Err((kind, detail)) => {
            inventory.complete = false;
            inventory.warnings.push(warning(kind, path, detail));
            return;
        }
    };
    inventory.bytes_inspected = inventory.bytes_inspected.saturating_add(bytes.len() as u64);
    for kind in &parsed.warnings {
        inventory.warnings.push(warning(
            *kind,
            path,
            match kind {
                Pcsx2InspectionWarningKind::InvalidUtf8 => {
                    "PNACH contains invalid UTF-8; metadata was decoded lossily"
                }
                Pcsx2InspectionWarningKind::MalformedPnach => {
                    "PNACH contains unrecognized patch syntax"
                }
                _ => "PNACH metadata warning",
            },
        ));
    }
    inventory.files.push(parsed);
}

fn parse_pnach(
    path: &Path,
    category: Pcsx2PatchCategory,
    bytes: &[u8],
) -> Result<Pcsx2PnachFile, (Pcsx2InspectionWarningKind, String)> {
    let lines: Vec<&[u8]> = bytes.split(|byte| *byte == b'\n').collect();
    if lines.len() > PCSX2_MAX_LINES_PER_FILE {
        return Err((
            Pcsx2InspectionWarningKind::LineCountLimitReached,
            format!("file exceeds the {PCSX2_MAX_LINES_PER_FILE}-line limit"),
        ));
    }
    if lines.iter().any(|line| line.len() > PCSX2_MAX_LINE_BYTES) {
        return Err((
            Pcsx2InspectionWarningKind::LineTooLong,
            format!("line exceeds the {PCSX2_MAX_LINE_BYTES}-byte limit"),
        ));
    }
    let mut file_warnings = Vec::new();
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            file_warnings.push(Pcsx2InspectionWarningKind::InvalidUtf8);
            String::from_utf8_lossy(bytes).into_owned()
        }
    };
    let mut title_candidates = BTreeSet::new();
    let mut region_candidates = BTreeSet::new();
    let mut comments = Vec::new();
    let mut patch_entry_count = 0_usize;
    let mut enabled_patch_count = 0_usize;
    let mut disabled_patch_count = 0_usize;
    let mut unknown_patch_count = 0_usize;
    for line in text.lines() {
        let line = line.trim();
        let lower = line.to_ascii_lowercase();
        if let Some(value) = line_value(line, &lower, "gametitle=") {
            if !value.is_empty() {
                title_candidates.insert(value.to_string());
            }
        } else if let Some(value) = line_value(line, &lower, "region=") {
            if !value.is_empty() {
                region_candidates.insert(value.to_string());
            }
        } else if let Some(value) = line_value(line, &lower, "comment=") {
            if !value.is_empty() && comments.len() < PCSX2_MAX_RETAINED_COMMENTS_PER_FILE {
                comments.push(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("//").or_else(|| line.strip_prefix('#')) {
            let value = value.trim();
            if !value.is_empty() && comments.len() < PCSX2_MAX_RETAINED_COMMENTS_PER_FILE {
                comments.push(value.to_string());
            }
        } else if let Some(rest) = lower.strip_prefix("patch=") {
            patch_entry_count += 1;
            match rest.split(',').next().map(str::trim) {
                Some("1") => enabled_patch_count += 1,
                Some("0") => disabled_patch_count += 1,
                _ => {
                    unknown_patch_count += 1;
                    file_warnings.push(Pcsx2InspectionWarningKind::MalformedPnach);
                }
            }
        }
    }
    file_warnings.sort_by_key(|warning| format!("{warning:?}"));
    file_warnings.dedup();
    let stem = path.file_stem().unwrap_or_default().to_os_string();
    let (serial_candidate, crc_candidate) = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(parse_patch_identity)
        .unwrap_or((None, None));
    let sha256 = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(Pcsx2PnachFile {
        path: path.to_path_buf(),
        filename_stem: stem,
        category,
        crc_candidate,
        serial_candidate,
        title_candidates: title_candidates.into_iter().collect(),
        region_candidates: region_candidates.into_iter().collect(),
        comments,
        patch_entry_count,
        enabled_patch_count,
        disabled_patch_count,
        unknown_patch_count,
        size_bytes: bytes.len() as u64,
        sha256,
        duplicate_crc: false,
        duplicate_filename: false,
        duplicate_content: false,
        warnings: file_warnings,
    })
}

fn line_value<'a>(line: &'a str, lower: &str, prefix: &str) -> Option<&'a str> {
    lower
        .strip_prefix(prefix)
        .map(|suffix| &line[line.len() - suffix.len()..])
}

fn mark_duplicates(inventory: &mut Pcsx2PnachInventory) {
    let mut crcs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut names: BTreeMap<OsString, Vec<usize>> = BTreeMap::new();
    let mut digests: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, file) in inventory.files.iter().enumerate() {
        if let Some(crc) = &file.crc_candidate {
            crcs.entry(crc.clone()).or_default().push(index);
        }
        names
            .entry(file.path.file_name().unwrap_or_default().to_os_string())
            .or_default()
            .push(index);
        digests.entry(file.sha256.clone()).or_default().push(index);
    }
    for indices in crcs.values().filter(|indices| indices.len() > 1) {
        for index in indices {
            inventory.files[*index].duplicate_crc = true;
            inventory.files[*index]
                .warnings
                .push(Pcsx2InspectionWarningKind::DuplicateCrc);
        }
    }
    for indices in names.values().filter(|indices| indices.len() > 1) {
        for index in indices {
            inventory.files[*index].duplicate_filename = true;
            inventory.files[*index]
                .warnings
                .push(Pcsx2InspectionWarningKind::DuplicateFilename);
        }
    }
    for indices in digests.values().filter(|indices| indices.len() > 1) {
        for index in indices {
            inventory.files[*index].duplicate_content = true;
            inventory.files[*index]
                .warnings
                .push(Pcsx2InspectionWarningKind::DuplicateContent);
        }
    }
}

fn is_pnach_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pnach"))
}

fn warning(
    kind: Pcsx2InspectionWarningKind,
    path: &Path,
    detail: impl Into<String>,
) -> Pcsx2InspectionWarning {
    Pcsx2InspectionWarning {
        kind,
        path: path.to_path_buf(),
        detail: detail.into(),
    }
}

fn limit_warning(
    inventory: &mut Pcsx2PnachInventory,
    kind: Pcsx2InspectionWarningKind,
    path: &Path,
    detail: impl Into<String>,
) {
    inventory.complete = false;
    inventory.warnings.push(warning(kind, path, detail));
}

/// Matches only against a caller-supplied verified executable CRC. Filename
/// and comment evidence can produce a candidate state, never an exact match.
pub fn match_pcsx2_inventory(
    inventory: &Pcsx2PnachInventory,
    verified_crc: Option<&str>,
    archive_title: Option<&str>,
) -> Pcsx2MatchResult {
    if let Some(value) = verified_crc {
        let Some(crc) = normalize_crc(value) else {
            return Pcsx2MatchResult {
                state: Pcsx2MatchState::InvalidVerifiedGameCrc,
                verified_crc: None,
                matching_files: Vec::new(),
                reason: "the supplied verified game CRC is not eight hexadecimal digits".into(),
            };
        };
        let matching_files: Vec<PathBuf> = inventory
            .files
            .iter()
            .filter(|file| file.crc_candidate.as_deref() == Some(crc.as_str()))
            .map(|file| file.path.clone())
            .collect();
        let (state, reason) = match matching_files.len() {
            0 => (
                Pcsx2MatchState::NoMatchingPnachFound,
                "no inspected PNACH filename contains the verified game CRC",
            ),
            1 => (
                Pcsx2MatchState::ExactCrcMatch,
                "one PNACH filename matches the verified game CRC",
            ),
            _ => (
                Pcsx2MatchState::MultiplePnachFilesForSameCrc,
                "multiple PNACH files match the verified game CRC",
            ),
        };
        return Pcsx2MatchResult {
            state,
            verified_crc: Some(crc),
            matching_files,
            reason: reason.into(),
        };
    }
    let normalized_title = archive_title
        .map(normalize_title)
        .filter(|title| !title.is_empty());
    let matching_files: Vec<PathBuf> = normalized_title
        .as_deref()
        .map(|wanted| {
            inventory
                .files
                .iter()
                .filter(|file| {
                    file.title_candidates
                        .iter()
                        .any(|title| normalize_title(title) == wanted)
                        || file
                            .filename_stem
                            .to_str()
                            .is_some_and(|stem| normalize_title(stem) == wanted)
                })
                .map(|file| file.path.clone())
                .collect()
        })
        .unwrap_or_default();
    if !matching_files.is_empty() {
        Pcsx2MatchResult {
            state: Pcsx2MatchState::CandidateByFilenameOrTitleOnly,
            verified_crc: None,
            matching_files,
            reason:
                "filename or comment-title matches are unverified candidates, not exact identity"
                    .into(),
        }
    } else {
        Pcsx2MatchResult {
            state: Pcsx2MatchState::NoVerifiedGameCrcAvailable,
            verified_crc: None,
            matching_files: Vec::new(),
            reason: "EmuWiz has no verified PCSX2 executable CRC for this archive".into(),
        }
    }
}

fn normalize_title(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(unix)]
fn directory_identity(metadata: &fs::Metadata) -> Option<Pcsx2DirectoryIdentity> {
    metadata.is_dir().then(|| Pcsx2DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn directory_identity(_metadata: &fs::Metadata) -> Option<Pcsx2DirectoryIdentity> {
    None
}

// =======================================================================
// Emulator Adapter Refresh Batch H: modernisation additions.
//
// Everything below is strictly additive - no existing public type, field,
// or function signature above this point is changed. `Pcsx2Profile`,
// `Pcsx2ProfileDiscovery`, `discover_pcsx2_profiles`,
// `inspect_pcsx2_profile`, `Pcsx2PnachInventory`, and `match_pcsx2_inventory`
// are all reused completely unchanged - the existing GUI workflow and CLI
// preview command that already consume them keep compiling and behaving
// exactly as before. This section only adds the version/BIOS/config/
// texture/memory-card/save-state/controller inspection PPSSPP and RPCS3
// already have, plus one new top-level `inspect_pcsx2_game` entry point
// that ties them together for a "selected verified title" summary - the
// same shape `ppsspp_local::inspect_ppsspp_game`/
// `rpcs3_local::inspect_rpcs3_game` already establish.
//
// # PS2 serial vs. PCSX2 executable CRC - two different keys, kept apart
//
// PCSX2's own PNACH patch/cheat matching (`match_pcsx2_inventory`, above)
// is keyed by the game's executable CRC - unchanged, and this section
// never re-derives or second-guesses it. Real PCSX2's own per-game
// config/texture/memory-card/save-state directories, by contrast, are
// keyed by PS2 *serial* (e.g. `SLUS-20312`), a different identifier this
// module has never modeled before. [`Pcsx2GameRequest`] therefore carries
// both, kept as two genuinely separate fields - see its own doc comment.
// Neither is ever derived from a filename, a PNACH comment, or a
// directory name; both must come from the caller already resolved (see
// [`crate::game_identity::serial_from_boot_path`] for where a real
// verified serial ultimately comes from, and `pcsx2::normalize_crc`/
// `pcsx2_identity::Pcsx2GameIdentity::verified_crc` for the CRC side).
// =======================================================================

/// The generic bounded-read primitive every new inspection function below
/// shares - the same O_NOFOLLOW/size-bound/symlink-rejection discipline
/// `inspect_pnach_file` (above) already uses, factored out as a plain
/// `(bytes, warning)` helper since the new callers below have no
/// PNACH-inventory-specific bookkeeping to fold a warning into.
fn read_bounded(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err("not found".to_string());
        }
        Err(error) => return Err(format!("cannot be inspected: {error}")),
    };
    if metadata.file_type().is_symlink() {
        return Err("symlink was not followed".to_string());
    }
    if !metadata.is_file() {
        return Err("non-regular file was skipped".to_string());
    }
    if metadata.len() > maximum_bytes {
        return Err(format!("file exceeds the {maximum_bytes}-byte limit"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(path)
        .map_err(|error| format!("cannot be opened read-only: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot be read: {error}"))?;
    if bytes.len() as u64 > maximum_bytes {
        return Err(format!(
            "file grew beyond the {maximum_bytes}-byte limit while reading"
        ));
    }
    Ok(bytes)
}

fn read_bounded_text(path: &Path, maximum_bytes: u64) -> Result<String, String> {
    let bytes = read_bounded(path, maximum_bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn is_regular_file_no_follow(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

pub const PCSX2_MAX_CONFIG_BYTES: u64 = 512 * 1024;
pub const PCSX2_MAX_TEXTURE_FILES: usize = 2_048;
pub const PCSX2_MAX_MEMCARD_CANDIDATES: usize = 32;
pub const PCSX2_MAX_SAVESTATE_CANDIDATES: usize = 512;
const MAX_INI_LINES: usize = 8_192;
const MAX_INI_LINE_BYTES: usize = 8 * 1024;
const MAX_RETAINED_UNKNOWN_SETTINGS: usize = 256;
const MAX_RETAINED_CONTROLLER_SECTIONS: usize = 16;

/// Parses a PCSX2 version from output already obtained by a caller. This
/// module still never executes a binary itself; discovery stays read-only.
/// Conservative and fail-soft: an unrecognised/changed `--version` shape
/// yields `None` rather than a guessed value.
pub fn parse_pcsx2_version(output: &str) -> Option<String> {
    let normalized = output.trim();
    let index = normalized
        .find("PCSX2 ")
        .map(|index| index + "PCSX2 ".len())
        .or_else(|| normalized.find('v').map(|index| index + 1))?;
    let tail = normalized[index..].trim_start_matches('v');
    let version: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    (version.split('.').count() >= 2 && version.chars().any(|character| character.is_ascii_digit()))
        .then_some(version)
}

// ---------------------------------------------------------------------
// BIOS
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2BiosVerification {
    /// A trusted hash helper/database confirmed this exact file. No such
    /// helper exists anywhere in this codebase today (audited before
    /// writing this module - see this section's own doc comment), so this
    /// variant is never produced yet; it exists so a future real verifier
    /// can be wired in without another breaking change.
    Verified,
    /// A file was found where a BIOS is expected, but nothing verified its
    /// contents - filename alone never verifies a BIOS.
    PresentUnverified,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2BiosInfo {
    pub path: Option<PathBuf>,
    pub verification: Pcsx2BiosVerification,
    /// A plain filename-derived hint only (real PCSX2 BIOS filenames often
    /// encode region, e.g. `SCPH-70012.bin`) - never used as verification,
    /// only as a label.
    pub filename_hint: Option<String>,
    pub warnings: Vec<String>,
}

/// Bounded presence inspection of `bios_root` (real PCSX2 layout:
/// `<configuration_path>/bios`). Never opens/hashes the BIOS image itself
/// (no trusted BIOS hash database exists in this codebase - see
/// [`Pcsx2BiosVerification::Verified`]'s own doc comment) - only lists
/// candidate files by extension and reports the first one found.
fn inspect_pcsx2_bios(bios_root: &Path) -> Pcsx2BiosInfo {
    let mut warnings = Vec::new();
    let read_dir = match fs::read_dir(bios_root) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Pcsx2BiosInfo {
                path: None,
                verification: Pcsx2BiosVerification::Missing,
                filename_hint: None,
                warnings,
            };
        }
        Err(error) => {
            return Pcsx2BiosInfo {
                path: None,
                verification: Pcsx2BiosVerification::Unreadable,
                filename_hint: None,
                warnings: vec![format!("BIOS directory cannot be inspected: {error}")],
            };
        }
    };
    let candidate = read_dir.flatten().map(|entry| entry.path()).find(|path| {
        is_regular_file_no_follow(path)
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"))
    });
    match candidate {
        Some(path) => {
            let filename_hint = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned());
            Pcsx2BiosInfo {
                path: Some(path),
                verification: Pcsx2BiosVerification::PresentUnverified,
                filename_hint,
                warnings,
            }
        }
        None => {
            warnings.push("no .bin file was found in the BIOS directory".to_string());
            Pcsx2BiosInfo {
                path: None,
                verification: Pcsx2BiosVerification::Missing,
                filename_hint: None,
                warnings,
            }
        }
    }
}

// ---------------------------------------------------------------------
// Global / per-game config (bounded INI-with-sections parser)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pcsx2Settings {
    pub renderer: Option<String>,
    pub internal_resolution: Option<String>,
    pub texture_filtering: Option<String>,
    pub anisotropic_filtering: Option<String>,
    pub deinterlacing: Option<String>,
    pub widescreen_patches_enabled: Option<bool>,
    pub frame_limiter: Option<String>,
    pub vsync: Option<bool>,
    pub ee_cycle_rate: Option<String>,
    pub ee_cycle_skip: Option<String>,
    pub mtvu_enabled: Option<bool>,
    pub audio_backend: Option<String>,
    pub cheats_enabled: Option<bool>,
    pub patches_enabled: Option<bool>,
    pub texture_replacement_enabled: Option<bool>,
    /// Unknown keys are retained in bounded form for later UI display.
    pub unknown: BTreeMap<String, String>,
    /// Section names actually observed (bounded), used only to derive
    /// [`Pcsx2ControllerInfo`] - never treated as settings themselves.
    controller_sections: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2Config {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub settings: Pcsx2Settings,
    pub warnings: Vec<String>,
}

/// Locates PCSX2's global config, preferring the current Qt-era
/// `inis/PCSX2.ini` and falling back to the older `PCSX2.ini` directly at
/// the profile root (the same two locations `inspect_pcsx2_marker`,
/// above, already treats as equally valid PCSX2 evidence).
fn pcsx2_global_config_path(configuration_path: &Path) -> PathBuf {
    let modern = configuration_path.join("inis/PCSX2.ini");
    if is_regular_file_no_follow(&modern) {
        modern
    } else {
        configuration_path.join("PCSX2.ini")
    }
}

fn inspect_pcsx2_config(path: &Path) -> Pcsx2Config {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let text = match read_bounded_text(path, PCSX2_MAX_CONFIG_BYTES) {
        Ok(text) => text,
        Err(_) if !exists => {
            return Pcsx2Config {
                path: path.to_path_buf(),
                exists,
                readable: false,
                settings: Pcsx2Settings::default(),
                warnings,
            };
        }
        Err(detail) => {
            warnings.push(format!("PCSX2 config could not be read: {detail}"));
            return Pcsx2Config {
                path: path.to_path_buf(),
                exists,
                readable: false,
                settings: Pcsx2Settings::default(),
                warnings,
            };
        }
    };
    let settings = parse_pcsx2_ini(&text, &mut warnings);
    Pcsx2Config {
        path: path.to_path_buf(),
        exists,
        readable: true,
        settings,
        warnings,
    }
}

/// A narrow, bounded parser for PCSX2.ini's real `[Section]` /
/// `Key = Value` shape - never a general INI implementation. An
/// unrecognised key, a malformed line, or content beyond the line/byte
/// bounds fails soft (skipped or retained bounded in `unknown`), never a
/// panic and never a guessed value for a known field.
fn parse_pcsx2_ini(text: &str, warnings: &mut Vec<String>) -> Pcsx2Settings {
    let mut settings = Pcsx2Settings::default();
    let mut section = String::new();
    for (index, raw) in text.lines().enumerate() {
        if index >= MAX_INI_LINES {
            warnings.push(format!(
                "INI parsing stopped at the {MAX_INI_LINES}-line limit"
            ));
            break;
        }
        if raw.len() > MAX_INI_LINE_BYTES {
            if warnings.len() < MAX_RETAINED_UNKNOWN_SETTINGS {
                warnings.push(format!(
                    "INI contains a line over {MAX_INI_LINE_BYTES} bytes"
                ));
            }
            continue;
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if let Some(value) = line.strip_suffix(']') {
                section = value[1..].to_string();
                if settings.controller_sections.len() < MAX_RETAINED_CONTROLLER_SECTIONS
                    && (section.to_ascii_lowercase().starts_with("pad")
                        || section.to_ascii_lowercase().starts_with("usb"))
                {
                    settings.controller_sections.push(section.clone());
                }
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            continue;
        }
        apply_pcsx2_setting(&mut settings, &section, key, value);
    }
    settings
}

fn apply_pcsx2_setting(settings: &mut Pcsx2Settings, section: &str, key: &str, value: &str) {
    let boolean = parse_ini_bool(value);
    match (section, key) {
        ("EmuCore/GS", "Renderer") => settings.renderer = value_or_none(value),
        ("EmuCore/GS", "upscale_multiplier" | "UpscaleMultiplier") => {
            settings.internal_resolution = value_or_none(value)
        }
        ("EmuCore/GS", "BiFilter") => settings.texture_filtering = value_or_none(value),
        ("EmuCore/GS", "MaxAnisotropy") => settings.anisotropic_filtering = value_or_none(value),
        ("EmuCore/GS", "deinterlace_mode" | "interlace") => {
            settings.deinterlacing = value_or_none(value)
        }
        ("EmuCore/GS", "VsyncEnable") => settings.vsync = boolean,
        ("EmuCore", "EnableWideScreenPatches") => settings.widescreen_patches_enabled = boolean,
        ("EmuCore", "EnableCheats") => settings.cheats_enabled = boolean,
        ("EmuCore", "EnablePatches") => settings.patches_enabled = boolean,
        ("EmuCore/GS", "LimitScalar" | "FramerateLimit") => {
            settings.frame_limiter = value_or_none(value)
        }
        ("EmuCore/Speedhacks", "EECycleRate") => settings.ee_cycle_rate = value_or_none(value),
        ("EmuCore/Speedhacks", "EECycleSkip") => settings.ee_cycle_skip = value_or_none(value),
        ("EmuCore/Speedhacks", "vuThread" | "MTVU") => settings.mtvu_enabled = boolean,
        ("SPU2/Output" | "SPU2", "Backend" | "output_module") => {
            settings.audio_backend = value_or_none(value)
        }
        ("EmuCore/GS", "LoadTextureReplacements") => settings.texture_replacement_enabled = boolean,
        _ if settings.unknown.len() < MAX_RETAINED_UNKNOWN_SETTINGS => {
            settings
                .unknown
                .insert(format!("{section}/{key}"), value.to_string());
        }
        _ => {}
    }
}

fn parse_ini_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn value_or_none(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn differing_settings_keys(global: &Pcsx2Settings, game: &Pcsx2Settings) -> Vec<String> {
    let global = flattened_settings(global);
    let game = flattened_settings(game);
    game.into_iter()
        .filter_map(|(key, value)| (global.get(&key) != Some(&value)).then_some(key))
        .collect()
}

fn flattened_settings(settings: &Pcsx2Settings) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    macro_rules! insert_opt {
        ($field:expr, $name:literal) => {
            if let Some(value) = &$field {
                map.insert($name.to_string(), value.to_string());
            }
        };
    }
    insert_opt!(settings.renderer, "renderer");
    insert_opt!(settings.internal_resolution, "internal_resolution");
    insert_opt!(settings.texture_filtering, "texture_filtering");
    insert_opt!(settings.anisotropic_filtering, "anisotropic_filtering");
    insert_opt!(settings.deinterlacing, "deinterlacing");
    insert_opt!(settings.frame_limiter, "frame_limiter");
    insert_opt!(settings.ee_cycle_rate, "ee_cycle_rate");
    insert_opt!(settings.ee_cycle_skip, "ee_cycle_skip");
    insert_opt!(settings.audio_backend, "audio_backend");
    if let Some(value) = settings.widescreen_patches_enabled {
        map.insert("widescreen_patches_enabled".to_string(), value.to_string());
    }
    if let Some(value) = settings.vsync {
        map.insert("vsync".to_string(), value.to_string());
    }
    if let Some(value) = settings.mtvu_enabled {
        map.insert("mtvu_enabled".to_string(), value.to_string());
    }
    for (key, value) in &settings.unknown {
        map.insert(format!("unknown/{key}"), value.clone());
    }
    map
}

// ---------------------------------------------------------------------
// Texture replacements
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pcsx2TextureInventory {
    pub path: PathBuf,
    pub present: bool,
    pub file_count: usize,
    pub total_size_bytes: u64,
    pub complete: bool,
    pub warnings: Vec<String>,
}

/// Bounded scan of `textures_root/<serial>` (real PCSX2 layout:
/// `<configuration_path>/textures/<SERIAL>`). Counts files and total size
/// only - never hashes or reads texture contents.
fn inspect_pcsx2_textures(textures_root: &Path, serial: &str) -> Pcsx2TextureInventory {
    let path = textures_root.join(serial);
    let mut warnings = Vec::new();
    let Ok(read_dir) = fs::read_dir(&path) else {
        return Pcsx2TextureInventory {
            path,
            present: false,
            file_count: 0,
            total_size_bytes: 0,
            complete: true,
            warnings,
        };
    };
    let mut file_count = 0usize;
    let mut total_size_bytes = 0u64;
    let mut complete = true;
    for (visited, entry) in read_dir.flatten().enumerate() {
        if visited >= PCSX2_MAX_ENTRIES_VISITED {
            complete = false;
            warnings.push(format!(
                "texture scan stopped at the {PCSX2_MAX_ENTRIES_VISITED}-entry limit"
            ));
            break;
        }
        if file_count >= PCSX2_MAX_TEXTURE_FILES {
            complete = false;
            warnings.push(format!(
                "texture scan stopped at the {PCSX2_MAX_TEXTURE_FILES}-file limit"
            ));
            break;
        }
        let entry_path = entry.path();
        if !is_regular_file_no_follow(&entry_path) {
            continue;
        }
        if let Ok(metadata) = fs::symlink_metadata(&entry_path) {
            file_count += 1;
            total_size_bytes = total_size_bytes.saturating_add(metadata.len());
        }
    }
    Pcsx2TextureInventory {
        present: true,
        path,
        file_count,
        total_size_bytes,
        complete,
        warnings,
    }
}

// ---------------------------------------------------------------------
// Memory cards
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2MemcardKind {
    /// A single memory-card image shared across every game - PCSX2's
    /// default arrangement. Never claimed to belong exclusively to one
    /// title.
    Shared,
    /// A per-title memory-card folder, keyed by serial - only reported
    /// when the caller has an actual serial to key by.
    PerGameFolder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2MemcardInfo {
    pub path: PathBuf,
    pub kind: Pcsx2MemcardKind,
    pub present: bool,
}

/// Bounded, conservative memory-card presence inspection under
/// `memcards_root` (real PCSX2 layout: `<configuration_path>/memcards`).
/// Every top-level `Mcd*.ps2`/`.bin` file is reported as `Shared` (PCSX2's
/// default arrangement makes no single-title claim possible); a
/// `<serial>/` subdirectory, when `serial` is supplied, is reported
/// separately as `PerGameFolder`.
fn inspect_pcsx2_memcards(memcards_root: &Path, serial: Option<&str>) -> Vec<Pcsx2MemcardInfo> {
    let mut cards = Vec::new();
    if let Ok(read_dir) = fs::read_dir(memcards_root) {
        for (visited, entry) in read_dir.flatten().enumerate() {
            if visited >= PCSX2_MAX_MEMCARD_CANDIDATES {
                break;
            }
            let entry_path = entry.path();
            if !is_regular_file_no_follow(&entry_path) {
                continue;
            }
            let looks_like_memcard = entry_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("ps2") || extension.eq_ignore_ascii_case("bin")
                });
            if looks_like_memcard {
                cards.push(Pcsx2MemcardInfo {
                    path: entry_path,
                    kind: Pcsx2MemcardKind::Shared,
                    present: true,
                });
            }
        }
    }
    if let Some(serial) = serial {
        let per_game_path = memcards_root.join(serial);
        cards.push(Pcsx2MemcardInfo {
            present: is_real_directory_no_follow(&per_game_path).unwrap_or(false),
            kind: Pcsx2MemcardKind::PerGameFolder,
            path: per_game_path,
        });
    }
    cards
}

// ---------------------------------------------------------------------
// Save states
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pcsx2SaveStateInventory {
    /// Save states whose filename starts with the requested serial - real
    /// PCSX2 save-state filenames are serial-prefixed
    /// (`SLUS-20312 (F460F374).00.p2s`), which is why this bounded
    /// prefix check is a genuinely reliable mapping rather than a filename
    /// guess. Still never authoritative for anything beyond "a save state
    /// exists for this serial."
    pub matched_count: usize,
    pub total_count_in_directory: usize,
    pub complete: bool,
}

fn inspect_pcsx2_savestates(sstates_root: &Path, serial: Option<&str>) -> Pcsx2SaveStateInventory {
    let mut inventory = Pcsx2SaveStateInventory {
        complete: true,
        ..Default::default()
    };
    let Ok(read_dir) = fs::read_dir(sstates_root) else {
        return inventory;
    };
    for (visited, entry) in read_dir.flatten().enumerate() {
        if visited >= PCSX2_MAX_SAVESTATE_CANDIDATES {
            inventory.complete = false;
            break;
        }
        let entry_path = entry.path();
        if !is_regular_file_no_follow(&entry_path) {
            continue;
        }
        let is_savestate = entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("p2s"));
        if !is_savestate {
            continue;
        }
        inventory.total_count_in_directory += 1;
        if let Some(serial) = serial
            && entry_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(serial))
        {
            inventory.matched_count += 1;
        }
    }
    inventory
}

// ---------------------------------------------------------------------
// Controllers
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pcsx2ControllerInfo {
    pub profile_configured: bool,
    /// Section names observed (e.g. `Pad1`, `USB1`), bounded - device/
    /// profile *names* are not reliably present in PCSX2.ini itself, so
    /// this reports which ports have any configuration at all, not device
    /// identity.
    pub configured_sections: Vec<String>,
}

fn controller_info_from(settings: &Pcsx2Settings) -> Pcsx2ControllerInfo {
    Pcsx2ControllerInfo {
        profile_configured: !settings.controller_sections.is_empty(),
        configured_sections: settings.controller_sections.clone(),
    }
}

// ---------------------------------------------------------------------
// Selected-title mapping and top-level inspection
// ---------------------------------------------------------------------

/// A deliberately separate input lane for identity supplied by core and
/// identifiers merely observed in PCSX2 context - mirrors
/// [`super::ppsspp_local::PpssppGameRequest`]/
/// [`super::rpcs3_local::Rpcs3GameRequest`] exactly. `verified_ps2_serial`
/// keys the serial-addressed local directories this section adds
/// (per-game config/textures/memory cards/save states);
/// `verified_executable_crc` is passed straight through to the existing,
/// unchanged [`match_pcsx2_inventory`] for PNACH matching - the two are
/// never conflated (see this section's own doc comment for why).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pcsx2GameRequest {
    pub verified_ps2_serial: Option<String>,
    pub verified_executable_crc: Option<String>,
    pub emulator_serial: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2SerialMapping {
    VerifiedPs2Serial,
    EmulatorMetadataOnly,
    Unavailable,
}

fn select_serial(request: &Pcsx2GameRequest) -> (Option<String>, Pcsx2SerialMapping) {
    if let Some(serial) = request
        .verified_ps2_serial
        .as_deref()
        .and_then(normalize_ps2_serial)
    {
        return (Some(serial), Pcsx2SerialMapping::VerifiedPs2Serial);
    }
    if let Some(serial) = request
        .emulator_serial
        .as_deref()
        .and_then(normalize_ps2_serial)
    {
        return (Some(serial), Pcsx2SerialMapping::EmulatorMetadataOnly);
    }
    (None, Pcsx2SerialMapping::Unavailable)
}

/// A light shape check only (uppercase alphanumeric plus `-`), not a
/// re-implementation of serial *extraction* - the authoritative grammar
/// for turning a boot path into a serial remains
/// [`crate::game_identity::serial_from_boot_path`], which this module
/// never re-derives (see this section's own doc comment).
fn normalize_ps2_serial(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()
        && trimmed.len() <= 16
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    .then(|| trimmed.to_ascii_uppercase())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2Health {
    pub detected: bool,
    pub config_readable: bool,
    pub bios: Pcsx2BiosVerification,
    pub patch_data_available: bool,
    pub serial_mapping: Pcsx2SerialMapping,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2GameInspection {
    pub serial: Option<String>,
    pub serial_mapping: Pcsx2SerialMapping,
    pub global_config: Pcsx2Config,
    pub per_game_config: Option<Pcsx2Config>,
    pub overridden_setting_keys: Vec<String>,
    /// The existing, unchanged PNACH inventory for this profile - see
    /// [`inspect_pcsx2_profile`], reused verbatim.
    pub patches: Option<Pcsx2PnachInventory>,
    /// The existing, unchanged CRC-based match result - see
    /// [`match_pcsx2_inventory`], reused verbatim.
    pub patch_match: Option<Pcsx2MatchResult>,
    pub textures: Option<Pcsx2TextureInventory>,
    pub memcards: Vec<Pcsx2MemcardInfo>,
    pub savestates: Pcsx2SaveStateInventory,
    pub controllers: Pcsx2ControllerInfo,
    pub bios: Pcsx2BiosInfo,
    pub health: Pcsx2Health,
}

/// Ties every new inspection function above together into one summary for
/// a selected, possibly-verified PS2 title - the same shape
/// `ppsspp_local::inspect_ppsspp_game`/`rpcs3_local::inspect_rpcs3_game`
/// already establish. Infallible: an internal failure of the existing
/// [`inspect_pcsx2_profile`] (e.g. the profile changed underfoot) is
/// recorded as a health warning rather than propagated, so a caller
/// always gets a populated summary to render.
pub fn inspect_pcsx2_game(
    profile: &Pcsx2Profile,
    request: &Pcsx2GameRequest,
) -> Pcsx2GameInspection {
    let (serial, serial_mapping) = select_serial(request);
    let mut health_warnings: Vec<String> = profile
        .blockers
        .iter()
        .map(|blocker| blocker.detail.clone())
        .collect();

    let global_config =
        inspect_pcsx2_config(&pcsx2_global_config_path(&profile.configuration_path));
    for warning in &global_config.warnings {
        health_warnings.push(warning.clone());
    }
    let per_game_config = serial.as_deref().map(|serial| {
        let modern = profile
            .configuration_path
            .join(format!("inis/gamesettings/{serial}.ini"));
        let legacy = profile
            .configuration_path
            .join(format!("gamesettings/{serial}.ini"));
        inspect_pcsx2_config(if is_regular_file_no_follow(&modern) {
            &modern
        } else {
            &legacy
        })
    });
    let overridden_setting_keys = per_game_config
        .as_ref()
        .map(|config| differing_settings_keys(&global_config.settings, &config.settings))
        .unwrap_or_default();

    let patches = match inspect_pcsx2_profile(profile) {
        Ok(inventory) => Some(inventory),
        Err(error) => {
            health_warnings.push(error.to_string());
            None
        }
    };
    let patch_match = patches.as_ref().map(|inventory| {
        match_pcsx2_inventory(inventory, request.verified_executable_crc.as_deref(), None)
    });

    let textures = serial
        .as_deref()
        .map(|serial| inspect_pcsx2_textures(&profile.configuration_path.join("textures"), serial));
    let memcards = inspect_pcsx2_memcards(
        &profile.configuration_path.join("memcards"),
        serial.as_deref(),
    );
    let savestates = inspect_pcsx2_savestates(
        &profile.configuration_path.join("sstates"),
        serial.as_deref(),
    );
    let controllers = controller_info_from(&global_config.settings);
    let bios = inspect_pcsx2_bios(&profile.configuration_path.join("bios"));
    for warning in &bios.warnings {
        health_warnings.push(warning.clone());
    }

    let health = Pcsx2Health {
        detected: profile.eligible,
        config_readable: global_config.readable,
        bios: bios.verification,
        patch_data_available: patches
            .as_ref()
            .is_some_and(|inventory| !inventory.files.is_empty()),
        serial_mapping,
        warnings: health_warnings,
    };

    Pcsx2GameInspection {
        serial,
        serial_mapping,
        global_config,
        per_game_config,
        overridden_setting_keys,
        patches,
        patch_match,
        textures,
        memcards,
        savestates,
        controllers,
        bios,
        health,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "archivefs-pcsx2-local-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn roots(root: &Path) -> Pcsx2ProfileDiscoveryRoots {
        Pcsx2ProfileDiscoveryRoots {
            home: root.join("home"),
            xdg_config_home: root.join("config"),
            xdg_data_home: root.join("data"),
            documents_home: root.join("home/Documents"),
            flatpak_system_root: root.join("system-flatpak"),
            appimage_directory: None,
            portable_configuration_roots: Vec::new(),
        }
    }

    fn make_profile(root: &Path) -> PathBuf {
        fs::create_dir_all(root.join("inis")).unwrap();
        fs::create_dir_all(root.join("cheats")).unwrap();
        root.to_path_buf()
    }

    fn eligible_profile(root: &Path) -> Pcsx2Profile {
        let mut discovery_roots = roots(root.parent().unwrap());
        discovery_roots.portable_configuration_roots = vec![root.to_path_buf()];
        discover_pcsx2_profiles(&discovery_roots)
            .unwrap()
            .profiles
            .into_iter()
            .find(|profile| profile.configuration_path == root)
            .unwrap()
    }

    #[test]
    fn discovers_native_and_flatpak_user_profiles() {
        let root = fixture_root("discovery");
        make_profile(&root.join("config/PCSX2"));
        make_profile(&root.join("home/.var/app/net.pcsx2.PCSX2/config/PCSX2"));
        fs::create_dir_all(root.join("data/flatpak/app/net.pcsx2.PCSX2")).unwrap();
        let discovery = discover_pcsx2_profiles(&roots(&root)).unwrap();
        assert_eq!(discovery.profiles.len(), 2);
        assert!(discovery.profiles.iter().all(|profile| profile.eligible));
        assert!(
            discovery
                .profiles
                .iter()
                .any(|profile| profile.installation_type == Pcsx2InstallationType::Native)
        );
        assert!(
            discovery
                .profiles
                .iter()
                .any(|profile| { profile.installation_type == Pcsx2InstallationType::FlatpakUser })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovers_appimage_adjacent_portable_profile() {
        let root = fixture_root("appimage-portable");
        make_profile(&root.join("appimage-dir"));
        fs::write(root.join("appimage-dir/portable.ini"), b"").unwrap();
        let mut discovery_roots = roots(&root);
        discovery_roots.appimage_directory = Some(root.join("appimage-dir"));
        let discovery = discover_pcsx2_profiles(&discovery_roots).unwrap();
        let profile = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_path == root.join("appimage-dir"))
            .expect("appimage-adjacent portable profile discovered");
        assert!(profile.eligible);
        assert_eq!(profile.installation_type, Pcsx2InstallationType::Portable);
        assert_eq!(profile.scope, Pcsx2ProfileScope::Portable);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn appimage_directory_without_pcsx2_evidence_is_blocked_not_guessed() {
        let root = fixture_root("appimage-no-evidence");
        fs::create_dir_all(root.join("appimage-dir")).unwrap();
        let mut discovery_roots = roots(&root);
        discovery_roots.appimage_directory = Some(root.join("appimage-dir"));
        let discovery = discover_pcsx2_profiles(&discovery_roots).unwrap();
        let profile = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_path == root.join("appimage-dir"))
            .expect("directory beside the AppImage is reported, even though it is not eligible");
        assert!(!profile.eligible);
        assert_eq!(
            profile.blockers[0].kind,
            Pcsx2ProfileBlockerKind::MissingPcsx2Evidence
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_appimage_directory_is_silently_absent() {
        let root = fixture_root("appimage-missing-dir");
        fs::create_dir_all(&root).unwrap();
        let mut discovery_roots = roots(&root);
        discovery_roots.appimage_directory = Some(root.join("does-not-exist"));
        let discovery = discover_pcsx2_profiles(&discovery_roots).unwrap();
        assert!(
            discovery
                .profiles
                .iter()
                .all(|profile| profile.configuration_path != root.join("does-not-exist"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovers_native_alternate_data_home_and_documents_profiles() {
        let root = fixture_root("native-alternate");
        make_profile(&root.join("data/PCSX2"));
        make_profile(&root.join("home/Documents/PCSX2"));
        let discovery = discover_pcsx2_profiles(&roots(&root)).unwrap();
        let alternates: Vec<_> = discovery
            .profiles
            .iter()
            .filter(|profile| profile.installation_type == Pcsx2InstallationType::NativeAlternate)
            .collect();
        assert_eq!(alternates.len(), 2);
        assert!(alternates.iter().all(|profile| profile.eligible));
        assert!(
            alternates
                .iter()
                .any(|profile| profile.configuration_path == root.join("data/PCSX2"))
        );
        assert!(
            alternates
                .iter()
                .any(|profile| profile.configuration_path == root.join("home/Documents/PCSX2"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multiple_discovered_profiles_require_explicit_confirmation_not_a_guess() {
        use super::super::pcsx2_identity::{Pcsx2ProfileChoiceError, confirmed_pcsx2_profile};

        let root = fixture_root("ambiguous-discovery");
        make_profile(&root.join("config/PCSX2"));
        make_profile(&root.join("appimage-dir"));
        let mut discovery_roots = roots(&root);
        discovery_roots.appimage_directory = Some(root.join("appimage-dir"));
        let discovery = discover_pcsx2_profiles(&discovery_roots).unwrap();
        assert_eq!(discovery.profiles.iter().filter(|p| p.eligible).count(), 2);
        let choice = confirmed_pcsx2_profile(&discovery, None);
        match choice {
            Err(Pcsx2ProfileChoiceError::ConfirmationRequired {
                eligible_profile_ids,
            }) => assert_eq!(eligible_profile_ids.len(), 2),
            other => panic!("expected ConfirmationRequired, got {other:?}"),
        }
        let native_id = discovery
            .profiles
            .iter()
            .find(|profile| profile.installation_type == Pcsx2InstallationType::Native)
            .unwrap()
            .profile_id
            .clone();
        assert_eq!(
            confirmed_pcsx2_profile(&discovery, Some(&native_id))
                .unwrap()
                .profile_id,
            native_id
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_system_flatpak_installation_scope() {
        let root = fixture_root("system-flatpak");
        make_profile(&root.join("home/.var/app/net.pcsx2.PCSX2/config/PCSX2"));
        fs::create_dir_all(root.join("system-flatpak/app/net.pcsx2.PCSX2")).unwrap();
        let profile = discover_pcsx2_profiles(&roots(&root))
            .unwrap()
            .profiles
            .pop()
            .unwrap();
        assert_eq!(
            profile.installation_type,
            Pcsx2InstallationType::FlatpakSystem
        );
        assert_eq!(
            profile.scope,
            Pcsx2ProfileScope::SystemInstallationUserProfile
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_directories_are_not_created_and_missing_portable_is_blocked() {
        let root = fixture_root("missing");
        fs::create_dir_all(&root).unwrap();
        let missing = root.join("portable");
        let mut discovery_roots = roots(&root);
        discovery_roots
            .portable_configuration_roots
            .push(missing.clone());
        let discovery = discover_pcsx2_profiles(&discovery_roots).unwrap();
        assert_eq!(discovery.profiles.len(), 1);
        assert!(!discovery.profiles[0].eligible);
        assert_eq!(
            discovery.profiles[0].blockers[0].kind,
            Pcsx2ProfileBlockerKind::MissingConfiguration
        );
        assert!(!missing.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_profile_and_cheat_directory_are_refused() {
        use std::os::unix::fs::symlink;
        let root = fixture_root("symlink");
        make_profile(&root.join("real"));
        fs::create_dir_all(root.join("container")).unwrap();
        symlink(root.join("real"), root.join("container/profile")).unwrap();
        let mut discovery_roots = roots(&root);
        discovery_roots.portable_configuration_roots = vec![root.join("container/profile")];
        let discovery = discover_pcsx2_profiles(&discovery_roots).unwrap();
        assert!(!discovery.profiles[0].eligible);

        let second = make_profile(&root.join("second"));
        fs::remove_dir_all(second.join("cheats")).unwrap();
        symlink(root.join("real/cheats"), second.join("cheats")).unwrap();
        discovery_roots.portable_configuration_roots = vec![second.clone()];
        let profile = discover_pcsx2_profiles(&discovery_roots)
            .unwrap()
            .profiles
            .pop()
            .unwrap();
        assert!(profile.eligible);
        assert_eq!(
            profile.patch_directories[0].state,
            Pcsx2PatchDirectoryState::UnsafePath
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_crc_metadata_and_categories_without_writing() {
        let root = fixture_root("parse");
        let profile_root = make_profile(&root.join("portable"));
        fs::create_dir_all(profile_root.join("cheats_ws")).unwrap();
        fs::write(
            profile_root.join("cheats/DEADBEEF.pnach"),
            b"gametitle=Example Game\nregion=PAL\ncomment=Owner note\npatch=1,EE,00100000,word,00000001\npatch=0,EE,00100004,word,00000002\n",
        )
        .unwrap();
        fs::write(
            profile_root.join("cheats_ws/CAFEBABE.pnach"),
            b"patch=1,EE,00100000,word,00000001\n",
        )
        .unwrap();
        let before = fs::read(profile_root.join("cheats/DEADBEEF.pnach")).unwrap();
        let inventory = inspect_pcsx2_profile(&eligible_profile(&profile_root)).unwrap();
        assert_eq!(inventory.files.len(), 2);
        assert_eq!(
            inventory.files[0].crc_candidate.as_deref(),
            Some("DEADBEEF")
        );
        assert_eq!(inventory.files[0].enabled_patch_count, 1);
        assert_eq!(inventory.files[0].disabled_patch_count, 1);
        assert_eq!(inventory.files[0].comments, vec!["Owner note"]);
        assert!(
            inventory
                .files
                .iter()
                .any(|file| { file.category == Pcsx2PatchCategory::WidescreenPatches })
        );
        assert_eq!(
            before,
            fs::read(profile_root.join("cheats/DEADBEEF.pnach")).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_and_oversized_pnach_files_are_reported() {
        let root = fixture_root("limits");
        let profile_root = make_profile(&root.join("portable"));
        fs::write(profile_root.join("cheats/BAD.pnach"), b"patch=maybe\n").unwrap();
        let mut oversized = File::create(profile_root.join("cheats/TOOBIG.pnach")).unwrap();
        oversized
            .write_all(&vec![b'x'; PCSX2_MAX_PNACH_FILE_BYTES as usize + 1])
            .unwrap();
        let inventory = inspect_pcsx2_profile(&eligible_profile(&profile_root)).unwrap();
        assert_eq!(inventory.files.len(), 1);
        assert!(
            inventory.files[0]
                .warnings
                .contains(&Pcsx2InspectionWarningKind::MalformedPnach)
        );
        assert!(
            inventory
                .warnings
                .iter()
                .any(|warning| { warning.kind == Pcsx2InspectionWarningKind::FileTooLarge })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicates_and_match_confidence_are_explicit() {
        let root = fixture_root("matches");
        let profile_root = make_profile(&root.join("portable"));
        fs::create_dir_all(profile_root.join("cheats_ws")).unwrap();
        let body = b"gametitle=Example Game\npatch=1,EE,0,word,1\n";
        fs::write(profile_root.join("cheats/DEADBEEF.pnach"), body).unwrap();
        fs::write(profile_root.join("cheats_ws/DEADBEEF.pnach"), body).unwrap();
        let inventory = inspect_pcsx2_profile(&eligible_profile(&profile_root)).unwrap();
        assert!(inventory.files.iter().all(|file| file.duplicate_crc));
        assert!(inventory.files.iter().all(|file| file.duplicate_filename));
        assert!(inventory.files.iter().all(|file| file.duplicate_content));
        assert_eq!(
            match_pcsx2_inventory(&inventory, Some("DEADBEEF"), None).state,
            Pcsx2MatchState::MultiplePnachFilesForSameCrc
        );
        assert_eq!(
            match_pcsx2_inventory(&inventory, None, Some("Example Game")).state,
            Pcsx2MatchState::CandidateByFilenameOrTitleOnly
        );
        assert_eq!(
            match_pcsx2_inventory(&inventory, None, Some("Different")).state,
            Pcsx2MatchState::NoVerifiedGameCrcAvailable
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_crc_filename_is_not_identity_evidence() {
        let root = fixture_root("invalid-crc");
        let profile_root = make_profile(&root.join("portable"));
        fs::write(profile_root.join("cheats/NOT-A-CRC.pnach"), b"patch=1,x\n").unwrap();
        let inventory = inspect_pcsx2_profile(&eligible_profile(&profile_root)).unwrap();
        assert_eq!(inventory.files[0].crc_candidate, None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn relative_and_filesystem_root_profiles_are_blocked_without_inspection() {
        let root = fixture_root("unsafe-roots");
        fs::create_dir_all(&root).unwrap();
        let mut discovery_roots = roots(&root);
        discovery_roots.portable_configuration_roots =
            vec![PathBuf::from("relative"), PathBuf::from("/")];
        let discovery = discover_pcsx2_profiles(&discovery_roots).unwrap();
        assert_eq!(discovery.profiles.len(), 2);
        assert!(discovery.profiles.iter().all(|profile| !profile.eligible));
        assert!(discovery.profiles.iter().any(|profile| {
            profile.blockers[0].kind == Pcsx2ProfileBlockerKind::PathNotAbsolute
        }));
        assert!(discovery.profiles.iter().any(|profile| {
            profile.blockers[0].kind == Pcsx2ProfileBlockerKind::FilesystemRoot
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_count_and_depth_limits_are_deterministic() {
        let root = fixture_root("bounded");
        let profile_root = make_profile(&root.join("portable"));
        fs::write(profile_root.join("cheats/00000001.pnach"), b"patch=1,x\n").unwrap();
        fs::write(profile_root.join("cheats/00000002.pnach"), b"patch=1,x\n").unwrap();
        fs::write(profile_root.join("cheats/00000003.pnach"), b"patch=1,x\n").unwrap();
        fs::create_dir_all(profile_root.join("cheats/a/b")).unwrap();
        fs::write(
            profile_root.join("cheats/a/b/00000004.pnach"),
            b"patch=1,x\n",
        )
        .unwrap();
        let profile = eligible_profile(&profile_root);
        let inventory = inspect_pcsx2_profile_with_limits(&profile, 2, 1).unwrap();
        assert_eq!(inventory.files.len(), 2);
        assert!(!inventory.complete);
        assert!(
            inventory.warnings.iter().any(|warning| {
                warning.kind == Pcsx2InspectionWarningKind::FileCountLimitReached
            })
        );
        assert!(
            inventory
                .warnings
                .iter()
                .any(|warning| { warning.kind == Pcsx2InspectionWarningKind::DepthLimitReached })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn changed_profile_identity_is_rejected_before_file_inspection() {
        let root = fixture_root("identity-change");
        let profile_root = make_profile(&root.join("portable"));
        let profile = eligible_profile(&profile_root);
        fs::rename(&profile_root, root.join("old-profile")).unwrap();
        make_profile(&profile_root);
        let error = inspect_pcsx2_profile(&profile).unwrap_err();
        assert!(matches!(error, Pcsx2InspectionError::ProfileChanged { .. }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_no_match_and_invalid_crc_states_require_verified_input() {
        let root = fixture_root("exact");
        let profile_root = make_profile(&root.join("portable"));
        fs::write(profile_root.join("cheats/DEADBEEF.pnach"), b"patch=1,x\n").unwrap();
        let inventory = inspect_pcsx2_profile(&eligible_profile(&profile_root)).unwrap();
        assert_eq!(
            match_pcsx2_inventory(&inventory, Some("deadbeef"), None).state,
            Pcsx2MatchState::ExactCrcMatch
        );
        assert_eq!(
            match_pcsx2_inventory(&inventory, Some("CAFEBABE"), None).state,
            Pcsx2MatchState::NoMatchingPnachFound
        );
        assert_eq!(
            match_pcsx2_inventory(&inventory, Some("not-a-crc"), None).state,
            Pcsx2MatchState::InvalidVerifiedGameCrc
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_profile_and_pnach_paths_remain_inspectable_by_exact_os_identity() {
        use std::os::unix::ffi::OsStringExt;
        let root = fixture_root("non-utf8");
        let profile_name = OsString::from_vec(b"PCSX2-\xff".to_vec());
        let profile_root = make_profile(&root.join(profile_name));
        let pnach_name = OsString::from_vec(b"DEADBEEF-\xfe.pnach".to_vec());
        fs::write(
            profile_root.join("cheats").join(&pnach_name),
            b"patch=1,x\n",
        )
        .unwrap();
        let profile = eligible_profile(&profile_root);
        let inventory = inspect_pcsx2_profile(&profile).unwrap();
        assert_eq!(inventory.files.len(), 1);
        assert_eq!(
            inventory.files[0].path.file_name(),
            Some(pnach_name.as_os_str())
        );
        assert!(profile.profile_id.starts_with("pcsx2-portable-"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_special_pnach_entries_are_reported_without_being_opened() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;
        let root = fixture_root("special");
        let profile_root = make_profile(&root.join("portable"));
        fs::write(root.join("outside.pnach"), b"patch=1,x\n").unwrap();
        symlink(
            root.join("outside.pnach"),
            profile_root.join("cheats/link.pnach"),
        )
        .unwrap();
        let socket_path = profile_root.join("cheats/socket.pnach");
        let _listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("Unix socket creation is not permitted in this test environment");
                fs::remove_dir_all(root).unwrap();
                return;
            }
            Err(error) => panic!("failed to create special-file fixture: {error}"),
        };
        let inventory = inspect_pcsx2_profile(&eligible_profile(&profile_root)).unwrap();
        assert!(inventory.files.is_empty());
        assert!(
            inventory
                .warnings
                .iter()
                .any(|warning| { warning.kind == Pcsx2InspectionWarningKind::SymlinkSkipped })
        );
        assert!(
            inventory
                .warnings
                .iter()
                .any(|warning| { warning.kind == Pcsx2InspectionWarningKind::SpecialFileSkipped })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn line_count_and_line_length_limits_refuse_unbounded_metadata() {
        let root = fixture_root("line-limits");
        let profile_root = make_profile(&root.join("portable"));
        fs::write(
            profile_root.join("cheats/LONGLINE.pnach"),
            vec![b'x'; PCSX2_MAX_LINE_BYTES + 1],
        )
        .unwrap();
        let many_lines = "\n".repeat(PCSX2_MAX_LINES_PER_FILE + 1);
        fs::write(profile_root.join("cheats/MANYLINES.pnach"), many_lines).unwrap();
        let inventory = inspect_pcsx2_profile(&eligible_profile(&profile_root)).unwrap();
        assert!(inventory.files.is_empty());
        assert!(
            inventory
                .warnings
                .iter()
                .any(|warning| { warning.kind == Pcsx2InspectionWarningKind::LineTooLong })
        );
        assert!(
            inventory.warnings.iter().any(|warning| {
                warning.kind == Pcsx2InspectionWarningKind::LineCountLimitReached
            })
        );
        fs::remove_dir_all(root).unwrap();
    }

    // ===================================================================
    // Emulator Adapter Refresh Batch H: modernisation tests.
    // ===================================================================

    fn write_global_config(profile_root: &Path, contents: &str) {
        fs::create_dir_all(profile_root.join("inis")).unwrap();
        fs::write(profile_root.join("inis/PCSX2.ini"), contents).unwrap();
    }

    // -- version ---------------------------------------------------------

    #[test]
    fn known_version_string_parses() {
        assert_eq!(
            parse_pcsx2_version("PCSX2 2.2.0-20250101120000"),
            Some("2.2.0".to_string())
        );
        assert_eq!(parse_pcsx2_version("v1.7.5"), Some("1.7.5".to_string()));
    }

    #[test]
    fn unknown_version_shape_fails_soft() {
        assert_eq!(parse_pcsx2_version("some unrelated tool"), None);
        assert_eq!(parse_pcsx2_version(""), None);
    }

    // -- global config -----------------------------------------------------

    #[test]
    fn valid_global_config_is_parsed() {
        let root = fixture_root("global-config");
        let profile_root = make_profile(&root);
        write_global_config(
            &profile_root,
            "[EmuCore/GS]\nRenderer = 12\nVsyncEnable = true\nLoadTextureReplacements = false\n\
             [EmuCore]\nEnableCheats = true\nEnableWideScreenPatches = false\n",
        );
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert!(inspection.global_config.readable);
        assert_eq!(
            inspection.global_config.settings.renderer.as_deref(),
            Some("12")
        );
        assert_eq!(inspection.global_config.settings.vsync, Some(true));
        assert_eq!(inspection.global_config.settings.cheats_enabled, Some(true));
        assert_eq!(
            inspection.global_config.settings.widescreen_patches_enabled,
            Some(false)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_global_config_fails_soft() {
        let root = fixture_root("malformed-config");
        let profile_root = make_profile(&root);
        write_global_config(
            &profile_root,
            "[EmuCore/GS]\nthis line has no equals sign\nVsyncEnable = true\n",
        );
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert!(inspection.global_config.readable);
        assert_eq!(inspection.global_config.settings.vsync, Some(true));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_global_config_is_not_readable_but_never_panics() {
        let root = fixture_root("missing-config");
        let profile_root = make_profile(&root);
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert!(!inspection.global_config.readable);
        fs::remove_dir_all(root).unwrap();
    }

    // -- BIOS --------------------------------------------------------------

    #[test]
    fn bios_present_is_unverified_never_verified_by_filename_alone() {
        let root = fixture_root("bios-present");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("bios")).unwrap();
        fs::write(profile_root.join("bios/SCPH-70012.bin"), b"not a real bios").unwrap();
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert_eq!(
            inspection.bios.verification,
            Pcsx2BiosVerification::PresentUnverified
        );
        assert_eq!(
            inspection.health.bios,
            Pcsx2BiosVerification::PresentUnverified
        );
        assert_eq!(inspection.bios.filename_hint.as_deref(), Some("SCPH-70012"));
    }

    #[test]
    fn bios_missing_is_reported_honestly() {
        let root = fixture_root("bios-missing");
        let profile_root = make_profile(&root);
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert_eq!(inspection.bios.verification, Pcsx2BiosVerification::Missing);
    }

    // -- serial mapping / identity safety -----------------------------------

    #[test]
    fn authoritative_ps2_serial_maps_per_game_assets() {
        let root = fixture_root("verified-serial");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("inis/gamesettings")).unwrap();
        fs::write(
            profile_root.join("inis/gamesettings/SLUS-20312.ini"),
            "[EmuCore/GS]\nRenderer = 14\n",
        )
        .unwrap();
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: Some("SLUS-20312".to_string()),
            verified_executable_crc: None,
            emulator_serial: Some("SLES-99999".to_string()),
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        assert_eq!(
            inspection.serial_mapping,
            Pcsx2SerialMapping::VerifiedPs2Serial
        );
        assert_eq!(inspection.serial.as_deref(), Some("SLUS-20312"));
        let per_game = inspection.per_game_config.expect("per-game config found");
        assert_eq!(
            per_game.settings.renderer.as_deref(),
            Some("14"),
            "verified serial must select assets, never the emulator-observed one"
        );
    }

    #[test]
    fn unresolved_identity_stays_unresolved_no_asset_mapping() {
        let root = fixture_root("unresolved-identity");
        let profile_root = make_profile(&root);
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert_eq!(inspection.serial_mapping, Pcsx2SerialMapping::Unavailable);
        assert!(inspection.serial.is_none());
        assert!(inspection.per_game_config.is_none());
    }

    #[test]
    fn emulator_metadata_only_never_overrides_a_conflicting_verified_identity() {
        // A caller whose own identity resolution conflicted must simply not
        // pass a `verified_ps2_serial` - this module never chooses a side
        // or falls back to emulator metadata to break a tie it was never
        // told about.
        let root = fixture_root("conflict");
        let profile_root = make_profile(&root);
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: None,
            verified_executable_crc: None,
            emulator_serial: Some("SLUS-99999".to_string()),
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        assert_eq!(
            inspection.serial_mapping,
            Pcsx2SerialMapping::EmulatorMetadataOnly
        );
        // Emulator-metadata-only mapping is explicitly weaker than
        // verified - a caller that knows identity conflicted is expected
        // to pass neither field, which the previous test already covers.
        assert_eq!(inspection.serial.as_deref(), Some("SLUS-99999"));
    }

    #[test]
    fn pcsx2_crc_never_becomes_preservation_identity() {
        // `Pcsx2GameRequest.verified_executable_crc` only ever feeds the
        // existing, unchanged `match_pcsx2_inventory` - it must never
        // influence `serial`/`serial_mapping` at all.
        let root = fixture_root("crc-metadata-only");
        let profile_root = make_profile(&root);
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: None,
            verified_executable_crc: Some("F460F374".to_string()),
            emulator_serial: None,
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        assert_eq!(inspection.serial_mapping, Pcsx2SerialMapping::Unavailable);
        assert!(inspection.serial.is_none());
    }

    #[test]
    fn directory_name_has_zero_identity_authority() {
        let root = fixture_root("dir-name-authority");
        let profile_root = make_profile(&root);
        // A per-game-settings file named after a *different* serial than
        // the one actually requested must never be picked up.
        fs::create_dir_all(profile_root.join("inis/gamesettings")).unwrap();
        fs::write(
            profile_root.join("inis/gamesettings/SLES-99999.ini"),
            "[EmuCore/GS]\nRenderer = 99\n",
        )
        .unwrap();
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: Some("SLUS-20312".to_string()),
            verified_executable_crc: None,
            emulator_serial: None,
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        let per_game = inspection
            .per_game_config
            .expect("per-game config lookup is always attempted for a known serial");
        assert!(
            !per_game.exists,
            "a per-game file named after a different serial must never be substituted"
        );
    }

    #[test]
    fn bios_filename_never_verifies_bios() {
        // Already covered by `bios_present_is_unverified_never_verified_by_filename_alone`
        // structurally (the only success path is `PresentUnverified`) -
        // this test additionally proves a filename matching a real,
        // well-known region string still never reaches `Verified`.
        let root = fixture_root("bios-filename-authority");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("bios")).unwrap();
        fs::write(profile_root.join("bios/SCPH-70012_verified.bin"), b"x").unwrap();
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert_ne!(
            inspection.bios.verification,
            Pcsx2BiosVerification::Verified
        );
    }

    // -- textures ------------------------------------------------------------

    #[test]
    fn texture_replacement_pack_is_detected_bounded() {
        let root = fixture_root("textures");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("textures/SLUS-20312")).unwrap();
        for index in 0..5 {
            fs::write(
                profile_root.join(format!("textures/SLUS-20312/tex{index}.png")),
                b"fake texture data",
            )
            .unwrap();
        }
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: Some("SLUS-20312".to_string()),
            verified_executable_crc: None,
            emulator_serial: None,
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        let textures = inspection.textures.expect("texture inventory present");
        assert!(textures.present);
        assert_eq!(textures.file_count, 5);
        assert!(textures.total_size_bytes > 0);
    }

    #[test]
    fn texture_directory_name_has_zero_identity_authority() {
        // The directory is keyed strictly by the *requested* serial as a
        // path component - a directory named after a different serial is
        // simply never visited.
        let root = fixture_root("textures-authority");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("textures/SLES-99999")).unwrap();
        fs::write(profile_root.join("textures/SLES-99999/tex0.png"), b"x").unwrap();
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: Some("SLUS-20312".to_string()),
            verified_executable_crc: None,
            emulator_serial: None,
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        let textures = inspection.textures.expect("texture inventory present");
        assert!(!textures.present);
        assert_eq!(textures.file_count, 0);
    }

    // -- memory cards ----------------------------------------------------

    #[test]
    fn shared_memory_card_is_detected_never_claimed_exclusive() {
        let root = fixture_root("memcard-shared");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("memcards")).unwrap();
        fs::write(profile_root.join("memcards/Mcd001.ps2"), b"x").unwrap();
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert!(
            inspection
                .memcards
                .iter()
                .any(|card| card.kind == Pcsx2MemcardKind::Shared && card.present)
        );
    }

    #[test]
    fn per_game_memory_card_folder_is_surfaced_when_present() {
        let root = fixture_root("memcard-per-game");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("memcards/SLUS-20312")).unwrap();
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: Some("SLUS-20312".to_string()),
            verified_executable_crc: None,
            emulator_serial: None,
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        assert!(
            inspection
                .memcards
                .iter()
                .any(|card| card.kind == Pcsx2MemcardKind::PerGameFolder && card.present)
        );
    }

    #[test]
    fn memory_card_filename_has_zero_identity_authority() {
        // A memory-card file's own name is never treated as evidence of
        // which title it belongs to - it is only ever reported as `Shared`.
        let root = fixture_root("memcard-authority");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("memcards")).unwrap();
        fs::write(profile_root.join("memcards/SLUS-20312.ps2"), b"x").unwrap();
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        let card = inspection
            .memcards
            .iter()
            .find(|card| card.path.ends_with("SLUS-20312.ps2"))
            .expect("card found");
        assert_eq!(card.kind, Pcsx2MemcardKind::Shared);
    }

    // -- save states -------------------------------------------------------

    #[test]
    fn save_state_matching_the_serial_prefix_is_counted() {
        let root = fixture_root("savestate");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("sstates")).unwrap();
        fs::write(
            profile_root.join("sstates/SLUS-20312 (F460F374).00.p2s"),
            b"x",
        )
        .unwrap();
        fs::write(
            profile_root.join("sstates/SLES-99999 (12345678).00.p2s"),
            b"x",
        )
        .unwrap();
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: Some("SLUS-20312".to_string()),
            verified_executable_crc: None,
            emulator_serial: None,
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        assert_eq!(inspection.savestates.matched_count, 1);
        assert_eq!(inspection.savestates.total_count_in_directory, 2);
    }

    // -- controllers -------------------------------------------------------

    #[test]
    fn controller_profile_presence_is_detected() {
        let root = fixture_root("controller");
        let profile_root = make_profile(&root);
        write_global_config(
            &profile_root,
            "[Pad1]\nType = DualShock2\n[USB1]\nType = none\n",
        );
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert!(inspection.controllers.profile_configured);
        assert!(
            inspection
                .controllers
                .configured_sections
                .contains(&"Pad1".to_string())
        );
    }

    #[test]
    fn no_controller_sections_is_reported_honestly() {
        let root = fixture_root("no-controller");
        let profile_root = make_profile(&root);
        write_global_config(&profile_root, "[EmuCore]\nEnableCheats = true\n");
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert!(!inspection.controllers.profile_configured);
    }

    // -- patches/cheats reuse the existing, unchanged pipeline unchanged ----

    #[test]
    fn patches_and_crc_matching_reuse_the_existing_unchanged_pipeline() {
        let root = fixture_root("patches-reuse");
        let profile_root = make_profile(&root);
        fs::write(
            profile_root.join("cheats/F460F374.pnach"),
            "// Test cheat\npatch=1,EE,00000000,extended,00000000\n",
        )
        .unwrap();
        let profile = eligible_profile(&profile_root);
        let request = Pcsx2GameRequest {
            verified_ps2_serial: Some("SLUS-20312".to_string()),
            verified_executable_crc: Some("F460F374".to_string()),
            emulator_serial: None,
        };
        let inspection = inspect_pcsx2_game(&profile, &request);
        let patches = inspection.patches.expect("patch inventory present");
        assert_eq!(patches.files.len(), 1);
        let patch_match = inspection.patch_match.expect("match result present");
        assert_eq!(patch_match.state, Pcsx2MatchState::ExactCrcMatch);
        assert!(inspection.health.patch_data_available);
    }

    #[test]
    fn pnach_crc_has_zero_preservation_authority() {
        // A PNACH file's own CRC (used only for the existing, unchanged
        // patch-matching pipeline) must never influence `serial`/
        // `serial_mapping` - identity and patch-matching stay separate
        // concerns.
        let root = fixture_root("pnach-crc-authority");
        let profile_root = make_profile(&root);
        fs::write(
            profile_root.join("cheats/F460F374.pnach"),
            "patch=1,EE,00000000,extended,00000000\n",
        )
        .unwrap();
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert_eq!(inspection.serial_mapping, Pcsx2SerialMapping::Unavailable);
        assert!(inspection.serial.is_none());
    }

    #[test]
    fn widescreen_patch_category_is_distinguished_from_cheats() {
        let root = fixture_root("widescreen");
        let profile_root = make_profile(&root);
        fs::create_dir_all(profile_root.join("cheats_ws")).unwrap();
        fs::write(
            profile_root.join("cheats_ws/F460F374.pnach"),
            "patch=1,EE,00000000,extended,00000000\n",
        )
        .unwrap();
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        let patches = inspection.patches.expect("patch inventory present");
        assert!(
            patches
                .files
                .iter()
                .any(|file| file.category == Pcsx2PatchCategory::WidescreenPatches)
        );
    }

    // -- non-PS2 selection / detected health --------------------------------

    #[test]
    fn a_non_ps2_selection_simply_yields_no_serial_mapping() {
        // This module has no platform concept of its own - a caller simply
        // never supplies a `verified_ps2_serial` for a non-PS2 title, and
        // the result is identical to any other unresolved-identity case.
        let root = fixture_root("non-ps2");
        let profile_root = make_profile(&root);
        let profile = eligible_profile(&profile_root);
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert_eq!(inspection.serial_mapping, Pcsx2SerialMapping::Unavailable);
    }

    #[test]
    fn detected_reflects_existing_eligibility_unchanged() {
        let root = fixture_root("detected");
        let profile_root = make_profile(&root);
        let profile = eligible_profile(&profile_root);
        assert!(profile.eligible, "fixture must be eligible");
        let inspection = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        assert!(inspection.health.detected);
    }

    // -- no mutation ---------------------------------------------------------

    #[test]
    fn game_inspection_never_mutates_anything_it_reads() {
        let root = fixture_root("no-mutation");
        let profile_root = make_profile(&root);
        write_global_config(&profile_root, "[EmuCore/GS]\nRenderer = 12\n");
        let config_path = profile_root.join("inis/PCSX2.ini");
        let before = fs::read(&config_path).unwrap();
        let profile = eligible_profile(&profile_root);
        let _ = inspect_pcsx2_game(&profile, &Pcsx2GameRequest::default());
        let after = fs::read(&config_path).unwrap();
        assert_eq!(before, after);
    }
}
