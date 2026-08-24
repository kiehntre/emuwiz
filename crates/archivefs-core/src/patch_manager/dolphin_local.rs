//! Bounded, read-only discovery and inspection of local Dolphin profiles.
//!
//! The adapter never starts Dolphin and has no write or network capability.
//! It accepts documented native/Flatpak roots and exact roots supplied by a
//! trusted caller, rejects symlinked roots, and opens regular GameSettings INI
//! files with `O_NOFOLLOW` on Unix.

use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::emulator_environment::EncodedPath;
use crate::platform_evidence_fusion::evidence_lineage::{ClaimType, Representation};

use super::destination_safety::{
    DestinationRootState, DestinationSafetyFailureReason, validate_destination_root,
};
use super::resolved_emulator_profile::{
    EmulatorDestinationDirectories, EmulatorInstallationType, EmulatorProfileConfidence,
    ResolvedEmulatorProfile,
};
use super::{EmulatorProfileCandidate, EmulatorProfileSelectReason, EmulatorProfileSelection};

pub const DOLPHIN_MAX_PROFILES: usize = 16;
pub const DOLPHIN_MAX_ENTRIES_VISITED: usize = 10_000;
pub const DOLPHIN_MAX_GAME_INI_FILES: usize = 2_048;
pub const DOLPHIN_MAX_GAME_INI_BYTES: u64 = 256 * 1024;
pub const DOLPHIN_MAX_TOTAL_GAME_INI_BYTES: u64 = 16 * 1024 * 1024;
pub const DOLPHIN_MAX_LINES_PER_FILE: usize = 8_192;
pub const DOLPHIN_MAX_LINE_BYTES: usize = 8 * 1024;
/// Bounds the global Dolphin configuration and graphics configuration reads
/// performed by the modern local inspection API.
pub const DOLPHIN_LOCAL_MAX_CONFIG_BYTES: u64 = 256 * 1024;
pub const DOLPHIN_LOCAL_MAX_TEXTURE_FILES: usize = 2_048;
pub const DOLPHIN_LOCAL_MAX_TEXTURE_DEPTH: usize = 2;
pub const DOLPHIN_LOCAL_MAX_SAVE_CANDIDATES: usize = 128;

const FLATPAK_APP_ID: &str = "org.DolphinEmu.dolphin-emu";
const MAX_RETAINED_NAMES_PER_KIND: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinInstallationType {
    Native,
    AppImage,
    FlatpakUser,
    FlatpakSystem,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinProfileScope {
    User,
    SystemInstallationUserProfile,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinProfileBlockerKind {
    PathNotAbsolute,
    FilesystemRoot,
    MissingConfiguration,
    UnsafePath,
    NotDirectory,
    Unreadable,
    MissingDolphinEvidence,
    ProfileLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DolphinProfileBlocker {
    pub kind: DolphinProfileBlockerKind,
    pub path: EncodedPath,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinSettingsDirectoryState {
    Available,
    Missing,
    UnsafePath,
    NotDirectory,
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DolphinDirectoryIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinProfile {
    pub profile_id: String,
    pub installation_type: DolphinInstallationType,
    pub scope: DolphinProfileScope,
    pub configuration_path: PathBuf,
    pub provenance: String,
    pub eligible: bool,
    pub blockers: Vec<DolphinProfileBlocker>,
    pub game_settings_path: PathBuf,
    pub game_settings_state: DolphinSettingsDirectoryState,
    pub game_settings_warning: Option<String>,
    pub configuration_identity: Option<DolphinDirectoryIdentity>,
    pub game_settings_identity: Option<DolphinDirectoryIdentity>,
    pub resolved: ResolvedEmulatorProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinProfileDiscovery {
    pub profiles: Vec<DolphinProfile>,
    pub warnings: Vec<DolphinProfileBlocker>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub flatpak_system_root: PathBuf,
    /// Exact, already-known Dolphin user directories; never search for these.
    pub explicit_configuration_roots: Vec<PathBuf>,
    /// Running Dolphin argv records. Production fills these from bounded
    /// `/proc/<pid>/cmdline` reads; tests and non-Linux callers can supply
    /// the same lossless argv representation directly.
    pub running_commands: Vec<DolphinCommandLine>,
    /// Launch commands associated with the selected emulator even when it
    /// is not currently running.
    pub selected_launch_commands: Vec<DolphinCommandLine>,
    /// Optional selected executable used to prefer its process/profile over
    /// unrelated simultaneously running Dolphin installations.
    pub selected_executable: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinCommandLine {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    /// Flatpak application identity established from the process sandbox's
    /// `.flatpak-info`, never inferred from an arbitrary argv string.
    pub flatpak_app_id: Option<String>,
}

impl DolphinProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, DolphinDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(DolphinDiscoveryError::HomeUnavailable)?;
        Ok(Self {
            xdg_config_home: env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config")),
            xdg_data_home: env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share")),
            home,
            flatpak_system_root: PathBuf::from("/var/lib/flatpak"),
            explicit_configuration_roots: Vec::new(),
            running_commands: discover_running_dolphin_commands(),
            selected_launch_commands: Vec::new(),
            selected_executable: None,
        })
    }
}

#[derive(Debug)]
pub enum DolphinDiscoveryError {
    HomeUnavailable,
}

impl std::fmt::Display for DolphinDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable => f.write_str("HOME is not set"),
        }
    }
}

impl std::error::Error for DolphinDiscoveryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinCodeKind {
    FramePatch,
    ActionReplay,
    Gecko,
    Riivolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinInspectionWarningKind {
    UnsafePath,
    UnreadablePath,
    SymlinkSkipped,
    SpecialFileSkipped,
    EntryLimitReached,
    FileCountLimitReached,
    FileTooLarge,
    TotalBytesLimitReached,
    LineCountLimitReached,
    LineTooLong,
    MalformedIni,
    InvalidUtf8,
    InvalidGameId,
    DuplicateGameIdentity,
    DuplicateFilename,
    DuplicateContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinInspectionWarning {
    pub kind: DolphinInspectionWarningKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinGameIniFile {
    pub path: PathBuf,
    pub filename_stem: OsString,
    pub game_id_candidate: Option<String>,
    pub revision_candidate: Option<u16>,
    pub region_candidate: Option<String>,
    pub frame_patch_names: Vec<String>,
    pub action_replay_names: Vec<String>,
    pub gecko_names: Vec<String>,
    pub riivolution_names: Vec<String>,
    pub enabled_frame_patch_names: Vec<String>,
    pub enabled_action_replay_names: Vec<String>,
    pub enabled_gecko_names: Vec<String>,
    pub enabled_riivolution_names: Vec<String>,
    pub size_bytes: u64,
    pub sha256: String,
    pub duplicate_game_identity: bool,
    pub duplicate_filename: bool,
    pub duplicate_content: bool,
    pub warnings: Vec<DolphinInspectionWarningKind>,
}

impl DolphinGameIniFile {
    pub fn definition_count(&self) -> usize {
        self.frame_patch_names.len()
            + self.action_replay_names.len()
            + self.gecko_names.len()
            + self.riivolution_names.len()
    }

    pub fn enabled_count(&self) -> usize {
        self.enabled_frame_patch_names.len()
            + self.enabled_action_replay_names.len()
            + self.enabled_gecko_names.len()
            + self.enabled_riivolution_names.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinGameIniInventory {
    pub profile_id: String,
    pub files: Vec<DolphinGameIniFile>,
    pub warnings: Vec<DolphinInspectionWarning>,
    pub entries_visited: usize,
    pub bytes_inspected: u64,
    pub complete: bool,
}

#[derive(Debug)]
pub enum DolphinInspectionError {
    IneligibleProfile { profile_id: String },
    ProfileChanged { path: PathBuf },
    UnsafeProfile { path: PathBuf },
}

impl std::fmt::Display for DolphinInspectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IneligibleProfile { profile_id } => {
                write!(f, "Dolphin profile {profile_id} is not eligible")
            }
            Self::ProfileChanged { path } => {
                write!(
                    f,
                    "Dolphin profile changed before inspection: {}",
                    path.display()
                )
            }
            Self::UnsafeProfile { path } => {
                write!(f, "Dolphin profile path is unsafe: {}", path.display())
            }
        }
    }
}

impl std::error::Error for DolphinInspectionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinMatchState {
    ExactGameIdMatch,
    ExactGameIdAndRevisionMatch,
    MultipleIniFilesForGame,
    NoVerifiedGameIdAvailable,
    NoMatchingIniFound,
    InvalidVerifiedGameId,
    RevisionMismatch,
    IdentityExtractionDeferred,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinMatchResult {
    pub state: DolphinMatchState,
    pub verified_game_id: Option<String>,
    pub verified_revision: Option<u16>,
    pub matching_files: Vec<PathBuf>,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct ProfileCandidate {
    installation_type: DolphinInstallationType,
    scope: DolphinProfileScope,
    configuration_root: PathBuf,
    /// Dolphin user/data root containing GameSettings and Load.
    path: PathBuf,
    provenance: String,
    report_missing: bool,
    executable: Option<PathBuf>,
    explicit_profile: bool,
    confidence: EmulatorProfileConfidence,
    priority: u16,
}

/// Discovers documented paths and exact caller-supplied roots only.
pub fn discover_dolphin_profiles(
    roots: &DolphinProfileDiscoveryRoots,
) -> Result<DolphinProfileDiscovery, DolphinDiscoveryError> {
    let flatpak_sandbox = roots.home.join(".var/app").join(FLATPAK_APP_ID);
    let flatpak_data_path = flatpak_sandbox.join("data/dolphin-emu");
    let flatpak_legacy_config_path = flatpak_sandbox.join("config/dolphin-emu");
    // Current Flatpak Dolphin splits configuration and user data: Dolphin.ini
    // remains under config while GameSettings lives under data. An older
    // config-tree user root is retained only when its own GameSettings is
    // positive evidence and the data tree has no GameSettings.
    let flatpak_uses_data_tree = real_directory(&flatpak_data_path)
        && (real_directory(&flatpak_data_path.join("GameSettings"))
            || !real_directory(&flatpak_legacy_config_path.join("GameSettings")));
    let flatpak_user_path = if flatpak_uses_data_tree {
        flatpak_data_path.clone()
    } else {
        flatpak_legacy_config_path.clone()
    };
    let user_install = roots.xdg_data_home.join("flatpak/app").join(FLATPAK_APP_ID);
    let system_install = roots.flatpak_system_root.join("app").join(FLATPAK_APP_ID);
    let system_only = real_directory(&system_install) && !real_directory(&user_install);
    let (flatpak_kind, flatpak_scope) = if system_only {
        (
            DolphinInstallationType::FlatpakSystem,
            DolphinProfileScope::SystemInstallationUserProfile,
        )
    } else {
        (
            DolphinInstallationType::FlatpakUser,
            DolphinProfileScope::User,
        )
    };
    let mut candidates = vec![
        ProfileCandidate {
            installation_type: DolphinInstallationType::Native,
            scope: DolphinProfileScope::User,
            configuration_root: roots.xdg_config_home.join("dolphin-emu"),
            path: roots.xdg_config_home.join("dolphin-emu"),
            provenance: "Native Dolphin XDG user directory fallback".to_string(),
            report_missing: false,
            executable: None,
            explicit_profile: false,
            confidence: EmulatorProfileConfidence::Speculative,
            priority: 100,
        },
        ProfileCandidate {
            installation_type: flatpak_kind,
            scope: flatpak_scope,
            configuration_root: flatpak_legacy_config_path.clone(),
            path: flatpak_user_path.clone(),
            provenance: if flatpak_uses_data_tree {
                "Known Flatpak data path for org.DolphinEmu.dolphin-emu".to_string()
            } else {
                "Existing legacy Flatpak config-tree Dolphin profile".to_string()
            },
            report_missing: false,
            executable: Some(PathBuf::from("/usr/bin/flatpak")),
            explicit_profile: false,
            confidence: EmulatorProfileConfidence::KnownPath,
            priority: 150,
        },
    ];
    for command in &roots.running_commands {
        if let Some(path) = dolphin_user_path(&command.arguments) {
            let selected_bonus = u16::from(
                roots.selected_executable.as_deref() == Some(command.executable.as_path()),
            ) * 25;
            candidates.push(command_candidate(
                command,
                path,
                EmulatorProfileConfidence::RunningExplicit,
                400 + selected_bonus,
                "Running Dolphin command line",
            ));
        } else if command.flatpak_app_id.as_deref() == Some(FLATPAK_APP_ID) {
            let path = if real_directory(&flatpak_user_path) {
                flatpak_user_path.clone()
            } else {
                continue;
            };
            candidates.push(ProfileCandidate {
                installation_type: flatpak_kind,
                scope: flatpak_scope,
                configuration_root: flatpak_legacy_config_path.clone(),
                path,
                provenance: format!(
                    "Running Flatpak Dolphin process: {} ({FLATPAK_APP_ID})",
                    command.executable.display()
                ),
                report_missing: true,
                executable: Some(command.executable.clone()),
                explicit_profile: false,
                confidence: EmulatorProfileConfidence::RunningExplicit,
                priority: 400,
            });
        }
    }
    for command in &roots.selected_launch_commands {
        if let Some(path) = dolphin_user_path(&command.arguments) {
            candidates.push(command_candidate(
                command,
                path,
                EmulatorProfileConfidence::SelectedLaunch,
                350,
                "Selected Dolphin launch command",
            ));
        }
    }
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|path| ProfileCandidate {
                installation_type: DolphinInstallationType::Explicit,
                scope: DolphinProfileScope::Explicit,
                configuration_root: path.clone(),
                path,
                provenance: "User-confirmed Dolphin profile override".to_string(),
                report_missing: true,
                executable: None,
                explicit_profile: true,
                confidence: EmulatorProfileConfidence::UserConfirmed,
                priority: 500,
            }),
    );
    candidates.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| b.priority.cmp(&a.priority))
    });
    candidates.dedup_by(|a, b| {
        if a.path != b.path {
            return false;
        }
        if !a.provenance.contains(&b.provenance) {
            a.provenance.push_str("; ");
            a.provenance.push_str(&b.provenance);
        }
        true
    });

    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for candidate in candidates {
        if profiles.len() >= DOLPHIN_MAX_PROFILES {
            warnings.push(blocker(
                DolphinProfileBlockerKind::ProfileLimitReached,
                &candidate.path,
                format!("profile discovery stopped at the {DOLPHIN_MAX_PROFILES}-profile limit"),
            ));
            break;
        }
        if !candidate.path.is_absolute() {
            profiles.push(blocked(
                candidate,
                DolphinProfileBlockerKind::PathNotAbsolute,
                "configuration path is not absolute",
            ));
            continue;
        }
        if candidate.path.parent().is_none() {
            profiles.push(blocked(
                candidate,
                DolphinProfileBlockerKind::FilesystemRoot,
                "a filesystem root cannot be a Dolphin profile",
            ));
            continue;
        }
        let validated = match validate_destination_root(&candidate.path) {
            Ok(value) => value,
            Err(error) => {
                let kind = match error.reason {
                    DestinationSafetyFailureReason::RootNotDirectory
                    | DestinationSafetyFailureReason::NonDirectoryParent => {
                        DolphinProfileBlockerKind::NotDirectory
                    }
                    DestinationSafetyFailureReason::InspectionFailed => {
                        DolphinProfileBlockerKind::Unreadable
                    }
                    _ => DolphinProfileBlockerKind::UnsafePath,
                };
                profiles.push(blocked(
                    candidate,
                    kind,
                    format!("configuration path rejected: {:?}", error.reason),
                ));
                continue;
            }
        };
        if validated.state() == DestinationRootState::Absent {
            if candidate.report_missing {
                profiles.push(blocked(
                    candidate,
                    DolphinProfileBlockerKind::MissingConfiguration,
                    "configuration directory does not exist",
                ));
            }
            continue;
        }
        if let Err((kind, detail)) = inspect_marker(&candidate.configuration_root) {
            profiles.push(blocked(candidate, kind, detail));
            continue;
        }
        let settings_path = candidate.path.join("GameSettings");
        let (settings_state, settings_warning, settings_identity) =
            inspect_settings(&settings_path);
        let identity = fs::symlink_metadata(&candidate.path)
            .ok()
            .and_then(|m| directory_identity(&m));
        profiles.push(DolphinProfile {
            profile_id: profile_id(candidate.installation_type, &candidate.path),
            installation_type: candidate.installation_type,
            scope: candidate.scope,
            configuration_path: candidate.path.clone(),
            provenance: candidate.provenance.clone(),
            eligible: true,
            blockers: Vec::new(),
            game_settings_path: settings_path.clone(),
            game_settings_state: settings_state,
            game_settings_warning: settings_warning,
            configuration_identity: identity,
            game_settings_identity: settings_identity,
            resolved: resolved_profile(&candidate, &settings_path),
        });
    }
    profiles.sort_by(|a, b| {
        a.installation_type
            .cmp(&b.installation_type)
            .then_with(|| a.configuration_path.cmp(&b.configuration_path))
    });
    Ok(DolphinProfileDiscovery {
        complete: warnings.is_empty(),
        profiles,
        warnings,
    })
}

fn inspect_marker(root: &Path) -> Result<(), (DolphinProfileBlockerKind, &'static str)> {
    let marker = root.join("Dolphin.ini");
    match fs::symlink_metadata(marker) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err((
            DolphinProfileBlockerKind::UnsafePath,
            "Dolphin.ini is a symlink",
        )),
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err((
            DolphinProfileBlockerKind::MissingDolphinEvidence,
            "Dolphin.ini is not a regular file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err((
            DolphinProfileBlockerKind::MissingDolphinEvidence,
            "Dolphin.ini was not found",
        )),
        Err(_) => Err((
            DolphinProfileBlockerKind::Unreadable,
            "Dolphin.ini is unreadable",
        )),
    }
}

fn inspect_settings(
    path: &Path,
) -> (
    DolphinSettingsDirectoryState,
    Option<String>,
    Option<DolphinDirectoryIdentity>,
) {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => (
            DolphinSettingsDirectoryState::UnsafePath,
            Some("GameSettings is a symlink and will not be followed".into()),
            None,
        ),
        Ok(m) if m.is_dir() => (
            DolphinSettingsDirectoryState::Available,
            None,
            directory_identity(&m),
        ),
        Ok(_) => (
            DolphinSettingsDirectoryState::NotDirectory,
            Some("GameSettings is not a directory".into()),
            None,
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            (DolphinSettingsDirectoryState::Missing, None, None)
        }
        Err(e) => (
            DolphinSettingsDirectoryState::Unreadable,
            Some(format!("GameSettings cannot be inspected: {e}")),
            None,
        ),
    }
}

fn blocked(
    candidate: ProfileCandidate,
    kind: DolphinProfileBlockerKind,
    detail: impl Into<String>,
) -> DolphinProfile {
    let settings = candidate.path.join("GameSettings");
    DolphinProfile {
        profile_id: profile_id(candidate.installation_type, &candidate.path),
        installation_type: candidate.installation_type,
        scope: candidate.scope,
        configuration_path: candidate.path.clone(),
        provenance: candidate.provenance.clone(),
        eligible: false,
        blockers: vec![blocker(kind, &candidate.path, detail)],
        game_settings_path: settings.clone(),
        game_settings_state: DolphinSettingsDirectoryState::Missing,
        game_settings_warning: None,
        configuration_identity: None,
        game_settings_identity: None,
        resolved: resolved_profile(&candidate, &settings),
    }
}

fn blocker(
    kind: DolphinProfileBlockerKind,
    path: &Path,
    detail: impl Into<String>,
) -> DolphinProfileBlocker {
    DolphinProfileBlocker {
        kind,
        path: EncodedPath::from_path(path),
        detail: detail.into(),
    }
}

fn real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

fn profile_id(kind: DolphinInstallationType, path: &Path) -> String {
    let mut digest = Sha256::new();
    #[cfg(unix)]
    digest.update(path.as_os_str().as_bytes());
    #[cfg(not(unix))]
    digest.update(path.as_os_str().to_string_lossy().as_bytes());
    let kind = match kind {
        DolphinInstallationType::Native => "native",
        DolphinInstallationType::AppImage => "appimage",
        DolphinInstallationType::FlatpakUser => "flatpak-user",
        DolphinInstallationType::FlatpakSystem => "flatpak-system",
        DolphinInstallationType::Explicit => "explicit",
    };
    format!(
        "dolphin-{kind}-{:016x}",
        u64::from_be_bytes(digest.finalize()[..8].try_into().unwrap())
    )
}

fn command_candidate(
    command: &DolphinCommandLine,
    path: PathBuf,
    confidence: EmulatorProfileConfidence,
    priority: u16,
    evidence: &str,
) -> ProfileCandidate {
    let appimage = is_appimage_executable(&command.executable);
    let flatpak = command.flatpak_app_id.as_deref() == Some(FLATPAK_APP_ID);
    ProfileCandidate {
        installation_type: if flatpak {
            DolphinInstallationType::FlatpakUser
        } else if appimage {
            DolphinInstallationType::AppImage
        } else {
            DolphinInstallationType::Explicit
        },
        scope: DolphinProfileScope::Explicit,
        configuration_root: path.clone(),
        provenance: format!(
            "{evidence}: {} --user-directory {}",
            command.executable.display(),
            path.display()
        ),
        path,
        report_missing: true,
        executable: Some(command.executable.clone()),
        explicit_profile: true,
        confidence,
        priority,
    }
}

fn is_appimage_executable(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str().to_string_lossy().starts_with(".mount_"))
        || path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("appimage"))
}

/// Extracts Dolphin's explicit user directory from a lossless argv vector.
/// `-u path`, `-u=path`, `--user path`, and `--user=path` are supported;
/// spaces are preserved because `/proc/<pid>/cmdline` is NUL-delimited rather
/// than shell-tokenized. Only absolute non-root paths are accepted here;
/// filesystem and symlink safety is then checked by profile discovery.
pub fn dolphin_user_path(arguments: &[OsString]) -> Option<PathBuf> {
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "-u" || argument == "--user" {
            return arguments
                .next()
                .and_then(|value| valid_dolphin_user_path(value.as_os_str()));
        }
        let text = argument.to_string_lossy();
        if let Some(value) = text
            .strip_prefix("-u=")
            .or_else(|| text.strip_prefix("--user="))
        {
            return valid_dolphin_user_path(OsString::from(value).as_os_str());
        }
    }
    None
}

fn valid_dolphin_user_path(value: &std::ffi::OsStr) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    (path.is_absolute() && path.parent().is_some()).then_some(path)
}

/// Dolphin-specific profile selection. A live runtime is authoritative over
/// remembered or merely installed profiles. Ambiguous live runtimes, and
/// multiple credible profiles while Dolphin is stopped, require an explicit
/// current-session choice rather than a hidden default.
pub fn select_dolphin_profile(
    discovery: &DolphinProfileDiscovery,
    session_explicit: Option<&str>,
) -> EmulatorProfileSelection {
    let candidates: Vec<EmulatorProfileCandidate> = discovery
        .profiles
        .iter()
        .map(|profile| EmulatorProfileCandidate {
            profile_id: profile.profile_id.clone(),
            root: profile.configuration_path.clone(),
            eligible: profile.eligible,
            is_portable: matches!(
                profile.installation_type,
                DolphinInstallationType::Explicit | DolphinInstallationType::AppImage
            ),
            evidence_priority: profile.resolved.priority,
        })
        .collect();
    let eligible: Vec<&DolphinProfile> = discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .collect();
    let active: Vec<&DolphinProfile> = eligible
        .iter()
        .copied()
        .filter(|profile| profile.resolved.confidence == EmulatorProfileConfidence::RunningExplicit)
        .collect();

    if active.len() == 1 {
        return EmulatorProfileSelection::Auto {
            profile_id: active[0].profile_id.clone(),
            reason: EmulatorProfileSelectReason::StrongestEvidence,
        };
    }
    if active.len() > 1 {
        if let Some(selected) = session_explicit
            && let Some(profile) = active.iter().find(|profile| profile.profile_id == selected)
        {
            return EmulatorProfileSelection::Auto {
                profile_id: profile.profile_id.clone(),
                reason: EmulatorProfileSelectReason::ExplicitChoice,
            };
        }
        return EmulatorProfileSelection::NeedsChoice { candidates };
    }
    if let Some(selected) = session_explicit
        && let Some(profile) = eligible
            .iter()
            .find(|profile| profile.profile_id == selected)
    {
        return EmulatorProfileSelection::Auto {
            profile_id: profile.profile_id.clone(),
            reason: EmulatorProfileSelectReason::ExplicitChoice,
        };
    }
    match eligible.as_slice() {
        [] => EmulatorProfileSelection::SetupNeeded,
        [profile] => EmulatorProfileSelection::Auto {
            profile_id: profile.profile_id.clone(),
            reason: EmulatorProfileSelectReason::OnlyValidProfile,
        },
        _ => EmulatorProfileSelection::NeedsChoice { candidates },
    }
}

fn resolved_profile(candidate: &ProfileCandidate, settings_path: &Path) -> ResolvedEmulatorProfile {
    let installation_type = match candidate.installation_type {
        DolphinInstallationType::Native => EmulatorInstallationType::NativeSystem,
        DolphinInstallationType::AppImage => EmulatorInstallationType::AppImage,
        DolphinInstallationType::FlatpakUser | DolphinInstallationType::FlatpakSystem => {
            EmulatorInstallationType::Flatpak
        }
        DolphinInstallationType::Explicit => EmulatorInstallationType::PortableCustom,
    };
    let writable = fs::metadata(&candidate.path)
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false);
    ResolvedEmulatorProfile {
        emulator_executable: candidate.executable.clone(),
        installation_type,
        configuration_root: candidate.configuration_root.clone(),
        data_user_root: candidate.path.clone(),
        active_explicit_profile: candidate.explicit_profile.then(|| candidate.path.clone()),
        destinations: EmulatorDestinationDirectories {
            cheats: Some(settings_path.to_path_buf()),
            patches: Some(settings_path.to_path_buf()),
            mods: Some(candidate.path.join("Load")),
            game_settings: Some(settings_path.to_path_buf()),
        },
        discovery_evidence: vec![candidate.provenance.clone()],
        confidence: candidate.confidence,
        priority: candidate.priority,
        writable,
    }
}

#[cfg(target_os = "linux")]
fn discover_running_dolphin_commands() -> Vec<DolphinCommandLine> {
    const MAX_PROCESSES: usize = 4096;
    const MAX_CMDLINE_BYTES: u64 = 64 * 1024;
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    for entry in entries.filter_map(Result::ok).take(MAX_PROCESSES) {
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let process = entry.path();
        let Ok(executable) = fs::read_link(process.join("exe")) else {
            continue;
        };
        let name_is_dolphin = executable.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .to_ascii_lowercase()
                .contains("dolphin")
        });
        if !name_is_dolphin {
            continue;
        }
        let Ok(mut file) = File::open(process.join("cmdline")) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file
            .by_ref()
            .take(MAX_CMDLINE_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > MAX_CMDLINE_BYTES
        {
            continue;
        }
        #[cfg(unix)]
        let arguments = bytes
            .split(|byte| *byte == 0)
            .filter(|argument| !argument.is_empty())
            .skip(1)
            .map(|argument| OsString::from_vec(argument.to_vec()))
            .collect();
        // Re-read the executable after cmdline/sandbox inspection. If the
        // process exited or the PID was reused, discard the stale snapshot.
        if !same_executable_snapshot(
            &executable,
            fs::read_link(process.join("exe")).ok().as_deref(),
        ) {
            continue;
        }
        commands.push(DolphinCommandLine {
            executable,
            arguments,
            flatpak_app_id: read_flatpak_app_id(&process.join("root/.flatpak-info")),
        });
    }
    commands.sort_by(|left, right| left.executable.cmp(&right.executable));
    commands
}

#[cfg(target_os = "linux")]
fn same_executable_snapshot(first: &Path, second: Option<&Path>) -> bool {
    second == Some(first)
}

#[cfg(target_os = "linux")]
fn read_flatpak_app_id(path: &Path) -> Option<String> {
    const MAX_FLATPAK_INFO_BYTES: u64 = 16 * 1024;
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_FLATPAK_INFO_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_FLATPAK_INFO_BYTES {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    let mut in_application = false;
    for line in text.lines() {
        if line.starts_with('[') {
            in_application = line == "[Application]";
        } else if in_application
            && let Some(value) = line.strip_prefix("name=")
            && value == FLATPAK_APP_ID
        {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn discover_running_dolphin_commands() -> Vec<DolphinCommandLine> {
    Vec::new()
}

pub fn inspect_dolphin_profile(
    profile: &DolphinProfile,
) -> Result<DolphinGameIniInventory, DolphinInspectionError> {
    inspect_dolphin_profile_with_limit(profile, DOLPHIN_MAX_GAME_INI_FILES)
}

fn inspect_dolphin_profile_with_limit(
    profile: &DolphinProfile,
    file_limit: usize,
) -> Result<DolphinGameIniInventory, DolphinInspectionError> {
    if !profile.eligible {
        return Err(DolphinInspectionError::IneligibleProfile {
            profile_id: profile.profile_id.clone(),
        });
    }
    let validated = validate_destination_root(&profile.configuration_path).map_err(|_| {
        DolphinInspectionError::UnsafeProfile {
            path: profile.configuration_path.clone(),
        }
    })?;
    if validated.state() != DestinationRootState::ExistingDirectory
        || inspect_marker(&profile.resolved.configuration_root).is_err()
    {
        return Err(DolphinInspectionError::ProfileChanged {
            path: profile.configuration_path.clone(),
        });
    }
    if profile.configuration_identity.is_some()
        && fs::symlink_metadata(&profile.configuration_path)
            .ok()
            .and_then(|m| directory_identity(&m))
            != profile.configuration_identity
    {
        return Err(DolphinInspectionError::ProfileChanged {
            path: profile.configuration_path.clone(),
        });
    }
    let mut inventory = DolphinGameIniInventory {
        profile_id: profile.profile_id.clone(),
        files: Vec::new(),
        warnings: Vec::new(),
        entries_visited: 0,
        bytes_inspected: 0,
        complete: true,
    };
    if profile.game_settings_state != DolphinSettingsDirectoryState::Available {
        return Ok(inventory);
    }
    if !matches!(validate_destination_root(&profile.game_settings_path), Ok(root) if root.state() == DestinationRootState::ExistingDirectory)
        || (profile.game_settings_identity.is_some()
            && fs::symlink_metadata(&profile.game_settings_path)
                .ok()
                .and_then(|m| directory_identity(&m))
                != profile.game_settings_identity)
    {
        warn(
            &mut inventory,
            DolphinInspectionWarningKind::UnsafePath,
            &profile.game_settings_path,
            "GameSettings path or identity changed after discovery",
        );
        return Ok(inventory);
    }
    let entries = match fs::read_dir(&profile.game_settings_path) {
        Ok(entries) => entries,
        Err(error) => {
            warn(
                &mut inventory,
                DolphinInspectionWarningKind::UnreadablePath,
                &profile.game_settings_path,
                format!("GameSettings cannot be read: {error}"),
            );
            return Ok(inventory);
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        if inventory.entries_visited >= DOLPHIN_MAX_ENTRIES_VISITED {
            warn(
                &mut inventory,
                DolphinInspectionWarningKind::EntryLimitReached,
                &profile.game_settings_path,
                format!("entry inspection stopped at {DOLPHIN_MAX_ENTRIES_VISITED}"),
            );
            break;
        }
        inventory.entries_visited += 1;
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(error) => warn(
                &mut inventory,
                DolphinInspectionWarningKind::UnreadablePath,
                &profile.game_settings_path,
                format!("directory entry cannot be read: {error}"),
            ),
        }
    }
    paths.sort();
    for path in paths {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(error) => {
                warn(
                    &mut inventory,
                    DolphinInspectionWarningKind::UnreadablePath,
                    &path,
                    format!("entry cannot be inspected: {error}"),
                );
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            warn(
                &mut inventory,
                DolphinInspectionWarningKind::SymlinkSkipped,
                &path,
                "symlink was not followed",
            );
            continue;
        }
        if !metadata.is_file() {
            if is_ini(&path) {
                warn(
                    &mut inventory,
                    DolphinInspectionWarningKind::SpecialFileSkipped,
                    &path,
                    "non-regular INI entry was skipped",
                );
            }
            continue;
        }
        if !is_ini(&path) {
            continue;
        }
        if inventory.files.len() >= file_limit {
            warn(
                &mut inventory,
                DolphinInspectionWarningKind::FileCountLimitReached,
                &path,
                format!("INI parsing stopped at {file_limit} files"),
            );
            break;
        }
        if metadata.len() > DOLPHIN_MAX_GAME_INI_BYTES {
            warn(
                &mut inventory,
                DolphinInspectionWarningKind::FileTooLarge,
                &path,
                format!("INI exceeds {DOLPHIN_MAX_GAME_INI_BYTES} bytes"),
            );
            continue;
        }
        if inventory.bytes_inspected.saturating_add(metadata.len())
            > DOLPHIN_MAX_TOTAL_GAME_INI_BYTES
        {
            warn(
                &mut inventory,
                DolphinInspectionWarningKind::TotalBytesLimitReached,
                &path,
                format!("total INI input would exceed {DOLPHIN_MAX_TOTAL_GAME_INI_BYTES} bytes"),
            );
            break;
        }
        if let Some(file) = inspect_ini(&path, metadata.len(), &mut inventory) {
            inventory.files.push(file);
        }
    }
    mark_duplicates(&mut inventory);
    inventory.files.sort_by(|a, b| a.path.cmp(&b.path));
    inventory
        .warnings
        .sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(&b.kind)));
    Ok(inventory)
}

fn inspect_ini(
    path: &Path,
    expected_size: u64,
    inventory: &mut DolphinGameIniInventory,
) -> Option<DolphinGameIniFile> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) => {
            warn(
                inventory,
                DolphinInspectionWarningKind::UnreadablePath,
                path,
                format!("INI cannot be opened safely: {error}"),
            );
            return None;
        }
    };
    let metadata = match file.metadata() {
        Ok(m) if m.is_file() && m.len() == expected_size => m,
        Ok(_) => {
            warn(
                inventory,
                DolphinInspectionWarningKind::UnsafePath,
                path,
                "INI identity or size changed before reading",
            );
            return None;
        }
        Err(error) => {
            warn(
                inventory,
                DolphinInspectionWarningKind::UnreadablePath,
                path,
                format!("opened INI cannot be inspected: {error}"),
            );
            return None;
        }
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .by_ref()
        .take(DOLPHIN_MAX_GAME_INI_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 != metadata.len()
    {
        warn(
            inventory,
            DolphinInspectionWarningKind::UnreadablePath,
            path,
            "INI could not be read completely",
        );
        return None;
    }
    let mut local_warnings = Vec::new();
    if bytes.split(|b| *b == b'\n').count() > DOLPHIN_MAX_LINES_PER_FILE {
        warn(
            inventory,
            DolphinInspectionWarningKind::LineCountLimitReached,
            path,
            format!("INI exceeds {DOLPHIN_MAX_LINES_PER_FILE} lines"),
        );
        return None;
    }
    if bytes
        .split(|b| *b == b'\n')
        .any(|line| line.len() > DOLPHIN_MAX_LINE_BYTES)
    {
        warn(
            inventory,
            DolphinInspectionWarningKind::LineTooLong,
            path,
            format!("INI contains a line over {DOLPHIN_MAX_LINE_BYTES} bytes"),
        );
        return None;
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            local_warnings.push(DolphinInspectionWarningKind::InvalidUtf8);
            warn(
                inventory,
                DolphinInspectionWarningKind::InvalidUtf8,
                path,
                "INI is not valid UTF-8; invalid bytes were replaced for structural parsing",
            );
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    let (game_id, revision, region) = parse_game_identity(path.file_stem().unwrap_or_default());
    if game_id.is_none() {
        local_warnings.push(DolphinInspectionWarningKind::InvalidGameId);
        warn(
            inventory,
            DolphinInspectionWarningKind::InvalidGameId,
            path,
            "filename is not a supported Dolphin game ID with optional revision",
        );
    }
    let mut parsed = ParsedIni::default();
    parse_ini_text(&text, &mut parsed, &mut local_warnings);
    if local_warnings.contains(&DolphinInspectionWarningKind::MalformedIni) {
        warn(
            inventory,
            DolphinInspectionWarningKind::MalformedIni,
            path,
            "INI contains malformed section or code-name syntax",
        );
    }
    inventory.bytes_inspected += bytes.len() as u64;
    Some(DolphinGameIniFile {
        path: path.to_path_buf(),
        filename_stem: path.file_stem().unwrap_or_default().to_os_string(),
        game_id_candidate: game_id,
        revision_candidate: revision,
        region_candidate: region,
        frame_patch_names: parsed.frame,
        action_replay_names: parsed.ar,
        gecko_names: parsed.gecko,
        riivolution_names: parsed.riivolution,
        enabled_frame_patch_names: parsed.frame_enabled,
        enabled_action_replay_names: parsed.ar_enabled,
        enabled_gecko_names: parsed.gecko_enabled,
        enabled_riivolution_names: parsed.riivolution_enabled,
        size_bytes: bytes.len() as u64,
        sha256: Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        duplicate_game_identity: false,
        duplicate_filename: false,
        duplicate_content: false,
        warnings: local_warnings,
    })
}

#[derive(Default)]
struct ParsedIni {
    frame: Vec<String>,
    ar: Vec<String>,
    gecko: Vec<String>,
    riivolution: Vec<String>,
    frame_enabled: Vec<String>,
    ar_enabled: Vec<String>,
    gecko_enabled: Vec<String>,
    riivolution_enabled: Vec<String>,
}

fn parse_ini_text(
    text: &str,
    parsed: &mut ParsedIni,
    warnings: &mut Vec<DolphinInspectionWarningKind>,
) {
    let mut section = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') {
                push_unique_warning(warnings, DolphinInspectionWarningKind::MalformedIni);
                section = None;
                continue;
            }
            section = section_kind(&line[1..line.len() - 1]);
            continue;
        }
        let Some((kind, enabled)) = section else {
            continue;
        };
        if !line.starts_with('$') {
            continue;
        }
        let name = line[1..]
            .split(['=', '\t'])
            .next()
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            push_unique_warning(warnings, DolphinInspectionWarningKind::MalformedIni);
            continue;
        }
        let target = match (kind, enabled) {
            (DolphinCodeKind::FramePatch, false) => &mut parsed.frame,
            (DolphinCodeKind::FramePatch, true) => &mut parsed.frame_enabled,
            (DolphinCodeKind::ActionReplay, false) => &mut parsed.ar,
            (DolphinCodeKind::ActionReplay, true) => &mut parsed.ar_enabled,
            (DolphinCodeKind::Gecko, false) => &mut parsed.gecko,
            (DolphinCodeKind::Gecko, true) => &mut parsed.gecko_enabled,
            (DolphinCodeKind::Riivolution, false) => &mut parsed.riivolution,
            (DolphinCodeKind::Riivolution, true) => &mut parsed.riivolution_enabled,
        };
        if target.len() < MAX_RETAINED_NAMES_PER_KIND && !target.iter().any(|value| value == name) {
            target.push(name.to_string());
        }
    }
}

fn section_kind(value: &str) -> Option<(DolphinCodeKind, bool)> {
    match value.trim().to_ascii_lowercase().as_str() {
        "onframe" => Some((DolphinCodeKind::FramePatch, false)),
        "onframe_enabled" => Some((DolphinCodeKind::FramePatch, true)),
        "actionreplay" => Some((DolphinCodeKind::ActionReplay, false)),
        "actionreplay_enabled" => Some((DolphinCodeKind::ActionReplay, true)),
        "gecko" => Some((DolphinCodeKind::Gecko, false)),
        "gecko_enabled" => Some((DolphinCodeKind::Gecko, true)),
        "riivolution" => Some((DolphinCodeKind::Riivolution, false)),
        "riivolution_enabled" => Some((DolphinCodeKind::Riivolution, true)),
        _ => None,
    }
}

fn parse_game_identity(stem: &std::ffi::OsStr) -> (Option<String>, Option<u16>, Option<String>) {
    let Some(stem) = stem.to_str() else {
        return (None, None, None);
    };
    let (id, revision) = match stem.rsplit_once('r') {
        Some((id, revision))
            if !revision.is_empty() && revision.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let Ok(revision) = revision.parse::<u16>() else {
                return (None, None, None);
            };
            (id, Some(revision))
        }
        _ => (stem, None),
    };
    if !(3..=6).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return (None, None, None);
    }
    let id = id.to_ascii_uppercase();
    let region = (id.len() >= 4).then(|| {
        match id.as_bytes()[3] as char {
            'E' => "NTSC-U",
            'J' => "NTSC-J",
            'K' | 'Q' | 'T' => "NTSC-K",
            'P' | 'D' | 'F' | 'H' | 'I' | 'L' | 'M' | 'R' | 'S' | 'U' | 'V' | 'X' | 'Y' | 'Z' => {
                "PAL"
            }
            _ => "Unknown",
        }
        .to_string()
    });
    (Some(id), revision, region)
}

fn normalize_verified_game_id(value: &str) -> Option<String> {
    let value = value.trim();
    ((3..=6).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| value.to_ascii_uppercase())
}

pub fn match_dolphin_inventory(
    inventory: &DolphinGameIniInventory,
    verified_game_id: Option<&str>,
    verified_revision: Option<u16>,
) -> DolphinMatchResult {
    let Some(value) = verified_game_id else {
        return DolphinMatchResult {
            state: DolphinMatchState::NoVerifiedGameIdAvailable,
            verified_game_id: None,
            verified_revision,
            matching_files: Vec::new(),
            reason: "EmuWiz has no separately verified Dolphin game ID for this archive".into(),
        };
    };
    let Some(game_id) = normalize_verified_game_id(value) else {
        return DolphinMatchResult {
            state: DolphinMatchState::InvalidVerifiedGameId,
            verified_game_id: None,
            verified_revision,
            matching_files: Vec::new(),
            reason: "the supplied verified game ID is not three to six ASCII letters or digits"
                .into(),
        };
    };
    let game_matches: Vec<&DolphinGameIniFile> = inventory
        .files
        .iter()
        .filter(|f| f.game_id_candidate.as_deref() == Some(&game_id))
        .collect();
    if game_matches.is_empty() {
        return DolphinMatchResult {
            state: DolphinMatchState::NoMatchingIniFound,
            verified_game_id: Some(game_id),
            verified_revision,
            matching_files: Vec::new(),
            reason: "no inspected GameSettings filename matches the verified game ID".into(),
        };
    }
    let selected: Vec<&DolphinGameIniFile> = match verified_revision {
        Some(revision) => game_matches
            .iter()
            .copied()
            .filter(|f| {
                f.revision_candidate == Some(revision)
                    // Every real GameSettings filename found on a real
                    // Dolphin installation during this adapter's own audit
                    // (e.g. "NACE01.ini", "PZLE01.ini") omits the "rN"
                    // revision suffix entirely for the common, unmarked
                    // case - Dolphin's own convention treats that as
                    // revision 0, the same way ROM naming omits "(Rev 0)".
                    // Without this, a verified revision-0 archive (the
                    // overwhelming majority of real discs) could never
                    // exact-match its own real, on-disk GameSettings file.
                    || (revision == 0 && f.revision_candidate.is_none())
            })
            .collect(),
        None => game_matches.clone(),
    };
    if verified_revision.is_some() && selected.is_empty() {
        return DolphinMatchResult {
            state: DolphinMatchState::RevisionMismatch,
            verified_game_id: Some(game_id),
            verified_revision,
            matching_files: game_matches.into_iter().map(|f| f.path.clone()).collect(),
            reason: "game ID matched, but no INI matched the verified revision".into(),
        };
    }
    let paths = selected
        .into_iter()
        .map(|f| f.path.clone())
        .collect::<Vec<_>>();
    let state = if paths.len() > 1 {
        DolphinMatchState::MultipleIniFilesForGame
    } else if verified_revision.is_some() {
        DolphinMatchState::ExactGameIdAndRevisionMatch
    } else {
        DolphinMatchState::ExactGameIdMatch
    };
    let reason = match state {
        DolphinMatchState::MultipleIniFilesForGame => {
            "multiple GameSettings INI files match the verified identity"
        }
        DolphinMatchState::ExactGameIdAndRevisionMatch => {
            "one GameSettings INI matches the verified game ID and revision"
        }
        _ => "one GameSettings INI matches the verified game ID",
    };
    DolphinMatchResult {
        state,
        verified_game_id: Some(game_id),
        verified_revision,
        matching_files: paths,
        reason: reason.into(),
    }
}

fn mark_duplicates(inventory: &mut DolphinGameIniInventory) {
    let mut identities: BTreeMap<(String, Option<u16>), usize> = BTreeMap::new();
    let mut filenames: BTreeMap<OsString, usize> = BTreeMap::new();
    let mut hashes: BTreeMap<String, usize> = BTreeMap::new();
    for file in &inventory.files {
        if let Some(id) = &file.game_id_candidate {
            *identities
                .entry((id.clone(), file.revision_candidate))
                .or_default() += 1;
        }
        *filenames
            .entry(file.path.file_name().unwrap_or_default().to_os_string())
            .or_default() += 1;
        *hashes.entry(file.sha256.clone()).or_default() += 1;
    }
    for file in &mut inventory.files {
        file.duplicate_game_identity = file.game_id_candidate.as_ref().is_some_and(|id| {
            identities
                .get(&(id.clone(), file.revision_candidate))
                .copied()
                .unwrap_or_default()
                > 1
        });
        file.duplicate_filename = filenames
            .get(file.path.file_name().unwrap_or_default())
            .copied()
            .unwrap_or_default()
            > 1;
        file.duplicate_content = hashes.get(&file.sha256).copied().unwrap_or_default() > 1;
        if file.duplicate_game_identity {
            push_unique_warning(
                &mut file.warnings,
                DolphinInspectionWarningKind::DuplicateGameIdentity,
            );
        }
        if file.duplicate_filename {
            push_unique_warning(
                &mut file.warnings,
                DolphinInspectionWarningKind::DuplicateFilename,
            );
        }
        if file.duplicate_content {
            push_unique_warning(
                &mut file.warnings,
                DolphinInspectionWarningKind::DuplicateContent,
            );
        }
    }
}

fn is_ini(path: &Path) -> bool {
    path.extension().is_some_and(|value| value == "ini")
}

fn warn(
    inventory: &mut DolphinGameIniInventory,
    kind: DolphinInspectionWarningKind,
    path: &Path,
    detail: impl Into<String>,
) {
    inventory.complete = false;
    inventory.warnings.push(DolphinInspectionWarning {
        kind,
        path: path.to_path_buf(),
        detail: detail.into(),
    });
}

fn push_unique_warning(
    warnings: &mut Vec<DolphinInspectionWarningKind>,
    kind: DolphinInspectionWarningKind,
) {
    if !warnings.contains(&kind) {
        warnings.push(kind);
    }
}

#[cfg(unix)]
fn directory_identity(metadata: &fs::Metadata) -> Option<DolphinDirectoryIdentity> {
    metadata.is_dir().then(|| DolphinDirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn directory_identity(_metadata: &fs::Metadata) -> Option<DolphinDirectoryIdentity> {
    None
}

// -------------------------------------------------------------------------
// Modern local inspection
//
// The original Dolphin GameSettings API above remains the compatibility API
// used by the cheat workflow.  The types below deliberately add a narrow,
// read-only health and selected-game view comparable with the newer local
// adapters.  They do not emit preservation evidence: a Dolphin game ID is a
// useful association key only after core has independently established it.

const DOLPHIN_LOCAL_MAX_UNKNOWN_SETTINGS: usize = 256;
const DOLPHIN_LOCAL_MAX_LINES: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinLocalInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinTargetPlatform {
    GameCube,
    Wii,
    Other,
}

/// Container context supplied by core.  This enum is intentionally not
/// inferred from a filename: `.iso`, `.gcm`, `.wbfs`, `.rvz`, `.wia`, and
/// `.chd` remain different representations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinDiscFormat {
    Iso,
    Gcm,
    Wbfs,
    Rvz,
    Wia,
    Chd,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinDiscContext {
    pub disc_number: u8,
    pub format: DolphinDiscFormat,
    pub representation: Representation,
    pub claim: ClaimType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinExecutable {
    pub path: PathBuf,
    pub installation_type: DolphinLocalInstallationType,
    /// This adapter parses only text supplied by an authorized outer probe;
    /// discovery never executes a Dolphin binary.
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLocalProfile {
    pub profile_id: String,
    pub installation_type: DolphinLocalInstallationType,
    pub configuration_root: PathBuf,
    pub data_root: PathBuf,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<DolphinExecutable>,
    pub dolphin_ini_path: PathBuf,
    pub graphics_ini_path: PathBuf,
    pub game_settings_path: PathBuf,
    pub textures_path: PathBuf,
    pub memory_cards_path: PathBuf,
    pub wii_data_path: PathBuf,
    pub save_states_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLocalProfileDiscovery {
    pub profiles: Vec<DolphinLocalProfile>,
    pub complete: bool,
}

/// Discovery stays limited to known XDG/Flatpak paths plus exact caller
/// supplied portable/custom locations.  It never walks a home directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLocalDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
    /// Present whenever `DOLPHIN_EMU_USERPATH` is set in the environment.
    /// Any value overrides Dolphin's default directory resolution, so its
    /// mere presence - not its content - is what launch-binding cares about.
    pub dolphin_emu_userpath_override: Option<PathBuf>,
}

impl DolphinLocalDiscoveryRoots {
    pub fn from_environment() -> Result<Self, DolphinDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(DolphinDiscoveryError::HomeUnavailable)?;
        let xdg_config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let xdg_data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let appimage_directory = env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .and_then(|path| path.parent().map(Path::to_path_buf));
        Ok(Self {
            home,
            xdg_config_home,
            xdg_data_home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory,
            dolphin_emu_userpath_override: env::var_os("DOLPHIN_EMU_USERPATH").map(PathBuf::from),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinGameIdMapping {
    /// A GameCube/Wii header identity core independently verified.  It is
    /// association metadata, not a Redump or container-hash equivalence.
    CoreVerifiedMetadata,
    EmulatorMetadataOnly,
    ConflictingEmulatorMetadata,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DolphinGameRequest {
    /// Core's canonical platform is retained as supplied; Dolphin never
    /// rewrites it from a profile, filename, or game ID.
    pub canonical_platform: Option<String>,
    pub target_platform: Option<DolphinTargetPlatform>,
    /// A core-verified disc-header Game ID, never a preservation match.
    pub verified_game_id: Option<String>,
    pub verified_revision: Option<u16>,
    /// Metadata reported by Dolphin, a frontend, or a caller.  It cannot
    /// create verified identity and is retained only for context.
    pub emulator_game_id: Option<String>,
    pub disc_contexts: Vec<DolphinDiscContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DolphinSettings {
    pub renderer: Option<String>,
    pub internal_resolution: Option<String>,
    pub widescreen: Option<bool>,
    pub vsync: Option<bool>,
    pub texture_filtering: Option<String>,
    pub texture_cache: Option<bool>,
    pub audio_backend: Option<String>,
    pub cheats_enabled: Option<bool>,
    pub controller_profile_present: Option<bool>,
    pub unknown: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinConfigInspection {
    pub dolphin_ini_path: PathBuf,
    pub graphics_ini_path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub settings: DolphinSettings,
    pub warnings: Vec<DolphinInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinGameSettingsInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub definition_count: usize,
    pub enabled_count: usize,
    pub warnings: Vec<DolphinInspectionWarning>,
}

/// Code definitions in a local Dolphin GameSettings file.  The file path is
/// association context only; it cannot establish a game identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinCheatInventory {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub definitions: usize,
    pub enabled_definitions: usize,
    pub warnings: Vec<DolphinInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTextureInventory {
    pub path: PathBuf,
    pub present: bool,
    pub file_count: usize,
    pub total_size_bytes: u64,
    pub complete: bool,
    pub warnings: Vec<DolphinInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinSaveInventory {
    /// These are candidates only.  A memory-card image or Wii NAND directory
    /// is not proof that it belongs to the selected game.
    pub candidate_paths: Vec<PathBuf>,
    pub wii_data_present: bool,
    pub complete: bool,
    pub warnings: Vec<DolphinInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinHealth {
    pub detected: bool,
    pub config_readable: bool,
    pub game_settings_available: bool,
    pub memory_cards_present: bool,
    pub wii_data_present: bool,
    pub game_id_mapping: DolphinGameIdMapping,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinGameInspection {
    pub game_id: Option<String>,
    pub emulator_game_id_context: Option<String>,
    pub game_id_mapping: DolphinGameIdMapping,
    pub identity_mismatch: Option<String>,
    pub canonical_platform: Option<String>,
    pub target_platform: Option<DolphinTargetPlatform>,
    pub disc_contexts: Vec<DolphinDiscContext>,
    pub global_config: DolphinConfigInspection,
    pub game_settings: Option<DolphinGameSettingsInspection>,
    pub cheats: Option<DolphinCheatInventory>,
    pub textures: Option<DolphinTextureInventory>,
    pub saves: DolphinSaveInventory,
    pub health: DolphinHealth,
}

#[derive(Clone)]
struct DolphinLocalCandidate {
    installation_type: DolphinLocalInstallationType,
    configuration_root: PathBuf,
    data_root: PathBuf,
}

pub fn discover_dolphin_local_profiles(
    roots: &DolphinLocalDiscoveryRoots,
) -> DolphinLocalProfileDiscovery {
    let flatpak_root = roots.home.join(".var/app").join(FLATPAK_APP_ID);
    let mut candidates = vec![
        DolphinLocalCandidate {
            installation_type: DolphinLocalInstallationType::Native,
            configuration_root: roots.xdg_config_home.join("dolphin-emu"),
            data_root: roots.xdg_data_home.join("dolphin-emu"),
        },
        DolphinLocalCandidate {
            installation_type: DolphinLocalInstallationType::FlatpakUser,
            configuration_root: flatpak_root.join("config/dolphin-emu"),
            data_root: flatpak_root.join("data/dolphin-emu"),
        },
    ];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|root| DolphinLocalCandidate {
                installation_type: DolphinLocalInstallationType::Portable,
                configuration_root: root.clone(),
                data_root: root,
            }),
    );
    if let Some(directory) = &roots.appimage_directory {
        candidates.push(DolphinLocalCandidate {
            installation_type: DolphinLocalInstallationType::Portable,
            configuration_root: directory.join("User"),
            data_root: directory.join("User"),
        });
    }
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|root| DolphinLocalCandidate {
                installation_type: DolphinLocalInstallationType::Explicit,
                configuration_root: root.clone(),
                data_root: root,
            }),
    );
    candidates.sort_by(|left, right| {
        left.configuration_root
            .cmp(&right.configuration_root)
            .then_with(|| left.data_root.cmp(&right.data_root))
    });
    candidates.dedup_by(|left, right| {
        left.configuration_root == right.configuration_root && left.data_root == right.data_root
    });
    let executables = discover_dolphin_local_executables(roots);
    DolphinLocalProfileDiscovery {
        profiles: candidates
            .into_iter()
            .take(DOLPHIN_MAX_PROFILES)
            .map(|candidate| dolphin_local_profile(candidate, &executables))
            .collect(),
        complete: true,
    }
}

/// Parses supplied version output only; it never launches Dolphin.
pub fn parse_dolphin_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        let index = lower.find("dolphin")?;
        let tail = line[index + "dolphin".len()..]
            .trim_start_matches(|value: char| {
                value.is_ascii_whitespace() || value == ':' || value == '-'
            })
            .trim_start_matches("Emulator")
            .trim();
        let version: String = tail
            .chars()
            .take_while(|value| value.is_ascii_alphanumeric() || matches!(value, '.' | '-'))
            .collect();
        (!version.is_empty() && version.chars().any(|value| value.is_ascii_digit()))
            .then_some(version)
    })
}

pub fn inspect_dolphin_local_game(
    profile: &DolphinLocalProfile,
    request: &DolphinGameRequest,
) -> DolphinGameInspection {
    let global_config = inspect_dolphin_local_config(profile);
    let (game_id, game_id_mapping, identity_mismatch) = dolphin_local_game_id(request);
    let game_settings = game_id.as_deref().map(|id| {
        let path = dolphin_local_game_settings_path(profile, id, request.verified_revision);
        inspect_dolphin_local_game_settings(&path)
    });
    let cheats = game_settings
        .as_ref()
        .map(|settings| DolphinCheatInventory {
            path: settings.path.clone(),
            exists: settings.exists,
            readable: settings.readable,
            definitions: settings.definition_count,
            enabled_definitions: settings.enabled_count,
            warnings: settings.warnings.clone(),
        });
    let textures = game_id
        .as_deref()
        .map(|id| inspect_dolphin_local_textures(&profile.textures_path.join(id)));
    let saves = inspect_dolphin_local_saves(profile);
    let mut warnings: Vec<String> = profile.blocker.iter().cloned().collect();
    warnings.extend(
        global_config
            .warnings
            .iter()
            .map(|warning| warning.detail.clone()),
    );
    if let Some(mismatch) = &identity_mismatch {
        warnings.push(mismatch.clone());
    }
    let health = DolphinHealth {
        detected: profile.eligible || !profile.executable_candidates.is_empty(),
        config_readable: global_config.readable,
        game_settings_available: is_real_directory_local(&profile.game_settings_path),
        memory_cards_present: saves
            .candidate_paths
            .iter()
            .any(|path| path.starts_with(&profile.memory_cards_path)),
        wii_data_present: saves.wii_data_present,
        game_id_mapping,
        warnings,
    };
    DolphinGameInspection {
        game_id,
        emulator_game_id_context: request.emulator_game_id.clone(),
        game_id_mapping,
        identity_mismatch,
        canonical_platform: request.canonical_platform.clone(),
        target_platform: request.target_platform,
        disc_contexts: request.disc_contexts.iter().take(32).cloned().collect(),
        global_config,
        game_settings,
        cheats,
        textures,
        saves,
        health,
    }
}

fn dolphin_local_profile(
    candidate: DolphinLocalCandidate,
    executables: &[DolphinExecutable],
) -> DolphinLocalProfile {
    let dolphin_ini_path = candidate.configuration_root.join("Dolphin.ini");
    let blocker =
        if !candidate.configuration_root.is_absolute() || !candidate.data_root.is_absolute() {
            Some("configuration and data roots must be absolute".to_string())
        } else if !is_real_directory_local(&candidate.configuration_root) {
            Some("configuration directory is absent, unsafe, or not a real directory".to_string())
        } else if !is_regular_file_local(&dolphin_ini_path) {
            Some("Dolphin.ini was not found as a regular file".to_string())
        } else {
            None
        };
    DolphinLocalProfile {
        profile_id: format!("dolphin:{}", candidate.configuration_root.display()),
        installation_type: candidate.installation_type,
        configuration_root: candidate.configuration_root.clone(),
        data_root: candidate.data_root.clone(),
        eligible: blocker.is_none(),
        blocker,
        executable_candidates: executables.to_vec(),
        dolphin_ini_path,
        graphics_ini_path: candidate.configuration_root.join("GFX.ini"),
        game_settings_path: candidate.data_root.join("GameSettings"),
        textures_path: candidate.data_root.join("Load/Textures"),
        memory_cards_path: candidate.data_root.join("GC"),
        wii_data_path: candidate.data_root.join("Wii"),
        save_states_path: candidate.data_root.join("StateSaves"),
    }
}

fn dolphin_local_game_settings_path(
    profile: &DolphinLocalProfile,
    game_id: &str,
    revision: Option<u16>,
) -> PathBuf {
    if let Some(revision) = revision {
        let revision_path = profile
            .game_settings_path
            .join(format!("{game_id}r{revision}.ini"));
        if is_regular_file_local(&revision_path) {
            return revision_path;
        }
    }
    profile.game_settings_path.join(format!("{game_id}.ini"))
}

fn discover_dolphin_local_executables(
    roots: &DolphinLocalDiscoveryRoots,
) -> Vec<DolphinExecutable> {
    let mut paths = roots.explicit_executables.clone();
    if let Some(directory) = &roots.appimage_directory {
        paths.extend([
            directory.join("Dolphin.AppImage"),
            directory.join("dolphin-emu.AppImage"),
            directory.join("dolphin-emu"),
        ]);
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path).take(128) {
            paths.extend([directory.join("dolphin-emu"), directory.join("dolphin-qt2")]);
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| is_regular_file_local(path))
        .map(|path| DolphinExecutable {
            installation_type: if roots.explicit_executables.contains(&path) {
                DolphinLocalInstallationType::Explicit
            } else if roots
                .appimage_directory
                .as_ref()
                .is_some_and(|directory| path.starts_with(directory))
            {
                DolphinLocalInstallationType::Portable
            } else {
                DolphinLocalInstallationType::Native
            },
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|output| parse_dolphin_version(output)),
            path,
        })
        .collect()
}

// --- Native launch binding -------------------------------------------------
//
// A profile's `executable_candidates` alone do not authorize a launch: the
// list is populated once for every discovered profile regardless of which
// installation it actually belongs to, and `data_root` is never a valid
// Dolphin `-u` argument on its own (Dolphin derives `Config/` beneath
// whatever `-u` receives, which would silently relocate a split XDG
// configuration root). This section proves, freshly and read-only, exactly
// which executable belongs to a profile and whether the profile's roots
// correspond to Dolphin's *default* directory resolution (no `-u`) or to a
// single genuine Dolphin user root (`-u <root>`). It never launches Dolphin,
// writes configuration, or creates directories.

/// How the eventual launch command should select Dolphin's user directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinUserDirectoryMode {
    /// No `-u` argument; Dolphin resolves its own default directories.
    DefaultNative,
    /// `-u <root>`; Dolphin derives `Config/`, `GameSettings/`, `GC/`,
    /// `Wii/`, etc. beneath this single verified root.
    ExplicitRoot(PathBuf),
}

/// A freshly proven executable/profile pairing, safe to use as the first two
/// (or three) tokens of a Dolphin native launch command. Callers must treat
/// this as re-derivable state, not a cache: call
/// [`resolve_dolphin_native_launch_binding`] again at the moment of launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinNativeLaunchBinding {
    pub executable: PathBuf,
    pub user_directory_mode: DolphinUserDirectoryMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinLaunchBlockerKind {
    /// Flatpak, or a portable/AppImage profile that cannot be proven
    /// native-equivalent by this adapter.
    UnsupportedInstallationType,
    /// More than one viable executable candidate matches the profile and no
    /// authority distinguishes them.
    AmbiguousExecutable,
    /// No candidate executable exists on disk.
    ExecutableMissing,
    /// A candidate executable exists but is a symlink or not a regular file.
    ExecutableUnsafe,
    /// A candidate executable exists as a regular file but lacks the
    /// executable permission bit.
    ExecutableNotExecutable,
    /// The profile's configuration/data roots (or Dolphin.ini) no longer
    /// match what discovery originally observed.
    ProfileRootMismatch,
    /// The profile's roots do not match Dolphin's current default XDG
    /// resolution, so `DefaultNative` cannot be proven.
    DefaultResolutionMismatch,
    /// The candidate explicit root is not an absolute, existing,
    /// non-symlinked directory with `Config/Dolphin.ini` beneath it.
    ExplicitRootInvalid,
    /// An environment variable (`DOLPHIN_EMU_USERPATH`) would override
    /// Dolphin's default directory resolution.
    EnvironmentOverridePresent,
    /// A `portable.txt` marker or legacy `~/.dolphin-emu` directory would
    /// change which directories Dolphin actually resolves to.
    PortableOrLegacyLayoutConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLaunchBlocker {
    pub kind: DolphinLaunchBlockerKind,
    pub detail: String,
}

fn launch_blocker(
    kind: DolphinLaunchBlockerKind,
    detail: impl Into<String>,
) -> DolphinLaunchBlocker {
    DolphinLaunchBlocker {
        kind,
        detail: detail.into(),
    }
}

/// Freshly revalidates `profile` against `roots` and either proves a launch
/// binding or returns a structured blocker. Pure and read-only: it inspects
/// filesystem metadata and the supplied environment-derived roots only, and
/// never spawns a process, writes Dolphin configuration, or creates
/// directories. Safe - and intended - to call again at future launch time.
pub fn resolve_dolphin_native_launch_binding(
    profile: &DolphinLocalProfile,
    roots: &DolphinLocalDiscoveryRoots,
) -> Result<DolphinNativeLaunchBinding, DolphinLaunchBlocker> {
    if !profile.eligible {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ProfileRootMismatch,
            "profile is not eligible",
        ));
    }
    match profile.installation_type {
        DolphinLocalInstallationType::Native => resolve_default_native_binding(profile, roots),
        DolphinLocalInstallationType::Explicit => resolve_explicit_root_binding(profile),
        DolphinLocalInstallationType::FlatpakUser => Err(launch_blocker(
            DolphinLaunchBlockerKind::UnsupportedInstallationType,
            "Flatpak Dolphin installations are not supported by this native launch binding",
        )),
        DolphinLocalInstallationType::Portable => Err(launch_blocker(
            DolphinLaunchBlockerKind::UnsupportedInstallationType,
            "portable/AppImage Dolphin profiles cannot be proven native-equivalent; failing closed",
        )),
    }
}

fn resolve_default_native_binding(
    profile: &DolphinLocalProfile,
    roots: &DolphinLocalDiscoveryRoots,
) -> Result<DolphinNativeLaunchBinding, DolphinLaunchBlocker> {
    let expected_config = roots.xdg_config_home.join("dolphin-emu");
    let expected_data = roots.xdg_data_home.join("dolphin-emu");
    if profile.configuration_root != expected_config || profile.data_root != expected_data {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::DefaultResolutionMismatch,
            format!(
                "profile roots {} / {} do not match the current default XDG resolution {} / {}",
                profile.configuration_root.display(),
                profile.data_root.display(),
                expected_config.display(),
                expected_data.display(),
            ),
        ));
    }
    if let Some(userpath) = &roots.dolphin_emu_userpath_override {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::EnvironmentOverridePresent,
            format!(
                "DOLPHIN_EMU_USERPATH is set to {} and would override the default directory resolution",
                userpath.display()
            ),
        ));
    }
    let legacy_root = roots.home.join(".dolphin-emu");
    if is_real_directory_local(&legacy_root) {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::PortableOrLegacyLayoutConflict,
            format!(
                "legacy Dolphin user directory {} exists and takes precedence over the XDG default",
                legacy_root.display()
            ),
        ));
    }
    if !is_real_directory_local(&profile.configuration_root)
        || !is_real_directory_local(&profile.data_root)
        || !is_regular_file_local(&profile.dolphin_ini_path)
    {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ProfileRootMismatch,
            "configuration root, data root, or Dolphin.ini no longer match the discovered profile",
        ));
    }
    let executable = resolve_native_executable(profile)?;
    if let Some(blocker) = portable_txt_conflict(&executable) {
        return Err(blocker);
    }
    Ok(DolphinNativeLaunchBinding {
        executable,
        user_directory_mode: DolphinUserDirectoryMode::DefaultNative,
    })
}

fn resolve_explicit_root_binding(
    profile: &DolphinLocalProfile,
) -> Result<DolphinNativeLaunchBinding, DolphinLaunchBlocker> {
    if profile.configuration_root != profile.data_root {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ExplicitRootInvalid,
            "configuration and data roots differ; a single Dolphin user root could not be proven",
        ));
    }
    let root = profile.configuration_root.clone();
    if !root.is_absolute() || root.parent().is_none() {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ExplicitRootInvalid,
            "root is not an absolute, non-filesystem-root path",
        ));
    }
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(launch_blocker(
                DolphinLaunchBlockerKind::ExplicitRootInvalid,
                format!("{} is a symlink", root.display()),
            ));
        }
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(launch_blocker(
                DolphinLaunchBlockerKind::ExplicitRootInvalid,
                format!("{} is not a directory", root.display()),
            ));
        }
        Err(_) => {
            return Err(launch_blocker(
                DolphinLaunchBlockerKind::ExplicitRootInvalid,
                format!("{} does not exist", root.display()),
            ));
        }
    }
    // Never infer a single-root layout from `data_root`/`GameSettings`
    // evidence alone: a genuine `-u <root>` layout must show Dolphin's own
    // `Config/Dolphin.ini` beneath the candidate root.
    let config_dir = root.join("Config");
    if !is_real_directory_local(&config_dir) {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ExplicitRootInvalid,
            format!(
                "{} does not exist; a single Dolphin user root layout could not be proven",
                config_dir.display()
            ),
        ));
    }
    if !is_regular_file_local(&config_dir.join("Dolphin.ini")) {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ExplicitRootInvalid,
            format!(
                "{} was not found as a regular file",
                config_dir.join("Dolphin.ini").display()
            ),
        ));
    }
    let executable = resolve_native_executable(profile)?;
    Ok(DolphinNativeLaunchBinding {
        executable,
        user_directory_mode: DolphinUserDirectoryMode::ExplicitRoot(root),
    })
}

/// `portable.txt` next to the executable forces Dolphin into portable mode,
/// which redirects directory resolution away from the profile just proven.
fn portable_txt_conflict(executable: &Path) -> Option<DolphinLaunchBlocker> {
    let marker = executable.parent()?.join("portable.txt");
    is_regular_file_local(&marker).then(|| {
        launch_blocker(
            DolphinLaunchBlockerKind::PortableOrLegacyLayoutConflict,
            format!(
                "{} exists next to the executable and would force portable mode",
                marker.display()
            ),
        )
    })
}

/// Binds exactly one executable to `profile`, matching candidates only by
/// the profile's own installation type - `executable_candidates` is shared
/// across every discovered profile and is not otherwise scoped per-profile.
/// Never falls back to a hard-coded name: a profile with no matching,
/// verified-safe candidate is always blocked.
fn resolve_native_executable(
    profile: &DolphinLocalProfile,
) -> Result<PathBuf, DolphinLaunchBlocker> {
    let matching: Vec<&DolphinExecutable> = profile
        .executable_candidates
        .iter()
        .filter(|candidate| candidate.installation_type == profile.installation_type)
        .collect();
    if matching.is_empty() {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ExecutableMissing,
            "no discovered executable is associated with this profile's installation type",
        ));
    }
    let mut valid = Vec::new();
    let mut last_error = None;
    for candidate in matching {
        match validate_native_executable(&candidate.path) {
            Ok(()) => valid.push(candidate.path.clone()),
            Err(error) => last_error = Some(error),
        }
    }
    match valid.len() {
        0 => Err(last_error.expect("at least one candidate was inspected")),
        1 => Ok(valid.into_iter().next().expect("length checked above")),
        count => Err(launch_blocker(
            DolphinLaunchBlockerKind::AmbiguousExecutable,
            format!(
                "{count} viable executables match this profile and none is distinguished as authoritative"
            ),
        )),
    }
}

fn validate_native_executable(path: &Path) -> Result<(), DolphinLaunchBlocker> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        launch_blocker(
            DolphinLaunchBlockerKind::ExecutableMissing,
            format!("{} does not exist", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ExecutableUnsafe,
            format!("{} is a symlink", path.display()),
        ));
    }
    if !metadata.is_file() {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ExecutableUnsafe,
            format!("{} is not a regular file", path.display()),
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(launch_blocker(
            DolphinLaunchBlockerKind::ExecutableNotExecutable,
            format!("{} is not executable", path.display()),
        ));
    }
    Ok(())
}

fn dolphin_local_game_id(
    request: &DolphinGameRequest,
) -> (Option<String>, DolphinGameIdMapping, Option<String>) {
    let verified = request
        .verified_game_id
        .as_deref()
        .and_then(normalize_verified_game_id);
    let emulator = request
        .emulator_game_id
        .as_deref()
        .and_then(normalize_verified_game_id);
    match (verified, emulator) {
        (Some(verified), Some(emulator)) if verified != emulator => (
            Some(verified.clone()),
            DolphinGameIdMapping::ConflictingEmulatorMetadata,
            Some(format!(
                "Dolphin metadata game ID {emulator} conflicts with core-verified disc-header game ID {verified}"
            )),
        ),
        (Some(verified), _) => (
            Some(verified),
            DolphinGameIdMapping::CoreVerifiedMetadata,
            None,
        ),
        (None, Some(emulator)) => (
            Some(emulator),
            DolphinGameIdMapping::EmulatorMetadataOnly,
            None,
        ),
        (None, None) => (None, DolphinGameIdMapping::Unavailable, None),
    }
}

fn inspect_dolphin_local_config(profile: &DolphinLocalProfile) -> DolphinConfigInspection {
    let mut warnings = Vec::new();
    let dolphin = read_dolphin_local_text(
        &profile.dolphin_ini_path,
        DOLPHIN_LOCAL_MAX_CONFIG_BYTES,
        &mut warnings,
    );
    let graphics = read_dolphin_local_text(
        &profile.graphics_ini_path,
        DOLPHIN_LOCAL_MAX_CONFIG_BYTES,
        &mut warnings,
    );
    let mut settings = DolphinSettings::default();
    if let Some(text) = dolphin.as_deref() {
        parse_dolphin_local_ini(
            text,
            &profile.dolphin_ini_path,
            &mut settings,
            &mut warnings,
        );
    }
    if let Some(text) = graphics.as_deref() {
        parse_dolphin_local_ini(
            text,
            &profile.graphics_ini_path,
            &mut settings,
            &mut warnings,
        );
    }
    DolphinConfigInspection {
        dolphin_ini_path: profile.dolphin_ini_path.clone(),
        graphics_ini_path: profile.graphics_ini_path.clone(),
        exists: profile.dolphin_ini_path.exists() || profile.graphics_ini_path.exists(),
        readable: dolphin.is_some(),
        settings,
        warnings,
    }
}

fn parse_dolphin_local_ini(
    text: &str,
    path: &Path,
    settings: &mut DolphinSettings,
    warnings: &mut Vec<DolphinInspectionWarning>,
) {
    let mut section = String::new();
    for (index, raw) in text.lines().enumerate() {
        if index >= DOLPHIN_LOCAL_MAX_LINES {
            push_dolphin_local_warning(
                warnings,
                DolphinInspectionWarningKind::LineCountLimitReached,
                path,
                format!("INI parsing stopped at {DOLPHIN_LOCAL_MAX_LINES} lines"),
            );
            break;
        }
        if raw.len() > DOLPHIN_MAX_LINE_BYTES {
            push_dolphin_local_warning(
                warnings,
                DolphinInspectionWarningKind::LineTooLong,
                path,
                format!("INI line exceeds {DOLPHIN_MAX_LINE_BYTES} bytes"),
            );
            continue;
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if let Some(section_name) = line.strip_suffix(']') {
                section = section_name[1..].trim().to_ascii_lowercase();
            } else {
                push_dolphin_local_warning(
                    warnings,
                    DolphinInspectionWarningKind::MalformedIni,
                    path,
                    "INI section does not end with ']'",
                );
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            push_dolphin_local_warning(
                warnings,
                DolphinInspectionWarningKind::MalformedIni,
                path,
                "INI setting has no '=' separator",
            );
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        let boolean = parse_dolphin_local_bool(value);
        match key.as_str() {
            "backend" | "renderer" => settings.renderer = Some(value.to_string()),
            "internalresolution" | "internal_resolution" | "efbresolution" => {
                settings.internal_resolution = Some(value.to_string())
            }
            "widescreenhack" | "widescreen_hack" | "widescreen" => settings.widescreen = boolean,
            "vsync" | "vsyncenabled" => settings.vsync = boolean,
            "texturefiltering" | "texture_filtering" => {
                settings.texture_filtering = Some(value.to_string())
            }
            "texturecache" | "texture_cache" => settings.texture_cache = boolean,
            "backendname" | "audio_backend" => settings.audio_backend = Some(value.to_string()),
            "enablecheats" | "enable_cheats" => settings.cheats_enabled = boolean,
            "profile" | "controllerprofile" | "controller_profile" => {
                settings.controller_profile_present = Some(!value.is_empty())
            }
            _ if settings.unknown.len() < DOLPHIN_LOCAL_MAX_UNKNOWN_SETTINGS => {
                settings
                    .unknown
                    .insert(format!("{section}.{key}"), value.to_string());
            }
            _ => {}
        }
    }
}

fn inspect_dolphin_local_game_settings(path: &Path) -> DolphinGameSettingsInspection {
    let mut warnings = Vec::new();
    let exists = path.exists();
    let Some(text) = read_dolphin_local_text(path, DOLPHIN_MAX_GAME_INI_BYTES, &mut warnings)
    else {
        return DolphinGameSettingsInspection {
            path: path.to_path_buf(),
            exists,
            readable: false,
            definition_count: 0,
            enabled_count: 0,
            warnings,
        };
    };
    let mut parsed = ParsedIni::default();
    let mut local_warnings = Vec::new();
    parse_ini_text(&text, &mut parsed, &mut local_warnings);
    for kind in local_warnings {
        push_dolphin_local_warning(&mut warnings, kind, path, "GameSettings parse warning");
    }
    DolphinGameSettingsInspection {
        path: path.to_path_buf(),
        exists,
        readable: true,
        definition_count: parsed.frame.len()
            + parsed.ar.len()
            + parsed.gecko.len()
            + parsed.riivolution.len(),
        enabled_count: parsed.frame_enabled.len()
            + parsed.ar_enabled.len()
            + parsed.gecko_enabled.len()
            + parsed.riivolution_enabled.len(),
        warnings,
    }
}

fn inspect_dolphin_local_textures(path: &Path) -> DolphinTextureInventory {
    let mut output = DolphinTextureInventory {
        path: path.to_path_buf(),
        present: is_real_directory_local(path),
        file_count: 0,
        total_size_bytes: 0,
        complete: true,
        warnings: Vec::new(),
    };
    if !output.present {
        return output;
    }
    let mut todo = VecDeque::from([(path.to_path_buf(), 0usize)]);
    let mut entries_seen = 0usize;
    while let Some((directory, depth)) = todo.pop_front() {
        let Ok(entries) = fs::read_dir(&directory) else {
            output.complete = false;
            push_dolphin_local_warning(
                &mut output.warnings,
                DolphinInspectionWarningKind::UnreadablePath,
                &directory,
                "texture directory cannot be read",
            );
            continue;
        };
        for entry in entries.flatten() {
            if entries_seen >= DOLPHIN_MAX_ENTRIES_VISITED {
                output.complete = false;
                push_dolphin_local_warning(
                    &mut output.warnings,
                    DolphinInspectionWarningKind::EntryLimitReached,
                    &directory,
                    format!("texture traversal stopped at {DOLPHIN_MAX_ENTRIES_VISITED} entries"),
                );
                return output;
            }
            entries_seen += 1;
            let entry_path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&entry_path) else {
                output.complete = false;
                continue;
            };
            if metadata.file_type().is_symlink() {
                push_dolphin_local_warning(
                    &mut output.warnings,
                    DolphinInspectionWarningKind::SymlinkSkipped,
                    &entry_path,
                    "symlink was not followed",
                );
            } else if metadata.is_file() {
                if output.file_count >= DOLPHIN_LOCAL_MAX_TEXTURE_FILES {
                    output.complete = false;
                    push_dolphin_local_warning(
                        &mut output.warnings,
                        DolphinInspectionWarningKind::FileCountLimitReached,
                        &entry_path,
                        format!(
                            "texture traversal stopped at {DOLPHIN_LOCAL_MAX_TEXTURE_FILES} files"
                        ),
                    );
                    return output;
                }
                output.file_count += 1;
                output.total_size_bytes = output.total_size_bytes.saturating_add(metadata.len());
            } else if metadata.is_dir() {
                if depth >= DOLPHIN_LOCAL_MAX_TEXTURE_DEPTH {
                    output.complete = false;
                    push_dolphin_local_warning(
                        &mut output.warnings,
                        DolphinInspectionWarningKind::EntryLimitReached,
                        &entry_path,
                        format!(
                            "texture traversal depth exceeds {DOLPHIN_LOCAL_MAX_TEXTURE_DEPTH}"
                        ),
                    );
                } else {
                    todo.push_back((entry_path, depth + 1));
                }
            }
        }
    }
    output
}

fn inspect_dolphin_local_saves(profile: &DolphinLocalProfile) -> DolphinSaveInventory {
    let mut output = DolphinSaveInventory {
        candidate_paths: Vec::new(),
        wii_data_present: is_real_directory_local(&profile.wii_data_path),
        complete: true,
        warnings: Vec::new(),
    };
    for directory in [&profile.memory_cards_path, &profile.save_states_path] {
        collect_dolphin_local_save_candidates(directory, &mut output);
    }
    output
}

fn collect_dolphin_local_save_candidates(path: &Path, output: &mut DolphinSaveInventory) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if output.candidate_paths.len() >= DOLPHIN_LOCAL_MAX_SAVE_CANDIDATES {
            output.complete = false;
            push_dolphin_local_warning(
                &mut output.warnings,
                DolphinInspectionWarningKind::FileCountLimitReached,
                path,
                format!("save inventory stopped at {DOLPHIN_LOCAL_MAX_SAVE_CANDIDATES} candidates"),
            );
            return;
        }
        let entry_path = entry.path();
        match fs::symlink_metadata(&entry_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                push_dolphin_local_warning(
                    &mut output.warnings,
                    DolphinInspectionWarningKind::SymlinkSkipped,
                    &entry_path,
                    "symlink was not followed",
                );
            }
            Ok(metadata) if metadata.is_file() => output.candidate_paths.push(entry_path),
            Ok(_) => {}
            Err(_) => output.complete = false,
        }
    }
}

fn read_dolphin_local_text(
    path: &Path,
    limit: u64,
    warnings: &mut Vec<DolphinInspectionWarning>,
) -> Option<String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            push_dolphin_local_warning(
                warnings,
                DolphinInspectionWarningKind::SymlinkSkipped,
                path,
                "symlink was not followed",
            );
            return None;
        }
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            push_dolphin_local_warning(
                warnings,
                DolphinInspectionWarningKind::SpecialFileSkipped,
                path,
                "path is not a regular file",
            );
            return None;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            push_dolphin_local_warning(
                warnings,
                DolphinInspectionWarningKind::UnreadablePath,
                path,
                format!("path cannot be inspected: {error}"),
            );
            return None;
        }
    };
    if metadata.len() > limit {
        push_dolphin_local_warning(
            warnings,
            DolphinInspectionWarningKind::FileTooLarge,
            path,
            format!("file exceeds {limit} bytes"),
        );
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) => {
            push_dolphin_local_warning(
                warnings,
                DolphinInspectionWarningKind::UnreadablePath,
                path,
                format!("path cannot be opened safely: {error}"),
            );
            return None;
        }
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 != metadata.len()
    {
        push_dolphin_local_warning(
            warnings,
            DolphinInspectionWarningKind::UnreadablePath,
            path,
            "file changed or could not be read completely",
        );
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) => {
            push_dolphin_local_warning(
                warnings,
                DolphinInspectionWarningKind::InvalidUtf8,
                path,
                "file is not valid UTF-8",
            );
            Some(String::from_utf8_lossy(error.as_bytes()).into_owned())
        }
    }
}

fn is_regular_file_local(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn is_real_directory_local(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

fn parse_dolphin_local_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn push_dolphin_local_warning(
    warnings: &mut Vec<DolphinInspectionWarning>,
    kind: DolphinInspectionWarningKind,
    path: &Path,
    detail: impl Into<String>,
) {
    warnings.push(DolphinInspectionWarning {
        kind,
        path: path.to_path_buf(),
        detail: detail.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "archivefs-dolphin-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn roots(root: &Path) -> DolphinProfileDiscoveryRoots {
        DolphinProfileDiscoveryRoots {
            home: root.join("home"),
            xdg_config_home: root.join("config"),
            xdg_data_home: root.join("data"),
            flatpak_system_root: root.join("system"),
            explicit_configuration_roots: Vec::new(),
            running_commands: Vec::new(),
            selected_launch_commands: Vec::new(),
            selected_executable: None,
        }
    }

    fn make_profile(path: &Path) -> PathBuf {
        fs::create_dir_all(path.join("GameSettings")).unwrap();
        fs::write(path.join("Dolphin.ini"), b"[Core]\n").unwrap();
        path.to_path_buf()
    }

    fn eligible(path: &Path) -> DolphinProfile {
        let mut discovery_roots = roots(path.parent().unwrap());
        discovery_roots
            .explicit_configuration_roots
            .push(path.to_path_buf());
        discover_dolphin_profiles(&discovery_roots)
            .unwrap()
            .profiles
            .into_iter()
            .find(|p| p.configuration_path == path)
            .unwrap()
    }

    #[test]
    fn discovers_native_flatpak_and_exact_profiles() {
        let root = fixture("discovery");
        make_profile(&root.join("config/dolphin-emu"));
        make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu"));
        let explicit = make_profile(&root.join("portable"));
        let mut discovery_roots = roots(&root);
        discovery_roots.explicit_configuration_roots.push(explicit);
        let discovery = discover_dolphin_profiles(&discovery_roots).unwrap();
        assert_eq!(discovery.profiles.len(), 3);
        assert!(discovery.profiles.iter().all(|p| p.eligible));
        fs::remove_dir_all(root).unwrap();
    }

    fn command(executable: &str, arguments: &[&str]) -> DolphinCommandLine {
        DolphinCommandLine {
            executable: PathBuf::from(executable),
            arguments: arguments.iter().map(OsString::from).collect(),
            flatpak_app_id: None,
        }
    }

    #[test]
    fn parses_running_u_forms_and_paths_with_spaces_losslessly() {
        assert_eq!(
            dolphin_user_path(&command("dolphin-emu", &["-u", "/profiles/Dolphin User"]).arguments),
            Some(PathBuf::from("/profiles/Dolphin User"))
        );
        assert_eq!(
            dolphin_user_path(&command("dolphin-emu", &["-u=/profiles/User"]).arguments),
            Some(PathBuf::from("/profiles/User"))
        );
        assert_eq!(
            dolphin_user_path(
                &command("dolphin-emu", &["--user", "/profiles/Long Dolphin User"]).arguments
            ),
            Some(PathBuf::from("/profiles/Long Dolphin User"))
        );
        assert_eq!(
            dolphin_user_path(&command("dolphin-emu", &["--user=/profiles/Equals User"]).arguments),
            Some(PathBuf::from("/profiles/Equals User"))
        );
    }

    #[test]
    fn malformed_or_non_absolute_user_arguments_are_not_profile_targets() {
        for arguments in [
            vec!["--user"],
            vec!["--user", "--batch"],
            vec!["--user=relative/path"],
            vec!["-u", "/"],
        ] {
            assert_eq!(
                dolphin_user_path(&command("dolphin-emu", &arguments).arguments),
                None
            );
        }
    }

    #[test]
    fn flatpak_data_tree_replaces_config_tree_when_both_exist() {
        let root = fixture("flatpak-data-tree");
        let data =
            make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu"));
        let config =
            make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu"));
        let discovery = discover_dolphin_profiles(&roots(&root)).unwrap();
        let profile = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_path == data)
            .expect("Flatpak data-tree profile");
        assert_eq!(profile.resolved.configuration_root, config);
        assert_eq!(profile.resolved.data_user_root, data);
        assert_eq!(profile.game_settings_path, data.join("GameSettings"));
        assert!(
            discovery
                .profiles
                .iter()
                .all(|profile| profile.configuration_path != config)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_flatpak_config_tree_requires_positive_evidence() {
        let root = fixture("flatpak-legacy-tree");
        let config =
            make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu"));
        let discovery = discover_dolphin_profiles(&roots(&root)).unwrap();
        assert!(
            discovery
                .profiles
                .iter()
                .any(|profile| profile.configuration_path == config)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn running_flatpak_selects_data_tree_over_inactive_appimage_profile() {
        let root = fixture("active-flatpak");
        let data =
            make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu"));
        make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu"));
        let appimage = make_profile(&root.join("Applications/Dolphin/User"));
        let mut discovery_roots = roots(&root);
        discovery_roots
            .explicit_configuration_roots
            .push(appimage.clone());
        let mut flatpak = command("/app/bin/dolphin-emu", &[]);
        flatpak.flatpak_app_id = Some(FLATPAK_APP_ID.to_string());
        discovery_roots.running_commands.push(flatpak);
        let discovery = discover_dolphin_profiles(&discovery_roots).unwrap();
        let selection = select_dolphin_profile(&discovery, None);
        let selected = match selection {
            EmulatorProfileSelection::Auto { profile_id, .. } => profile_id,
            other => panic!("expected active Flatpak selection, got {other:?}"),
        };
        assert_eq!(
            discovery
                .profiles
                .iter()
                .find(|profile| profile.profile_id == selected)
                .unwrap()
                .configuration_path,
            data
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unique_active_custom_profile_wins_over_inactive_flatpak() {
        let root = fixture("active-custom");
        make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu"));
        make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu"));
        let active = make_profile(&root.join("Applications/Dolphin/User"));
        let mut discovery_roots = roots(&root);
        discovery_roots.running_commands.push(command(
            "/tmp/.mount_Dolphin123/bin/dolphin-emu",
            &["--user", active.to_str().unwrap()],
        ));
        let discovery = discover_dolphin_profiles(&discovery_roots).unwrap();
        let selection = select_dolphin_profile(&discovery, None);
        assert!(matches!(
            selection,
            EmulatorProfileSelection::Auto { ref profile_id, .. }
                if discovery.profiles.iter().any(|profile|
                    profile.profile_id == *profile_id && profile.configuration_path == active)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn two_active_profiles_require_a_current_explicit_choice() {
        let root = fixture("two-active");
        let first = make_profile(&root.join("first/User"));
        let second = make_profile(&root.join("second/User"));
        let mut discovery_roots = roots(&root);
        discovery_roots.running_commands = vec![
            command(
                "/opt/first/Dolphin.AppImage",
                &["-u", first.to_str().unwrap()],
            ),
            command(
                "/opt/second/Dolphin.AppImage",
                &["--user", second.to_str().unwrap()],
            ),
        ];
        let discovery = discover_dolphin_profiles(&discovery_roots).unwrap();
        assert!(matches!(
            select_dolphin_profile(&discovery, None),
            EmulatorProfileSelection::NeedsChoice { .. }
        ));
        let second_id = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_path == second)
            .unwrap()
            .profile_id
            .clone();
        assert!(matches!(
            select_dolphin_profile(&discovery, Some(&second_id)),
            EmulatorProfileSelection::Auto {
                reason: EmulatorProfileSelectReason::ExplicitChoice,
                ..
            }
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stopped_dolphin_selects_one_profile_but_not_multiple_profiles() {
        let root = fixture("stopped-selection");
        make_profile(&root.join("config/dolphin-emu"));
        let one = discover_dolphin_profiles(&roots(&root)).unwrap();
        assert!(matches!(
            select_dolphin_profile(&one, None),
            EmulatorProfileSelection::Auto {
                reason: EmulatorProfileSelectReason::OnlyValidProfile,
                ..
            }
        ));
        make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu"));
        make_profile(&root.join("home/.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu"));
        let multiple = discover_dolphin_profiles(&roots(&root)).unwrap();
        assert!(matches!(
            select_dolphin_profile(&multiple, None),
            EmulatorProfileSelection::NeedsChoice { .. }
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stale_process_executable_snapshot_is_rejected() {
        assert!(same_executable_snapshot(
            Path::new("/app/bin/dolphin-emu"),
            Some(Path::new("/app/bin/dolphin-emu"))
        ));
        assert!(!same_executable_snapshot(
            Path::new("/app/bin/dolphin-emu"),
            Some(Path::new("/usr/bin/unrelated"))
        ));
        assert!(!same_executable_snapshot(
            Path::new("/app/bin/dolphin-emu"),
            None
        ));
    }

    #[test]
    fn running_appimage_explicit_profile_beats_native_fallback() {
        let root = fixture("running-appimage");
        let native = make_profile(&root.join("config/dolphin-emu"));
        let active = make_profile(&root.join("Applications/Dolphin/User"));
        let mut discovery_roots = roots(&root);
        discovery_roots.running_commands.push(command(
            "/tmp/.mount_Dolphin123/bin/dolphin-emu",
            &["-u", active.to_str().unwrap()],
        ));
        let discovery = discover_dolphin_profiles(&discovery_roots).unwrap();
        let resolved = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_path == active)
            .unwrap();
        assert_eq!(
            resolved.installation_type,
            DolphinInstallationType::AppImage
        );
        assert_eq!(
            resolved.resolved.confidence,
            EmulatorProfileConfidence::RunningExplicit
        );
        assert!(
            resolved.resolved.priority
                > discovery
                    .profiles
                    .iter()
                    .find(|p| p.configuration_path == native)
                    .unwrap()
                    .resolved
                    .priority
        );
        assert_eq!(resolved.game_settings_path, active.join("GameSettings"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_launch_command_resolves_when_dolphin_is_not_running() {
        let root = fixture("selected-launch");
        let active = make_profile(&root.join("Selected AppImage User"));
        let mut discovery_roots = roots(&root);
        discovery_roots.selected_launch_commands.push(command(
            "/opt/Dolphin.AppImage",
            &[&format!("-u={}", active.display())],
        ));
        let discovery = discover_dolphin_profiles(&discovery_roots).unwrap();
        let profile = discovery
            .profiles
            .iter()
            .find(|p| p.configuration_path == active)
            .unwrap();
        assert_eq!(
            profile.resolved.confidence,
            EmulatorProfileConfidence::SelectedLaunch
        );
        assert_eq!(
            profile.resolved.emulator_executable.as_deref(),
            Some(Path::new("/opt/Dolphin.AppImage"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_default_is_fallback_when_no_stronger_evidence_exists() {
        let root = fixture("native-fallback");
        let native = make_profile(&root.join("config/dolphin-emu"));
        let discovery = discover_dolphin_profiles(&roots(&root)).unwrap();
        assert_eq!(discovery.profiles.len(), 1);
        assert_eq!(discovery.profiles[0].configuration_path, native);
        assert_eq!(discovery.profiles[0].resolved.priority, 100);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_executable_breaks_running_process_priority_tie() {
        let root = fixture("selected-executable");
        let selected = make_profile(&root.join("selected/User"));
        let unrelated = make_profile(&root.join("unrelated/User"));
        let selected_executable = PathBuf::from("/opt/selected/Dolphin.AppImage");
        let mut discovery_roots = roots(&root);
        discovery_roots.selected_executable = Some(selected_executable.clone());
        discovery_roots.running_commands = vec![
            command(
                selected_executable.to_str().unwrap(),
                &["-u", selected.to_str().unwrap()],
            ),
            command(
                "/opt/unrelated/Dolphin.AppImage",
                &["-u", unrelated.to_str().unwrap()],
            ),
        ];
        let discovery = discover_dolphin_profiles(&discovery_roots).unwrap();
        let selected_priority = discovery
            .profiles
            .iter()
            .find(|p| p.configuration_path == selected)
            .unwrap()
            .resolved
            .priority;
        let unrelated_priority = discovery
            .profiles
            .iter()
            .find(|p| p.configuration_path == unrelated)
            .unwrap()
            .resolved
            .priority;
        assert!(selected_priority > unrelated_priority);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirmed_real_command_resolves_exact_g9re7d_target() {
        let arguments = command(
            "/tmp/.mount_Dolphin/bin/dolphin-emu",
            &["-u", "/home/davedap/Applications/Dolphin/User"],
        );
        let user = dolphin_user_path(&arguments.arguments).unwrap();
        assert_eq!(
            user.join("GameSettings/G9RE7D.ini"),
            PathBuf::from("/home/davedap/Applications/Dolphin/User/GameSettings/G9RE7D.ini")
        );
    }

    #[test]
    fn parses_supported_sections_without_modifying_files() {
        let root = fixture("parse");
        let profile = make_profile(&root.join("portable"));
        let ini = profile.join("GameSettings/GALE01r2.ini");
        let body = b"[OnFrame]\n$60 FPS\n0x0=1\n[ActionReplay]\n$Infinite Lives\n[ActionReplay_Enabled]\n$Infinite Lives\n[Gecko]\n$Widescreen\n[Gecko_Enabled]\n$Widescreen\n[Riivolution]\n$Texture Pack\n";
        fs::write(&ini, body).unwrap();
        let inventory = inspect_dolphin_profile(&eligible(&profile)).unwrap();
        assert_eq!(inventory.files.len(), 1);
        let file = &inventory.files[0];
        assert_eq!(file.game_id_candidate.as_deref(), Some("GALE01"));
        assert_eq!(file.revision_candidate, Some(2));
        assert_eq!(file.region_candidate.as_deref(), Some("NTSC-U"));
        assert_eq!(file.definition_count(), 4);
        assert_eq!(file.enabled_count(), 2);
        assert_eq!(fs::read(ini).unwrap(), body);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn matching_requires_verified_identity() {
        let root = fixture("match");
        let profile = make_profile(&root.join("portable"));
        fs::write(
            profile.join("GameSettings/GALE01r2.ini"),
            b"[Gecko]\n$Code\n",
        )
        .unwrap();
        let inventory = inspect_dolphin_profile(&eligible(&profile)).unwrap();
        assert_eq!(
            match_dolphin_inventory(&inventory, None, None).state,
            DolphinMatchState::NoVerifiedGameIdAvailable
        );
        assert_eq!(
            match_dolphin_inventory(&inventory, Some("gale01"), Some(2)).state,
            DolphinMatchState::ExactGameIdAndRevisionMatch
        );
        assert_eq!(
            match_dolphin_inventory(&inventory, Some("GALE01"), Some(1)).state,
            DolphinMatchState::RevisionMismatch
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unsuffixed_filename_exact_matches_a_verified_revision_zero() {
        // Every real GameSettings filename found on a real Dolphin
        // installation (e.g. "NACE01.ini") omits the "rN" suffix entirely
        // for the common, unmarked case - this is the overwhelming
        // majority of real discs, so a verified revision-0 archive must
        // exact-match its own real file, not report RevisionMismatch.
        let root = fixture("revision-zero");
        let profile = make_profile(&root.join("portable"));
        fs::write(
            profile.join("GameSettings/GAFE01.ini"),
            b"[Gecko]\n$Code\nAABBCCDD 11223344\n",
        )
        .unwrap();
        let inventory = inspect_dolphin_profile(&eligible(&profile)).unwrap();
        assert_eq!(
            match_dolphin_inventory(&inventory, Some("GAFE01"), Some(0)).state,
            DolphinMatchState::ExactGameIdAndRevisionMatch
        );
        // A genuinely different, non-zero verified revision must still be
        // rejected - the fix is specific to revision 0, not a blanket
        // "no suffix always matches" rule.
        assert_eq!(
            match_dolphin_inventory(&inventory, Some("GAFE01"), Some(1)).state,
            DolphinMatchState::RevisionMismatch
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_profiles_and_ini_files() {
        use std::os::unix::fs::symlink;
        let root = fixture("symlink");
        let real = make_profile(&root.join("real"));
        fs::create_dir_all(root.join("container")).unwrap();
        symlink(&real, root.join("container/profile")).unwrap();
        let mut discovery_roots = roots(&root);
        discovery_roots
            .explicit_configuration_roots
            .push(root.join("container/profile"));
        assert!(
            !discover_dolphin_profiles(&discovery_roots)
                .unwrap()
                .profiles[0]
                .eligible
        );
        fs::write(root.join("outside.ini"), b"[Gecko]\n$Code\n").unwrap();
        symlink(
            root.join("outside.ini"),
            real.join("GameSettings/GALE01.ini"),
        )
        .unwrap();
        let inventory = inspect_dolphin_profile(&eligible(&real)).unwrap();
        assert!(inventory.files.is_empty());
        assert!(
            inventory
                .warnings
                .iter()
                .any(|w| w.kind == DolphinInspectionWarningKind::SymlinkSkipped)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_and_unsafe_exact_roots_are_blocked() {
        let root = fixture("blocked");
        fs::create_dir_all(&root).unwrap();
        let mut discovery_roots = roots(&root);
        discovery_roots.explicit_configuration_roots = vec![
            root.join("missing"),
            PathBuf::from("relative"),
            PathBuf::from("/"),
        ];
        let discovery = discover_dolphin_profiles(&discovery_roots).unwrap();
        assert_eq!(discovery.profiles.len(), 3);
        assert!(discovery.profiles.iter().all(|p| !p.eligible));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_ids_and_resource_limits_are_explicit() {
        let root = fixture("limits");
        let profile = make_profile(&root.join("portable"));
        fs::write(
            profile.join("GameSettings/not-an-id.ini"),
            b"[Gecko]\n$Code\n",
        )
        .unwrap();
        fs::write(
            profile.join("GameSettings/GALE01.ini"),
            vec![b'x'; DOLPHIN_MAX_LINE_BYTES + 1],
        )
        .unwrap();
        let inventory = inspect_dolphin_profile(&eligible(&profile)).unwrap();
        assert_eq!(inventory.files.len(), 1);
        assert!(
            inventory
                .warnings
                .iter()
                .any(|w| w.kind == DolphinInspectionWarningKind::InvalidGameId)
        );
        assert!(
            inventory
                .warnings
                .iter()
                .any(|w| w.kind == DolphinInspectionWarningKind::LineTooLong)
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn local_roots(root: &Path) -> DolphinLocalDiscoveryRoots {
        DolphinLocalDiscoveryRoots {
            home: root.join("home"),
            xdg_config_home: root.join("config"),
            xdg_data_home: root.join("data"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            dolphin_emu_userpath_override: None,
        }
    }

    fn make_local_profile(config: &Path, data: &Path) {
        fs::create_dir_all(config).unwrap();
        fs::create_dir_all(data).unwrap();
        fs::write(config.join("Dolphin.ini"), b"[Core]\nEnableCheats = True\n").unwrap();
    }

    #[test]
    fn modern_local_discovery_covers_native_flatpak_portable_and_supplied_version() {
        let root = fixture("modern-discovery");
        let native_config = root.join("config/dolphin-emu");
        let native_data = root.join("data/dolphin-emu");
        make_local_profile(&native_config, &native_data);
        let flatpak_config =
            root.join("home/.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu");
        let flatpak_data = root.join("home/.var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu");
        make_local_profile(&flatpak_config, &flatpak_data);
        let portable = root.join("portable/User");
        make_local_profile(&portable, &portable);
        let executable = root.join("bin/dolphin-emu");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"not executed").unwrap();
        let mut roots = local_roots(&root);
        roots.portable_configuration_roots.push(portable.clone());
        roots.explicit_executables.push(executable.clone());
        roots
            .known_version_outputs
            .insert(executable.clone(), "Dolphin 2409-17\n".into());
        let discovery = discover_dolphin_local_profiles(&roots);
        assert!(discovery.profiles.iter().any(|profile| {
            profile.installation_type == DolphinLocalInstallationType::Native
                && profile.eligible
                && profile.configuration_root == native_config
                && profile.data_root == native_data
        }));
        assert!(discovery.profiles.iter().any(|profile| {
            profile.installation_type == DolphinLocalInstallationType::FlatpakUser
                && profile.eligible
                && profile.configuration_root == flatpak_config
                && profile.data_root == flatpak_data
        }));
        let portable_profile = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_root == portable)
            .unwrap();
        assert_eq!(
            portable_profile.executable_candidates[0].version.as_deref(),
            Some("2409-17")
        );
        assert_eq!(parse_dolphin_version("unrelated text"), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn modern_inspection_reads_context_without_creating_identity_or_writing() {
        let root = fixture("modern-inspection");
        let config = root.join("portable");
        make_local_profile(&config, &config);
        fs::write(
            config.join("GFX.ini"),
            b"[Settings]\nBackend = Vulkan\nInternalResolution = 3\nVSync = True\n",
        )
        .unwrap();
        fs::create_dir_all(config.join("GameSettings")).unwrap();
        let game_settings = config.join("GameSettings/GALE01.ini");
        let game_settings_bytes = b"[Gecko]\n$Wide\n[Gecko_Enabled]\n$Wide\n";
        fs::write(&game_settings, game_settings_bytes).unwrap();
        fs::create_dir_all(config.join("Load/Textures/GALE01/nested")).unwrap();
        fs::write(
            config.join("Load/Textures/GALE01/nested/texture.png"),
            b"texture",
        )
        .unwrap();
        fs::create_dir_all(config.join("GC")).unwrap();
        fs::write(config.join("GC/MemoryCardA.USA.raw"), b"card").unwrap();
        fs::create_dir_all(config.join("Wii/title")).unwrap();
        fs::create_dir_all(config.join("StateSaves")).unwrap();
        fs::write(config.join("StateSaves/GALE01.sav"), b"state").unwrap();
        let mut roots = local_roots(&root);
        roots.portable_configuration_roots.push(config.clone());
        let profile = discover_dolphin_local_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| profile.configuration_root == config)
            .unwrap();
        let inspection = inspect_dolphin_local_game(
            &profile,
            &DolphinGameRequest {
                canonical_platform: Some("GameCube".into()),
                target_platform: Some(DolphinTargetPlatform::GameCube),
                verified_game_id: Some("GALE01".into()),
                verified_revision: Some(0),
                emulator_game_id: Some("GALE01".into()),
                disc_contexts: vec![DolphinDiscContext {
                    disc_number: 1,
                    format: DolphinDiscFormat::Gcm,
                    representation: Representation::RawDisc,
                    claim: ClaimType::ExactBytesMatch,
                }],
            },
        );
        assert_eq!(
            inspection.game_id_mapping,
            DolphinGameIdMapping::CoreVerifiedMetadata
        );
        assert_eq!(inspection.canonical_platform.as_deref(), Some("GameCube"));
        assert_eq!(
            inspection.global_config.settings.renderer.as_deref(),
            Some("Vulkan")
        );
        assert_eq!(inspection.game_settings.as_ref().unwrap().enabled_count, 1);
        assert_eq!(inspection.cheats.as_ref().unwrap().enabled_definitions, 1);
        assert_eq!(inspection.textures.as_ref().unwrap().file_count, 1);
        assert!(inspection.saves.wii_data_present);
        assert!(inspection.health.memory_cards_present);
        assert_eq!(fs::read(game_settings).unwrap(), game_settings_bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn emulator_game_id_and_filenames_never_override_core_identity() {
        let root = fixture("modern-identity");
        let config = root.join("portable");
        make_local_profile(&config, &config);
        fs::create_dir_all(config.join("GameSettings")).unwrap();
        // The only matching filename belongs to the emulator metadata ID;
        // it cannot cause a core identity winner to change.
        fs::write(config.join("GameSettings/WRONG1.ini"), b"[Gecko]\n$Code\n").unwrap();
        let mut roots = local_roots(&root);
        roots.portable_configuration_roots.push(config.clone());
        let profile = discover_dolphin_local_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| profile.configuration_root == config)
            .unwrap();
        let inspection = inspect_dolphin_local_game(
            &profile,
            &DolphinGameRequest {
                canonical_platform: Some("Wii".into()),
                target_platform: Some(DolphinTargetPlatform::Wii),
                verified_game_id: Some("RMGE01".into()),
                verified_revision: None,
                emulator_game_id: Some("WRONG1".into()),
                disc_contexts: vec![
                    DolphinDiscContext {
                        disc_number: 1,
                        format: DolphinDiscFormat::Wbfs,
                        representation: Representation::RawDisc,
                        claim: ClaimType::ExactBytesMatch,
                    },
                    DolphinDiscContext {
                        disc_number: 2,
                        format: DolphinDiscFormat::Chd,
                        representation: Representation::LogicalChd,
                        claim: ClaimType::ExactLogicalDiscMatch,
                    },
                ],
            },
        );
        assert_eq!(inspection.game_id.as_deref(), Some("RMGE01"));
        assert_eq!(
            inspection.game_id_mapping,
            DolphinGameIdMapping::ConflictingEmulatorMetadata
        );
        assert!(inspection.identity_mismatch.is_some());
        assert_eq!(
            inspection.game_settings.as_ref().unwrap().path,
            config.join("GameSettings/RMGE01.ini")
        );
        assert_eq!(inspection.disc_contexts[0].format, DolphinDiscFormat::Wbfs);
        assert_eq!(inspection.disc_contexts[1].format, DolphinDiscFormat::Chd);
        assert_eq!(
            inspection.disc_contexts[1].representation,
            Representation::LogicalChd
        );

        let metadata_only = inspect_dolphin_local_game(
            &profile,
            &DolphinGameRequest {
                emulator_game_id: Some("WRONG1".into()),
                ..DolphinGameRequest::default()
            },
        );
        assert_eq!(
            metadata_only.game_id_mapping,
            DolphinGameIdMapping::EmulatorMetadataOnly
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn modern_config_malformed_and_oversized_input_fails_soft() {
        let root = fixture("modern-bounds");
        let config = root.join("portable");
        make_local_profile(&config, &config);
        fs::write(config.join("GFX.ini"), b"[Settings\nNoSeparator\n").unwrap();
        let mut roots = local_roots(&root);
        roots.portable_configuration_roots.push(config.clone());
        let profile = discover_dolphin_local_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| profile.configuration_root == config)
            .unwrap();
        let inspection = inspect_dolphin_local_game(&profile, &DolphinGameRequest::default());
        assert!(
            inspection
                .global_config
                .warnings
                .iter()
                .any(|warning| warning.kind == DolphinInspectionWarningKind::MalformedIni)
        );
        fs::write(
            config.join("GFX.ini"),
            vec![b'x'; DOLPHIN_LOCAL_MAX_CONFIG_BYTES as usize + 1],
        )
        .unwrap();
        let oversized = inspect_dolphin_local_game(&profile, &DolphinGameRequest::default());
        assert!(
            oversized
                .global_config
                .warnings
                .iter()
                .any(|warning| warning.kind == DolphinInspectionWarningKind::FileTooLarge)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn modern_game_settings_uses_verified_revision_but_filenames_stay_context_only() {
        let root = fixture("modern-revision");
        let config = root.join("portable");
        make_local_profile(&config, &config);
        fs::create_dir_all(config.join("GameSettings")).unwrap();
        fs::write(
            config.join("GameSettings/GALE01r2.ini"),
            b"[ActionReplay]\n$Code\n[ActionReplay_Enabled]\n$Code\n",
        )
        .unwrap();
        let mut roots = local_roots(&root);
        roots.portable_configuration_roots.push(config.clone());
        let profile = discover_dolphin_local_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| profile.configuration_root == config)
            .unwrap();
        let inspection = inspect_dolphin_local_game(
            &profile,
            &DolphinGameRequest {
                verified_game_id: Some("GALE01".into()),
                verified_revision: Some(2),
                ..DolphinGameRequest::default()
            },
        );
        assert_eq!(
            inspection.game_settings.unwrap().path,
            config.join("GameSettings/GALE01r2.ini")
        );
        assert_eq!(inspection.cheats.as_ref().unwrap().definitions, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disc_format_contexts_remain_distinct_without_filename_inference() {
        let formats = [
            DolphinDiscFormat::Iso,
            DolphinDiscFormat::Gcm,
            DolphinDiscFormat::Wbfs,
            DolphinDiscFormat::Rvz,
            DolphinDiscFormat::Wia,
            DolphinDiscFormat::Chd,
        ];
        assert_eq!(formats.len(), 6);
        assert_ne!(DolphinDiscFormat::Iso, DolphinDiscFormat::Gcm);
        assert_ne!(DolphinDiscFormat::Wbfs, DolphinDiscFormat::Rvz);
        assert_ne!(DolphinDiscFormat::Wia, DolphinDiscFormat::Chd);
    }

    // --- Native launch binding ---------------------------------------------

    fn make_executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"not executed").unwrap();
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
    }

    fn executable(
        path: PathBuf,
        installation_type: DolphinLocalInstallationType,
    ) -> DolphinExecutable {
        DolphinExecutable {
            path,
            installation_type,
            version: None,
        }
    }

    fn native_profile(
        roots: &DolphinLocalDiscoveryRoots,
        executable_candidates: Vec<DolphinExecutable>,
    ) -> DolphinLocalProfile {
        let config = roots.xdg_config_home.join("dolphin-emu");
        let data = roots.xdg_data_home.join("dolphin-emu");
        make_local_profile(&config, &data);
        DolphinLocalProfile {
            profile_id: "test-native".into(),
            installation_type: DolphinLocalInstallationType::Native,
            configuration_root: config.clone(),
            data_root: data.clone(),
            eligible: true,
            blocker: None,
            executable_candidates,
            dolphin_ini_path: config.join("Dolphin.ini"),
            graphics_ini_path: config.join("GFX.ini"),
            game_settings_path: data.join("GameSettings"),
            textures_path: data.join("Load/Textures"),
            memory_cards_path: data.join("GC"),
            wii_data_path: data.join("Wii"),
            save_states_path: data.join("StateSaves"),
        }
    }

    fn explicit_profile(
        root: &Path,
        executable_candidates: Vec<DolphinExecutable>,
    ) -> DolphinLocalProfile {
        DolphinLocalProfile {
            profile_id: "test-explicit".into(),
            installation_type: DolphinLocalInstallationType::Explicit,
            configuration_root: root.to_path_buf(),
            data_root: root.to_path_buf(),
            eligible: true,
            blocker: None,
            executable_candidates,
            dolphin_ini_path: root.join("Dolphin.ini"),
            graphics_ini_path: root.join("GFX.ini"),
            game_settings_path: root.join("GameSettings"),
            textures_path: root.join("Load/Textures"),
            memory_cards_path: root.join("GC"),
            wii_data_path: root.join("Wii"),
            save_states_path: root.join("StateSaves"),
        }
    }

    #[test]
    fn standard_native_xdg_profile_produces_default_native() {
        let root = fixture("launch-native-default");
        let roots = local_roots(&root);
        let dolphin = roots
            .xdg_data_home
            .parent()
            .unwrap()
            .join("bin/dolphin-emu");
        make_executable(&dolphin);
        let profile = native_profile(
            &roots,
            vec![executable(
                dolphin.clone(),
                DolphinLocalInstallationType::Native,
            )],
        );
        let binding = resolve_dolphin_native_launch_binding(&profile, &roots).unwrap();
        assert_eq!(binding.executable, dolphin);
        assert_eq!(
            binding.user_directory_mode,
            DolphinUserDirectoryMode::DefaultNative
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn genuine_single_user_root_produces_explicit_root() {
        let root = fixture("launch-explicit-root");
        let profile_root = root.join("portable");
        fs::create_dir_all(profile_root.join("Config")).unwrap();
        fs::write(profile_root.join("Config/Dolphin.ini"), b"[Core]\n").unwrap();
        let dolphin = root.join("bin/dolphin-emu");
        make_executable(&dolphin);
        let profile = explicit_profile(
            &profile_root,
            vec![executable(
                dolphin.clone(),
                DolphinLocalInstallationType::Explicit,
            )],
        );
        let binding = resolve_dolphin_native_launch_binding(&profile, &local_roots(&root)).unwrap();
        assert_eq!(binding.executable, dolphin);
        assert_eq!(
            binding.user_directory_mode,
            DolphinUserDirectoryMode::ExplicitRoot(profile_root)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn data_root_alone_never_becomes_explicit_root() {
        let root = fixture("launch-data-root-only");
        let profile_root = root.join("portable");
        fs::create_dir_all(profile_root.join("GameSettings")).unwrap();
        // No Config/ beneath the root: only data-shaped evidence exists.
        let profile = explicit_profile(&profile_root, Vec::new());
        let blocker =
            resolve_dolphin_native_launch_binding(&profile, &local_roots(&root)).unwrap_err();
        assert_eq!(blocker.kind, DolphinLaunchBlockerKind::ExplicitRootInvalid);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn single_unambiguous_executable_binds() {
        let root = fixture("launch-single-executable");
        let roots = local_roots(&root);
        let dolphin = root.join("bin/dolphin-emu");
        make_executable(&dolphin);
        let profile = native_profile(
            &roots,
            vec![executable(
                dolphin.clone(),
                DolphinLocalInstallationType::Native,
            )],
        );
        assert_eq!(resolve_native_executable(&profile).unwrap(), dolphin);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multiple_indistinguishable_executables_are_blocked() {
        let root = fixture("launch-ambiguous-executable");
        let roots = local_roots(&root);
        let first = root.join("bin/dolphin-emu");
        let second = root.join("alt-bin/dolphin-emu");
        make_executable(&first);
        make_executable(&second);
        let profile = native_profile(
            &roots,
            vec![
                executable(first, DolphinLocalInstallationType::Native),
                executable(second, DolphinLocalInstallationType::Native),
            ],
        );
        let blocker = resolve_native_executable(&profile).unwrap_err();
        assert_eq!(blocker.kind, DolphinLaunchBlockerKind::AmbiguousExecutable);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_executable_is_blocked() {
        let root = fixture("launch-missing-executable");
        let roots = local_roots(&root);
        let dolphin = root.join("bin/dolphin-emu");
        let profile = native_profile(
            &roots,
            vec![executable(dolphin, DolphinLocalInstallationType::Native)],
        );
        let blocker = resolve_native_executable(&profile).unwrap_err();
        assert_eq!(blocker.kind, DolphinLaunchBlockerKind::ExecutableMissing);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_executable_is_blocked() {
        let root = fixture("launch-symlink-executable");
        let roots = local_roots(&root);
        let real = root.join("bin/dolphin-emu-real");
        make_executable(&real);
        let link = root.join("bin/dolphin-emu");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let profile = native_profile(
            &roots,
            vec![executable(link, DolphinLocalInstallationType::Native)],
        );
        let blocker = resolve_native_executable(&profile).unwrap_err();
        assert_eq!(blocker.kind, DolphinLaunchBlockerKind::ExecutableUnsafe);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_regular_executable_is_blocked() {
        let root = fixture("launch-non-regular-executable");
        let roots = local_roots(&root);
        let dolphin = root.join("bin/dolphin-emu");
        fs::create_dir_all(&dolphin).unwrap();
        let profile = native_profile(
            &roots,
            vec![executable(dolphin, DolphinLocalInstallationType::Native)],
        );
        let blocker = resolve_native_executable(&profile).unwrap_err();
        assert_eq!(blocker.kind, DolphinLaunchBlockerKind::ExecutableUnsafe);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn non_executable_permission_is_blocked() {
        let root = fixture("launch-non-executable-permission");
        let roots = local_roots(&root);
        let dolphin = root.join("bin/dolphin-emu");
        fs::create_dir_all(dolphin.parent().unwrap()).unwrap();
        fs::write(&dolphin, b"not executed").unwrap();
        let mut perms = fs::metadata(&dolphin).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&dolphin, perms).unwrap();
        let profile = native_profile(
            &roots,
            vec![executable(dolphin, DolphinLocalInstallationType::Native)],
        );
        let blocker = resolve_native_executable(&profile).unwrap_err();
        assert_eq!(
            blocker.kind,
            DolphinLaunchBlockerKind::ExecutableNotExecutable
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flatpak_profile_is_blocked_for_native_launch_binding() {
        let root = fixture("launch-flatpak-blocked");
        let roots = local_roots(&root);
        let mut profile = native_profile(&roots, Vec::new());
        profile.installation_type = DolphinLocalInstallationType::FlatpakUser;
        let blocker = resolve_dolphin_native_launch_binding(&profile, &roots).unwrap_err();
        assert_eq!(
            blocker.kind,
            DolphinLaunchBlockerKind::UnsupportedInstallationType
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_profile_is_blocked_for_native_launch_binding() {
        let root = fixture("launch-portable-blocked");
        let profile_root = root.join("AppImage/User");
        fs::create_dir_all(profile_root.join("Config")).unwrap();
        fs::write(profile_root.join("Config/Dolphin.ini"), b"[Core]\n").unwrap();
        let mut profile = explicit_profile(&profile_root, Vec::new());
        profile.installation_type = DolphinLocalInstallationType::Portable;
        let blocker =
            resolve_dolphin_native_launch_binding(&profile, &local_roots(&root)).unwrap_err();
        assert_eq!(
            blocker.kind,
            DolphinLaunchBlockerKind::UnsupportedInstallationType
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dolphin_emu_userpath_override_rejects_default_native() {
        let root = fixture("launch-userpath-override");
        let mut roots = local_roots(&root);
        let dolphin = root.join("bin/dolphin-emu");
        make_executable(&dolphin);
        let profile = native_profile(
            &roots,
            vec![executable(dolphin, DolphinLocalInstallationType::Native)],
        );
        roots.dolphin_emu_userpath_override = Some(root.join("override"));
        let blocker = resolve_dolphin_native_launch_binding(&profile, &roots).unwrap_err();
        assert_eq!(
            blocker.kind,
            DolphinLaunchBlockerKind::EnvironmentOverridePresent
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_txt_marker_rejects_default_native() {
        let root = fixture("launch-portable-txt");
        let roots = local_roots(&root);
        let dolphin = root.join("bin/dolphin-emu");
        make_executable(&dolphin);
        fs::write(dolphin.parent().unwrap().join("portable.txt"), b"").unwrap();
        let profile = native_profile(
            &roots,
            vec![executable(dolphin, DolphinLocalInstallationType::Native)],
        );
        let blocker = resolve_dolphin_native_launch_binding(&profile, &roots).unwrap_err();
        assert_eq!(
            blocker.kind,
            DolphinLaunchBlockerKind::PortableOrLegacyLayoutConflict
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_dolphin_emu_directory_rejects_default_native() {
        let root = fixture("launch-legacy-precedence");
        let roots = local_roots(&root);
        let dolphin = root.join("bin/dolphin-emu");
        make_executable(&dolphin);
        fs::create_dir_all(roots.home.join(".dolphin-emu")).unwrap();
        let profile = native_profile(
            &roots,
            vec![executable(dolphin, DolphinLocalInstallationType::Native)],
        );
        let blocker = resolve_dolphin_native_launch_binding(&profile, &roots).unwrap_err();
        assert_eq!(
            blocker.kind,
            DolphinLaunchBlockerKind::PortableOrLegacyLayoutConflict
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drifted_roots_reject_default_native() {
        let root = fixture("launch-drifted-roots");
        let roots = local_roots(&root);
        let dolphin = root.join("bin/dolphin-emu");
        make_executable(&dolphin);
        let mut profile = native_profile(
            &roots,
            vec![executable(dolphin, DolphinLocalInstallationType::Native)],
        );
        // Simulate a profile snapshot that no longer matches the fresh
        // default XDG resolution (e.g. XDG_CONFIG_HOME changed since
        // discovery ran).
        profile.configuration_root = root.join("stale-config/dolphin-emu");
        let blocker = resolve_dolphin_native_launch_binding(&profile, &roots).unwrap_err();
        assert_eq!(
            blocker.kind,
            DolphinLaunchBlockerKind::DefaultResolutionMismatch
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_explicit_root_is_rejected() {
        let root = fixture("launch-symlinked-root");
        let real_root = root.join("real-portable");
        fs::create_dir_all(real_root.join("Config")).unwrap();
        fs::write(real_root.join("Config/Dolphin.ini"), b"[Core]\n").unwrap();
        let linked_root = root.join("portable");
        std::os::unix::fs::symlink(&real_root, &linked_root).unwrap();
        let profile = explicit_profile(&linked_root, Vec::new());
        let blocker =
            resolve_dolphin_native_launch_binding(&profile, &local_roots(&root)).unwrap_err();
        assert_eq!(blocker.kind, DolphinLaunchBlockerKind::ExplicitRootInvalid);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn no_hard_coded_executable_fallback() {
        let root = fixture("launch-no-fallback");
        let roots = local_roots(&root);
        // No executable candidates supplied at all; a well-known name like
        // "dolphin-emu" must never be assumed even though the profile is
        // otherwise eligible and native.
        let profile = native_profile(&roots, Vec::new());
        let blocker = resolve_dolphin_native_launch_binding(&profile, &roots).unwrap_err();
        assert_eq!(blocker.kind, DolphinLaunchBlockerKind::ExecutableMissing);
        fs::remove_dir_all(root).unwrap();
    }
}
