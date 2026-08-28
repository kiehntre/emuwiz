//! Bounded, read-only discovery and inspection of local RPCS3 (PS3
//! emulator) profiles.
//!
//! RPCS3 is intentionally treated as a consumer of PS3 identity, exactly
//! the same discipline [`super::ppsspp_local`] already established for
//! PPSSPP/PSP. A `TITLE_ID` read from a local PARAM.SFO, a directory name
//! under `dev_hdd0/game`, a patch definition's own title key, an update's
//! `APP_VER`, or a DLC package's `CONTENT_ID` are all useful emulator
//! context - never game identity evidence. Preservation identity for a
//! PS3 title comes from [`crate::ps3_boot_evidence`]/
//! [`crate::ps3_disc_evidence`]/[`crate::param_sfo`] and the caller's own
//! resolved [`crate::platform_evidence_fusion::identity_presentation::IdentityStatus`],
//! and this module only ever *maps* an already-verified title ID onto
//! local RPCS3 assets, falling back to unauthoritative emulator-observed
//! metadata when no verified ID is available. It can never construct or
//! upgrade identity itself.
//!
//! This module never starts RPCS3, writes a file, edits `config.yml` or a
//! per-game config, enables a patch, installs a PKG, moves/copies a RAP,
//! installs firmware, or follows a symlink. It never scrapes the RPCS3
//! compatibility database or downloads anything - every fact here comes
//! from bounded, local, read-only inspection of documented paths.

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde::Serialize;

use crate::emulator_environment::EncodedPath;
use crate::param_sfo::{SfoObservation, parse_param_sfo};
use crate::ps3_disc_evidence::derive_title_id_from_content_id;

use super::destination_safety::{
    DestinationRootState, DestinationSafetyFailureReason, validate_destination_root,
};

pub const RPCS3_MAX_PROFILES: usize = 16;
pub const RPCS3_MAX_CONFIG_BYTES: u64 = 512 * 1024;
pub const RPCS3_MAX_SFO_SCAN_ENTRIES: usize = 4_096;
pub const RPCS3_MAX_DLC_ENTRIES: usize = 256;
pub const RPCS3_MAX_PATCH_ENTRIES: usize = 512;
pub const RPCS3_MAX_ENTRIES_VISITED: usize = 10_000;
pub const RPCS3_MAX_SAVE_TROPHY_CANDIDATES: usize = 128;

const FLATPAK_APP_ID: &str = "net.rpcs3.RPCS3";
const MAX_YAML_LINES: usize = 8_192;
const MAX_YAML_LINE_BYTES: usize = 4 * 1024;
const MAX_RETAINED_UNKNOWN_SETTINGS: usize = 256;
/// The version RPCS3/PARAM.SFO conventionally ships as the unpatched
/// baseline for many titles. Not authoritative for every game - only used
/// as a soft "does this look like it has ever been updated" signal.
const BASELINE_APP_VERSIONS: [&str; 2] = ["01.00", "1.00"];

// ---------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rpcs3InstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rpcs3ProfileScope {
    User,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rpcs3ProfileBlockerKind {
    PathNotAbsolute,
    FilesystemRoot,
    MissingConfiguration,
    UnsafePath,
    NotDirectory,
    Unreadable,
    MissingRpcs3Evidence,
    ProfileLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rpcs3ProfileBlocker {
    pub kind: Rpcs3ProfileBlockerKind,
    pub path: EncodedPath,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rpcs3InspectionWarningKind {
    UnsafePath,
    UnreadablePath,
    SymlinkSkipped,
    SpecialFileSkipped,
    EntryLimitReached,
    DepthLimitReached,
    FileTooLarge,
    InvalidUtf8,
    MalformedConfig,
    MalformedSfo,
    LineCountLimitReached,
    LineTooLong,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3InspectionWarning {
    pub kind: Rpcs3InspectionWarningKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3Executable {
    pub path: PathBuf,
    pub installation_type: Rpcs3InstallationType,
    /// Deliberately optional: discovery never executes a user binary. A
    /// caller that has already obtained read-only version text can use
    /// [`parse_rpcs3_version`] without changing this safety boundary.
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3Profile {
    pub profile_id: String,
    pub installation_type: Rpcs3InstallationType,
    pub scope: Rpcs3ProfileScope,
    /// The directory containing RPCS3's `dev_hdd0`/`config.yml`.
    pub configuration_path: PathBuf,
    pub provenance: &'static str,
    pub eligible: bool,
    pub blockers: Vec<Rpcs3ProfileBlocker>,
    pub executable_candidates: Vec<Rpcs3Executable>,
    pub dev_hdd0_path: PathBuf,
    pub dev_flash_path: PathBuf,
    pub global_config_path: PathBuf,
    pub games_path: PathBuf,
    pub custom_configs_path: PathBuf,
    pub patches_path: PathBuf,
    pub games_yml_path: PathBuf,
}

/// The exact native RPCS3 executable selected for a launch. This is a
/// binding, not a command: the launch planner owns argv construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3LaunchBinding {
    pub executable: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rpcs3LaunchBlockerKind {
    ProfileIneligible,
    UnsupportedInstallation,
    ExecutableMissing,
    AmbiguousExecutable,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3LaunchBlocker {
    pub kind: Rpcs3LaunchBlockerKind,
    pub detail: String,
}

fn rpcs3_launch_blocker(
    kind: Rpcs3LaunchBlockerKind,
    detail: impl Into<String>,
) -> Rpcs3LaunchBlocker {
    Rpcs3LaunchBlocker {
        kind,
        detail: detail.into(),
    }
}

/// Resolves one safe native Linux RPCS3 executable for a discovered profile.
/// Flatpak/portable profiles are intentionally not bound here because this
/// milestone has no proven configuration-directory/argv contract for them.
pub fn resolve_rpcs3_native_launch_binding(
    profile: &Rpcs3Profile,
) -> Result<Rpcs3LaunchBinding, Rpcs3LaunchBlocker> {
    if !profile.eligible {
        return Err(rpcs3_launch_blocker(
            Rpcs3LaunchBlockerKind::ProfileIneligible,
            "profile is not eligible",
        ));
    }
    if profile.installation_type != Rpcs3InstallationType::Native {
        return Err(rpcs3_launch_blocker(
            Rpcs3LaunchBlockerKind::UnsupportedInstallation,
            "only native Linux RPCS3 profiles have a reviewed direct argv contract",
        ));
    }
    let matching: Vec<&Rpcs3Executable> = profile
        .executable_candidates
        .iter()
        .filter(|candidate| candidate.installation_type == Rpcs3InstallationType::Native)
        .collect();
    if matching.is_empty() {
        return Err(rpcs3_launch_blocker(
            Rpcs3LaunchBlockerKind::ExecutableMissing,
            "no native RPCS3 executable was discovered",
        ));
    }
    let mut valid = Vec::new();
    let mut last_error = None;
    for candidate in matching {
        match validate_rpcs3_executable(&candidate.path) {
            Ok(()) => valid.push(candidate.path.clone()),
            Err(error) => last_error = Some(error),
        }
    }
    match valid.len() {
        0 => Err(last_error.expect("at least one executable was inspected")),
        1 => Ok(Rpcs3LaunchBinding {
            executable: valid.pop().expect("length checked above"),
        }),
        count => Err(rpcs3_launch_blocker(
            Rpcs3LaunchBlockerKind::AmbiguousExecutable,
            format!("{count} viable native RPCS3 executables remain"),
        )),
    }
}

fn validate_rpcs3_executable(path: &Path) -> Result<(), Rpcs3LaunchBlocker> {
    if !path.is_absolute() {
        return Err(rpcs3_launch_blocker(
            Rpcs3LaunchBlockerKind::ExecutableUnsafe,
            format!("{} is not an absolute path", path.display()),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        rpcs3_launch_blocker(
            Rpcs3LaunchBlockerKind::ExecutableMissing,
            format!("{} does not exist", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(rpcs3_launch_blocker(
            Rpcs3LaunchBlockerKind::ExecutableUnsafe,
            format!("{} is not a safe regular executable file", path.display()),
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(rpcs3_launch_blocker(
            Rpcs3LaunchBlockerKind::ExecutableNotExecutable,
            format!("{} is not executable", path.display()),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3ProfileDiscovery {
    pub profiles: Vec<Rpcs3Profile>,
    pub warnings: Vec<Rpcs3ProfileBlocker>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3ProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    /// Exact configuration roots explicitly configured by the user or a
    /// higher-level settings layer. Never discovered by scanning.
    pub explicit_configuration_roots: Vec<PathBuf>,
    /// Exact known portable/AppImage configuration directories.
    pub portable_configuration_roots: Vec<PathBuf>,
    /// Exact known executables; useful for configured AppImage/custom
    /// paths.
    pub explicit_executables: Vec<PathBuf>,
    /// Version text obtained by an already-authorized outer probe.
    /// Discovery itself never executes the binary.
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
}

impl Rpcs3ProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, Rpcs3DiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(Rpcs3DiscoveryError::HomeUnavailable)?;
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
        })
    }
}

#[derive(Debug)]
pub enum Rpcs3DiscoveryError {
    HomeUnavailable,
}

impl std::fmt::Display for Rpcs3DiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable => formatter.write_str("HOME is not set"),
        }
    }
}

impl std::error::Error for Rpcs3DiscoveryError {}

#[derive(Debug, Clone)]
struct ProfileCandidate {
    installation_type: Rpcs3InstallationType,
    scope: Rpcs3ProfileScope,
    configuration_path: PathBuf,
    provenance: &'static str,
}

/// Discovers only documented XDG/Flatpak paths and exact caller-provided
/// portable/custom paths. No home-directory recursion and no process
/// launch occurs here.
pub fn discover_rpcs3_profiles(roots: &Rpcs3ProfileDiscoveryRoots) -> Rpcs3ProfileDiscovery {
    let mut candidates = vec![
        ProfileCandidate {
            installation_type: Rpcs3InstallationType::Native,
            scope: Rpcs3ProfileScope::User,
            configuration_path: roots.xdg_config_home.join("rpcs3"),
            provenance: "XDG RPCS3 configuration directory",
        },
        ProfileCandidate {
            installation_type: Rpcs3InstallationType::Native,
            scope: Rpcs3ProfileScope::User,
            configuration_path: roots.xdg_data_home.join("rpcs3"),
            provenance: "XDG RPCS3 data directory",
        },
        ProfileCandidate {
            installation_type: Rpcs3InstallationType::FlatpakUser,
            scope: Rpcs3ProfileScope::User,
            configuration_path: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("config/rpcs3"),
            provenance: "Flatpak RPCS3 configuration directory",
        },
        ProfileCandidate {
            installation_type: Rpcs3InstallationType::FlatpakUser,
            scope: Rpcs3ProfileScope::User,
            configuration_path: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("data/rpcs3"),
            provenance: "Flatpak RPCS3 data directory",
        },
    ];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|path| ProfileCandidate {
                installation_type: Rpcs3InstallationType::Portable,
                scope: Rpcs3ProfileScope::Explicit,
                configuration_path: path,
                provenance: "caller-supplied RPCS3 portable/AppImage configuration directory",
            }),
    );
    if let Some(directory) = &roots.appimage_directory {
        candidates.push(ProfileCandidate {
            installation_type: Rpcs3InstallationType::Portable,
            scope: Rpcs3ProfileScope::Explicit,
            configuration_path: directory.join("config"),
            provenance: "APPIMAGE-adjacent RPCS3 portable configuration directory",
        });
    }
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|path| ProfileCandidate {
                installation_type: Rpcs3InstallationType::Explicit,
                scope: Rpcs3ProfileScope::Explicit,
                configuration_path: path,
                provenance: "explicit RPCS3 configuration directory",
            }),
    );
    candidates.sort_by(|left, right| left.configuration_path.cmp(&right.configuration_path));
    candidates.dedup_by(|left, right| left.configuration_path == right.configuration_path);

    let executables = discover_executables(roots);
    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for candidate in candidates {
        if profiles.len() >= RPCS3_MAX_PROFILES {
            warnings.push(blocker(
                Rpcs3ProfileBlockerKind::ProfileLimitReached,
                &candidate.configuration_path,
                format!("profile discovery stopped at the {RPCS3_MAX_PROFILES}-profile limit"),
            ));
            break;
        }
        if !candidate.configuration_path.exists() && candidate.scope == Rpcs3ProfileScope::User {
            continue;
        }
        profiles.push(validate_profile(candidate, &executables));
    }
    Rpcs3ProfileDiscovery {
        profiles,
        warnings,
        complete: true,
    }
}

fn validate_profile(candidate: ProfileCandidate, executables: &[Rpcs3Executable]) -> Rpcs3Profile {
    let path = candidate.configuration_path;
    let mut blockers = Vec::new();
    let eligible = if !path.is_absolute() {
        blockers.push(blocker(
            Rpcs3ProfileBlockerKind::PathNotAbsolute,
            &path,
            "configuration path is not absolute",
        ));
        false
    } else if path.parent().is_none() {
        blockers.push(blocker(
            Rpcs3ProfileBlockerKind::FilesystemRoot,
            &path,
            "a filesystem root cannot be an RPCS3 profile",
        ));
        false
    } else {
        match validate_destination_root(&path) {
            Ok(validated) if validated.state() == DestinationRootState::Absent => {
                blockers.push(blocker(
                    Rpcs3ProfileBlockerKind::MissingConfiguration,
                    &path,
                    "configuration directory does not exist",
                ));
                false
            }
            Ok(_)
                if !is_real_directory(&path.join("dev_hdd0"))
                    && !is_regular_file(&path.join("config.yml")) =>
            {
                blockers.push(blocker(
                    Rpcs3ProfileBlockerKind::MissingRpcs3Evidence,
                    &path,
                    "neither dev_hdd0/ nor config.yml was found",
                ));
                false
            }
            Ok(_) => true,
            Err(error) => {
                let kind = match error.reason {
                    DestinationSafetyFailureReason::RootNotDirectory
                    | DestinationSafetyFailureReason::NonDirectoryParent => {
                        Rpcs3ProfileBlockerKind::NotDirectory
                    }
                    DestinationSafetyFailureReason::InspectionFailed => {
                        Rpcs3ProfileBlockerKind::Unreadable
                    }
                    _ => Rpcs3ProfileBlockerKind::UnsafePath,
                };
                blockers.push(blocker(
                    kind,
                    &path,
                    format!("configuration path rejected: {:?}", error.reason),
                ));
                false
            }
        }
    };
    let dev_hdd0_path = path.join("dev_hdd0");
    let games_path = dev_hdd0_path.join("game");
    Rpcs3Profile {
        profile_id: format!("rpcs3:{}", path.display()),
        installation_type: candidate.installation_type,
        scope: candidate.scope,
        configuration_path: path.clone(),
        provenance: candidate.provenance,
        eligible,
        blockers,
        executable_candidates: executables.to_vec(),
        dev_flash_path: path.join("dev_flash"),
        global_config_path: path.join("config.yml"),
        custom_configs_path: path.join("config/custom_configs"),
        patches_path: path.join("patches"),
        games_yml_path: path.join("games.yml"),
        dev_hdd0_path,
        games_path,
    }
}

fn discover_executables(roots: &Rpcs3ProfileDiscoveryRoots) -> Vec<Rpcs3Executable> {
    let mut paths = roots.explicit_executables.clone();
    if let Some(directory) = &roots.appimage_directory {
        paths.extend([directory.join("RPCS3.AppImage"), directory.join("rpcs3")]);
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path).take(128) {
            paths.push(directory.join("rpcs3"));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| is_regular_file(path))
        .map(|path| Rpcs3Executable {
            installation_type: if roots.explicit_executables.contains(&path) {
                Rpcs3InstallationType::Explicit
            } else if roots
                .appimage_directory
                .as_ref()
                .is_some_and(|directory| path.starts_with(directory))
            {
                Rpcs3InstallationType::Portable
            } else {
                Rpcs3InstallationType::Native
            },
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|output| parse_rpcs3_version(output)),
            path,
        })
        .collect()
}

/// Parses an RPCS3 version from output already obtained by a caller. The
/// adapter itself does not execute binaries, keeping discovery read-only.
/// Conservative and fail-soft: an unrecognised/changed `--version` shape
/// yields `None` rather than a guessed value.
pub fn parse_rpcs3_version(output: &str) -> Option<String> {
    let normalized = output.trim();
    let index = normalized
        .find("rpcs3-v")
        .or_else(|| normalized.find("RPCS3 v"))?;
    let tail = &normalized[index..];
    let tail = tail
        .trim_start_matches("rpcs3-v")
        .trim_start_matches("RPCS3 v");
    let version: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    (version.split('.').count() >= 2 && version.chars().any(|character| character.is_ascii_digit()))
        .then_some(version)
}

// ---------------------------------------------------------------------
// Selected-title mapping
// ---------------------------------------------------------------------

/// A deliberately separate input lane for identity supplied by core and an
/// identifier merely observed in RPCS3 context. Neither filename nor any
/// inspected directory can construct `verified_ps3_title_id`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rpcs3GameRequest {
    pub verified_ps3_title_id: Option<String>,
    pub emulator_game_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rpcs3GameIdMapping {
    VerifiedPs3TitleId,
    EmulatorMetadataOnly,
    Unavailable,
}

fn select_title_id(request: &Rpcs3GameRequest) -> (Option<String>, Rpcs3GameIdMapping) {
    if let Some(id) = request
        .verified_ps3_title_id
        .as_deref()
        .and_then(normalize_title_id)
    {
        return (Some(id), Rpcs3GameIdMapping::VerifiedPs3TitleId);
    }
    if let Some(id) = request
        .emulator_game_id
        .as_deref()
        .and_then(normalize_title_id)
    {
        return (Some(id), Rpcs3GameIdMapping::EmulatorMetadataOnly);
    }
    (None, Rpcs3GameIdMapping::Unavailable)
}

fn normalize_title_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()
        && trimmed.len() <= 32
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then(|| trimmed.to_ascii_uppercase())
}

// ---------------------------------------------------------------------
// Installed base game / disc game / update
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3InstalledGame {
    pub title_id: String,
    pub param_sfo_path: PathBuf,
    pub install_path: PathBuf,
    /// Read from `PARAM.SFO`'s `TITLE` field - display-only, never
    /// authoritative for identity (see this module's own doc comment).
    pub display_title: Option<String>,
    pub app_version: Option<String>,
    pub category: Option<String>,
    pub disc_style: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rpcs3UpdateInfo {
    pub detected: bool,
    pub version: Option<String>,
    pub path: Option<PathBuf>,
}

/// Locates and parses `PARAM.SFO` at `install_path/PARAM.SFO`, bounded and
/// fail-soft. Never treats the directory name as identity - only the
/// parsed `TITLE_ID` field is ever compared against `title_id`, and only
/// for a sanity warning, never to substitute a different value.
fn inspect_installed_game(
    title_id: &str,
    install_path: &Path,
    disc_style: bool,
    warnings: &mut Vec<Rpcs3InspectionWarning>,
) -> Option<Rpcs3InstalledGame> {
    let sfo_path = install_path.join("PARAM.SFO");
    let sfo = read_sfo(&sfo_path, warnings)?;
    Some(Rpcs3InstalledGame {
        title_id: title_id.to_string(),
        param_sfo_path: sfo_path,
        install_path: install_path.to_path_buf(),
        display_title: sfo.get_text("TITLE").map(str::to_string),
        app_version: sfo.get_text("APP_VER").map(str::to_string),
        category: sfo.get_text("CATEGORY").map(str::to_string),
        disc_style,
    })
}

fn inspect_update(base_game: Option<&Rpcs3InstalledGame>) -> Rpcs3UpdateInfo {
    // RPCS3 applies an installed update in place onto the base title's own
    // `dev_hdd0/game/<TITLE_ID>` directory - there is no separate
    // "current update" directory to enumerate. `APP_VER` differing from
    // the conventional unpatched baseline is the only locally-observable,
    // bounded signal that an update was ever applied; this is a
    // conservative approximation, not a guarantee, and is documented as
    // such rather than presented as certain.
    let Some(game) = base_game else {
        return Rpcs3UpdateInfo::default();
    };
    let Some(version) = &game.app_version else {
        return Rpcs3UpdateInfo::default();
    };
    let looks_updated = !BASELINE_APP_VERSIONS.contains(&version.as_str());
    Rpcs3UpdateInfo {
        detected: looks_updated,
        version: looks_updated.then(|| version.clone()),
        path: looks_updated.then(|| game.param_sfo_path.clone()),
    }
}

// ---------------------------------------------------------------------
// DLC
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3DlcEntry {
    pub content_id: Option<String>,
    pub title_id: Option<String>,
    pub path: PathBuf,
    pub display_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rpcs3DlcInventory {
    pub entries: Vec<Rpcs3DlcEntry>,
    pub count: usize,
    pub complete: bool,
    pub warnings: Vec<Rpcs3InspectionWarning>,
}

/// Bounded scan of `games_path`'s immediate children for content whose
/// `PARAM.SFO`-derived `CONTENT_ID` maps back to `title_id` (via
/// [`derive_title_id_from_content_id`], unchanged) but whose own directory
/// is not the base title itself. Never a filename guess: only a parsed
/// `CONTENT_ID` field counts.
fn inspect_dlc(
    games_path: &Path,
    title_id: &str,
    warnings: &mut Vec<Rpcs3InspectionWarning>,
) -> Rpcs3DlcInventory {
    let mut inventory = Rpcs3DlcInventory {
        complete: true,
        ..Default::default()
    };
    let Ok(read_dir) = fs::read_dir(games_path) else {
        return inventory;
    };
    let mut visited = 0usize;
    for entry in read_dir.flatten() {
        visited += 1;
        if visited > RPCS3_MAX_ENTRIES_VISITED {
            inventory.complete = false;
            warn(
                warnings,
                Rpcs3InspectionWarningKind::EntryLimitReached,
                games_path,
                format!("DLC scan stopped at the {RPCS3_MAX_ENTRIES_VISITED}-entry limit"),
            );
            break;
        }
        if inventory.entries.len() >= RPCS3_MAX_DLC_ENTRIES {
            inventory.complete = false;
            warn(
                warnings,
                Rpcs3InspectionWarningKind::EntryLimitReached,
                games_path,
                format!("DLC scan stopped at the {RPCS3_MAX_DLC_ENTRIES}-entry limit"),
            );
            break;
        }
        let entry_path = entry.path();
        if !is_real_directory(&entry_path) {
            continue;
        }
        let Some(sfo) = read_sfo(&entry_path.join("PARAM.SFO"), warnings) else {
            continue;
        };
        let content_id = sfo.get_text("CONTENT_ID").map(str::to_string);
        let Some(derived_title_id) = content_id
            .as_deref()
            .and_then(derive_title_id_from_content_id)
        else {
            continue;
        };
        if derived_title_id != title_id {
            continue;
        }
        let entry_title_id = sfo.get_text("TITLE_ID").map(str::to_string);
        if entry_title_id.as_deref() == Some(title_id) {
            // This is the base title's own directory, not add-on content.
            continue;
        }
        inventory.entries.push(Rpcs3DlcEntry {
            content_id,
            title_id: entry_title_id,
            path: entry_path,
            display_title: sfo.get_text("TITLE").map(str::to_string),
        });
    }
    inventory.count = inventory.entries.len();
    inventory
}

// ---------------------------------------------------------------------
// Per-game config (bounded YAML subset)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rpcs3Settings {
    pub cpu_decoder: Option<String>,
    pub spu_decoder: Option<String>,
    pub renderer: Option<String>,
    pub resolution_scale: Option<String>,
    pub frame_limit: Option<String>,
    pub vsync: Option<bool>,
    pub write_color_buffers: Option<bool>,
    pub strict_rendering_mode: Option<bool>,
    pub audio_backend: Option<String>,
    pub controller_profile_present: Option<bool>,
    /// Unknown keys are retained in bounded form for later UI display.
    pub unknown: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3PerGameConfig {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub settings: Rpcs3Settings,
    pub warnings: Vec<Rpcs3InspectionWarning>,
}

fn inspect_per_game_config(path: &Path) -> Rpcs3PerGameConfig {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, RPCS3_MAX_CONFIG_BYTES, &mut warnings) else {
        return Rpcs3PerGameConfig {
            path: path.to_path_buf(),
            exists,
            readable: false,
            settings: Rpcs3Settings::default(),
            warnings,
        };
    };
    let settings = parse_yaml_settings(&text, path, &mut warnings);
    Rpcs3PerGameConfig {
        path: path.to_path_buf(),
        exists,
        readable: true,
        settings,
        warnings,
    }
}

/// A narrow, bounded parser for exactly the two-level `Section:` /
/// `  Key: value` shape RPCS3's YAML config files use for the settings
/// this module models - never a general YAML implementation, and never
/// used to interpret anything beyond flat scalar values. An unrecognised
/// key, a malformed line, or a structure deeper than two levels simply
/// fails soft (skipped or retained verbatim in `unknown`), never a panic
/// and never a partial/guessed value for a known field.
fn parse_yaml_settings(
    text: &str,
    path: &Path,
    warnings: &mut Vec<Rpcs3InspectionWarning>,
) -> Rpcs3Settings {
    let mut settings = Rpcs3Settings::default();
    let mut section = String::new();
    for (index, raw) in text.lines().enumerate() {
        if index >= MAX_YAML_LINES {
            warn(
                warnings,
                Rpcs3InspectionWarningKind::LineCountLimitReached,
                path,
                format!("YAML parsing stopped at the {MAX_YAML_LINES}-line limit"),
            );
            break;
        }
        if raw.len() > MAX_YAML_LINE_BYTES {
            warn(
                warnings,
                Rpcs3InspectionWarningKind::LineTooLong,
                path,
                format!("YAML contains a line over {MAX_YAML_LINE_BYTES} bytes"),
            );
            continue;
        }
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let trimmed = raw.trim();
        if indent == 0 {
            // A bare top-level "Section:" line - only the section name
            // matters here, never a scalar value on this line.
            if let Some(name) = trimmed.strip_suffix(':') {
                section = name.trim().to_ascii_lowercase();
            } else {
                section.clear();
            }
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            warn(
                warnings,
                Rpcs3InspectionWarningKind::MalformedConfig,
                path,
                "YAML setting has no ':' separator",
            );
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            continue;
        }
        apply_yaml_setting(&mut settings, &section, key, value);
    }
    settings
}

fn apply_yaml_setting(settings: &mut Rpcs3Settings, section: &str, key: &str, value: &str) {
    let normalized_key = key.to_ascii_lowercase();
    let boolean = parse_bool(value);
    match (section, normalized_key.as_str()) {
        ("core", "ppu decoder") => settings.cpu_decoder = value_or_none(value),
        ("core", "spu decoder") => settings.spu_decoder = value_or_none(value),
        ("video", "renderer") => settings.renderer = value_or_none(value),
        ("video", "resolution scale") => settings.resolution_scale = value_or_none(value),
        ("core", "frame limit") | ("video", "frame limit") => {
            settings.frame_limit = value_or_none(value)
        }
        ("video", "vsync") => settings.vsync = boolean,
        ("video", "write color buffers") => settings.write_color_buffers = boolean,
        ("video", "strict rendering mode") => settings.strict_rendering_mode = boolean,
        ("audio", "audio backend") => settings.audio_backend = value_or_none(value),
        ("input/output", _) if key.to_ascii_lowercase().contains("player") => {
            settings.controller_profile_present = boolean.or(Some(true))
        }
        _ if settings.unknown.len() < MAX_RETAINED_UNKNOWN_SETTINGS => {
            settings
                .unknown
                .insert(format!("{section}/{key}"), value.to_string());
        }
        _ => {}
    }
}

fn value_or_none(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn differing_keys(global: &Rpcs3Settings, game: &Rpcs3Settings) -> Vec<String> {
    let global = flattened_settings(global);
    let game = flattened_settings(game);
    game.into_iter()
        .filter_map(|(key, value)| (global.get(&key) != Some(&value)).then_some(key))
        .collect()
}

fn flattened_settings(settings: &Rpcs3Settings) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    macro_rules! insert_opt {
        ($field:expr, $name:literal) => {
            if let Some(value) = &$field {
                map.insert($name.to_string(), value.to_string());
            }
        };
    }
    insert_opt!(settings.cpu_decoder, "cpu_decoder");
    insert_opt!(settings.spu_decoder, "spu_decoder");
    insert_opt!(settings.renderer, "renderer");
    insert_opt!(settings.resolution_scale, "resolution_scale");
    insert_opt!(settings.frame_limit, "frame_limit");
    if let Some(value) = settings.vsync {
        map.insert("vsync".to_string(), value.to_string());
    }
    if let Some(value) = settings.write_color_buffers {
        map.insert("write_color_buffers".to_string(), value.to_string());
    }
    if let Some(value) = settings.strict_rendering_mode {
        map.insert("strict_rendering_mode".to_string(), value.to_string());
    }
    insert_opt!(settings.audio_backend, "audio_backend");
    for (key, value) in &settings.unknown {
        map.insert(format!("unknown/{key}"), value.clone());
    }
    map
}

// ---------------------------------------------------------------------
// Patches (local inspection only - never enabled, never downloaded)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3PatchEntry {
    pub name: Option<String>,
    /// Best-effort, from the same local `patch.yml` block, never a
    /// separately-fetched enable-state source. `None` when this parser
    /// cannot reliably determine enablement for this entry.
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rpcs3PatchInventory {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub entries: Vec<Rpcs3PatchEntry>,
    pub enabled_count: usize,
    pub complete: bool,
    pub warnings: Vec<Rpcs3InspectionWarning>,
}

/// Locates `patches/patch.yml` and, if present, finds the block whose
/// top-level YAML key equals `title_id` (RPCS3's own convention: patch
/// definitions are keyed by title ID at the top level), then lists that
/// block's immediate `- Name: ...` style patch entries, bounded. A patch
/// entry's own name/title has zero authority over game identity - see
/// this module's own doc comment; this function never returns anything
/// that could be mistaken for a resolved title.
fn inspect_patches(patches_path: &Path, title_id: &str) -> Rpcs3PatchInventory {
    let path = patches_path.join("patch.yml");
    let mut warnings = Vec::new();
    let exists = path.exists();
    let Some(text) = read_text(&path, RPCS3_MAX_CONFIG_BYTES, &mut warnings) else {
        return Rpcs3PatchInventory {
            path,
            exists,
            readable: false,
            entries: Vec::new(),
            enabled_count: 0,
            complete: true,
            warnings,
        };
    };
    let entries = parse_patch_entries(&text, title_id, &path, &mut warnings);
    let enabled_count = entries
        .iter()
        .filter(|entry| entry.enabled == Some(true))
        .count();
    Rpcs3PatchInventory {
        path,
        exists,
        readable: true,
        enabled_count,
        complete: entries.len() < RPCS3_MAX_PATCH_ENTRIES,
        entries,
        warnings,
    }
}

/// Narrow, bounded scan: finds the top-level line matching `title_id:`,
/// then collects each following, more-indented `Name:`-bearing line until
/// indentation returns to a top-level key or the file ends. Never a
/// general YAML parser - see [`parse_yaml_settings`]'s own doc comment for
/// the same reasoning.
fn parse_patch_entries(
    text: &str,
    title_id: &str,
    path: &Path,
    warnings: &mut Vec<Rpcs3InspectionWarning>,
) -> Vec<Rpcs3PatchEntry> {
    let mut entries = Vec::new();
    let mut inside_title_block = false;
    for (index, raw) in text.lines().enumerate() {
        if index >= MAX_YAML_LINES {
            warn(
                warnings,
                Rpcs3InspectionWarningKind::LineCountLimitReached,
                path,
                format!("patch YAML parsing stopped at the {MAX_YAML_LINES}-line limit"),
            );
            break;
        }
        if entries.len() >= RPCS3_MAX_PATCH_ENTRIES {
            break;
        }
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let trimmed = raw.trim();
        if indent == 0 {
            inside_title_block = trimmed.trim_end_matches(':') == title_id;
            continue;
        }
        if !inside_title_block {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- Name:").or_else(|| {
            trimmed
                .strip_prefix('-')
                .map(str::trim)
                .and_then(|line| line.strip_prefix("Name:"))
        }) {
            entries.push(Rpcs3PatchEntry {
                name: value_or_none(rest.trim()),
                enabled: None,
            });
        } else if let Some(rest) = trimmed.strip_prefix("Enabled:")
            && let Some(last) = entries.last_mut()
        {
            last.enabled = parse_bool(rest.trim());
        }
    }
    entries
}

// ---------------------------------------------------------------------
// Firmware, saves/trophies
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "version")]
pub enum Rpcs3FirmwareStatus {
    Present(Option<String>),
    Missing,
    Unknown,
}

/// Bounded presence check only - never validates firmware authenticity or
/// downloads anything. Looks for the small set of files RPCS3 itself
/// writes/expects under `dev_flash` once firmware is installed.
fn inspect_firmware(dev_flash_path: &Path) -> Rpcs3FirmwareStatus {
    if !is_real_directory(dev_flash_path) {
        return Rpcs3FirmwareStatus::Unknown;
    }
    let marker_present = [
        dev_flash_path.join("vsh/module/vsh.self"),
        dev_flash_path.join("sys/external/liblv2.sprx"),
    ]
    .iter()
    .any(|path| is_regular_file(path));
    if !marker_present {
        return Rpcs3FirmwareStatus::Missing;
    }
    let version = read_small_file(&dev_flash_path.join("vsh/etc/version.txt"))
        .and_then(|text| text.lines().next().map(str::trim).map(str::to_string))
        .filter(|text| !text.is_empty() && text.len() <= 32);
    Rpcs3FirmwareStatus::Present(version)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rpcs3SaveTrophyInventory {
    pub save_data_found: bool,
    pub trophies_found: bool,
    pub warnings: Vec<Rpcs3InspectionWarning>,
}

/// Conservative presence-only detection under RPCS3's conventional
/// `dev_hdd0/home/<user_id>/{savedata,trophy}` layout, bounded. This
/// module makes no attempt to map an individual save/trophy directory to
/// a specific title without a reliable title-ID relationship - it only
/// reports whether *any* save/trophy data exists at all for the default
/// user.
fn inspect_save_trophy(dev_hdd0_path: &Path) -> Rpcs3SaveTrophyInventory {
    let mut warnings = Vec::new();
    let home_root = dev_hdd0_path.join("home");
    let mut save_data_found = false;
    let mut trophies_found = false;
    let Ok(read_dir) = fs::read_dir(&home_root) else {
        return Rpcs3SaveTrophyInventory {
            save_data_found,
            trophies_found,
            warnings,
        };
    };
    for (visited, user_entry) in read_dir.flatten().enumerate() {
        if visited >= RPCS3_MAX_SAVE_TROPHY_CANDIDATES {
            warn(
                &mut warnings,
                Rpcs3InspectionWarningKind::EntryLimitReached,
                &home_root,
                format!(
                    "save/trophy scan stopped at the {RPCS3_MAX_SAVE_TROPHY_CANDIDATES}-user limit"
                ),
            );
            break;
        }
        let user_path = user_entry.path();
        if !is_real_directory(&user_path) {
            continue;
        }
        if directory_has_any_entry(&user_path.join("savedata")) {
            save_data_found = true;
        }
        if directory_has_any_entry(&user_path.join("trophy")) {
            trophies_found = true;
        }
    }
    Rpcs3SaveTrophyInventory {
        save_data_found,
        trophies_found,
        warnings,
    }
}

fn directory_has_any_entry(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------
// Health + top-level inspection
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3Health {
    pub detected: bool,
    pub config_readable: bool,
    pub dev_hdd0_readable: bool,
    pub firmware: Rpcs3FirmwareStatus,
    pub patch_data_available: bool,
    pub title_id_mapping: Rpcs3GameIdMapping,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3GameInspection {
    pub title_id: Option<String>,
    pub title_id_mapping: Rpcs3GameIdMapping,
    pub base_game: Option<Rpcs3InstalledGame>,
    pub disc_game: Option<Rpcs3InstalledGame>,
    pub update: Rpcs3UpdateInfo,
    pub dlc: Rpcs3DlcInventory,
    pub per_game_config: Option<Rpcs3PerGameConfig>,
    pub overridden_setting_keys: Vec<String>,
    pub patches: Option<Rpcs3PatchInventory>,
    pub save_trophy: Rpcs3SaveTrophyInventory,
    pub health: Rpcs3Health,
}

/// Inspects RPCS3 assets only after the caller separates an
/// already-verified PS3 title ID from an RPCS3-observed one. This
/// function emits no core identity evidence and cannot upgrade either
/// value; see this module's own doc comment.
pub fn inspect_rpcs3_game(
    profile: &Rpcs3Profile,
    request: &Rpcs3GameRequest,
) -> Rpcs3GameInspection {
    let (title_id, title_id_mapping) = select_title_id(request);
    let mut warnings = Vec::new();

    let base_game = title_id.as_ref().and_then(|id| {
        inspect_installed_game(id, &profile.games_path.join(id), false, &mut warnings)
    });
    let disc_game = if base_game.is_none() {
        title_id.as_ref().and_then(|id| {
            [
                profile.configuration_path.join("disc").join(id),
                profile.configuration_path.join("games").join(id),
            ]
            .into_iter()
            .find_map(|candidate| inspect_installed_game(id, &candidate, true, &mut warnings))
        })
    } else {
        None
    };
    let update = inspect_update(base_game.as_ref().or(disc_game.as_ref()));
    let dlc = title_id
        .as_ref()
        .map(|id| inspect_dlc(&profile.games_path, id, &mut warnings))
        .unwrap_or_default();
    let per_game_config = title_id
        .as_ref()
        .map(|id| inspect_per_game_config(&profile.custom_configs_path.join(format!("{id}.yml"))));
    let global_config = inspect_per_game_config(&profile.global_config_path);
    let overridden_setting_keys = per_game_config
        .as_ref()
        .map(|config| differing_keys(&global_config.settings, &config.settings))
        .unwrap_or_default();
    let patches = title_id
        .as_ref()
        .map(|id| inspect_patches(&profile.patches_path, id));
    let save_trophy = inspect_save_trophy(&profile.dev_hdd0_path);
    let firmware = inspect_firmware(&profile.dev_flash_path);

    let mut health_warnings: Vec<String> = profile
        .blockers
        .iter()
        .map(|blocker| blocker.detail.clone())
        .collect();
    for warning in &global_config.warnings {
        health_warnings.push(warning.detail.clone());
    }
    let health = Rpcs3Health {
        detected: profile.eligible || !profile.executable_candidates.is_empty(),
        config_readable: global_config.readable,
        dev_hdd0_readable: is_real_directory(&profile.dev_hdd0_path),
        firmware,
        patch_data_available: is_regular_file(&profile.patches_path.join("patch.yml")),
        title_id_mapping,
        warnings: health_warnings,
    };

    Rpcs3GameInspection {
        title_id,
        title_id_mapping,
        base_game,
        disc_game,
        update,
        dlc,
        per_game_config,
        overridden_setting_keys,
        patches,
        save_trophy,
        health,
    }
}

// ---------------------------------------------------------------------
// Shared low-level helpers
// ---------------------------------------------------------------------

fn read_sfo(path: &Path, warnings: &mut Vec<Rpcs3InspectionWarning>) -> Option<SfoObservation> {
    let bytes = read_bytes(path, crate::param_sfo::MAX_SFO_BYTES as u64, warnings)?;
    let sfo = parse_param_sfo(&bytes);
    if sfo.is_none() {
        warn(
            warnings,
            Rpcs3InspectionWarningKind::MalformedSfo,
            path,
            "PARAM.SFO could not be parsed",
        );
    }
    if let Some(observed) = &sfo
        && observed.entries.len() > RPCS3_MAX_SFO_SCAN_ENTRIES
    {
        return None;
    }
    sfo
}

fn read_bytes(
    path: &Path,
    maximum_bytes: u64,
    warnings: &mut Vec<Rpcs3InspectionWarning>,
) -> Option<Vec<u8>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            warn(
                warnings,
                Rpcs3InspectionWarningKind::UnreadablePath,
                path,
                format!("file cannot be inspected: {error}"),
            );
            return None;
        }
    };
    if metadata.file_type().is_symlink() {
        warn(
            warnings,
            Rpcs3InspectionWarningKind::SymlinkSkipped,
            path,
            "symlink was not followed",
        );
        return None;
    }
    if !metadata.is_file() {
        warn(
            warnings,
            Rpcs3InspectionWarningKind::SpecialFileSkipped,
            path,
            "non-regular file was skipped",
        );
        return None;
    }
    if metadata.len() > maximum_bytes {
        warn(
            warnings,
            Rpcs3InspectionWarningKind::FileTooLarge,
            path,
            format!("file exceeds the {maximum_bytes}-byte limit"),
        );
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) => {
            warn(
                warnings,
                Rpcs3InspectionWarningKind::UnreadablePath,
                path,
                format!("file cannot be opened read-only: {error}"),
            );
            return None;
        }
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if let Err(error) = file
        .by_ref()
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
    {
        warn(
            warnings,
            Rpcs3InspectionWarningKind::UnreadablePath,
            path,
            format!("file cannot be read: {error}"),
        );
        return None;
    }
    if bytes.len() as u64 > maximum_bytes {
        warn(
            warnings,
            Rpcs3InspectionWarningKind::FileTooLarge,
            path,
            format!("file grew beyond the {maximum_bytes}-byte limit while reading"),
        );
        return None;
    }
    Some(bytes)
}

fn read_text(
    path: &Path,
    maximum_bytes: u64,
    warnings: &mut Vec<Rpcs3InspectionWarning>,
) -> Option<String> {
    let bytes = read_bytes(path, maximum_bytes, warnings)?;
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) => {
            warn(
                warnings,
                Rpcs3InspectionWarningKind::InvalidUtf8,
                path,
                "file is not valid UTF-8; invalid bytes were replaced for parsing",
            );
            Some(String::from_utf8_lossy(error.as_bytes()).into_owned())
        }
    }
}

/// Small, best-effort read for a short marker/version file - failures are
/// silent (`None`) since callers only ever use this for optional display
/// text, never a gating decision.
fn read_small_file(path: &Path) -> Option<String> {
    let mut warnings = Vec::new();
    read_text(path, 4 * 1024, &mut warnings)
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn warn(
    warnings: &mut Vec<Rpcs3InspectionWarning>,
    kind: Rpcs3InspectionWarningKind,
    path: &Path,
    detail: impl Into<String>,
) {
    if !warnings
        .iter()
        .any(|warning| warning.kind == kind && warning.path == path)
    {
        warnings.push(Rpcs3InspectionWarning {
            kind,
            path: path.to_path_buf(),
            detail: detail.into(),
        });
    }
}

fn blocker(
    kind: Rpcs3ProfileBlockerKind,
    path: &Path,
    detail: impl Into<String>,
) -> Rpcs3ProfileBlocker {
    Rpcs3ProfileBlocker {
        kind,
        path: EncodedPath::from_path(path),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;

    use super::*;

    // -------------------------------------------------------------------
    // Fixture helpers
    // -------------------------------------------------------------------

    struct SfoBuilder {
        entries: Vec<(String, u16, Vec<u8>)>,
    }

    const FORMAT_UTF8: u16 = 0x0204;

    impl SfoBuilder {
        fn new() -> Self {
            Self {
                entries: Vec::new(),
            }
        }

        fn text(mut self, key: &str, value: &str) -> Self {
            let mut raw = value.as_bytes().to_vec();
            raw.push(0);
            self.entries.push((key.to_string(), FORMAT_UTF8, raw));
            self
        }

        fn build(self) -> Vec<u8> {
            const HEADER_BYTES: usize = 20;
            const INDEX_ENTRY_BYTES: usize = 16;
            let index_table_len = self.entries.len() * INDEX_ENTRY_BYTES;
            let key_table_start = HEADER_BYTES + index_table_len;

            let mut key_table = Vec::new();
            let mut key_offsets = Vec::new();
            for (key, _, _) in &self.entries {
                key_offsets.push(key_table.len() as u16);
                key_table.extend_from_slice(key.as_bytes());
                key_table.push(0);
            }
            while key_table.len() % 4 != 0 {
                key_table.push(0);
            }

            let data_table_start = key_table_start + key_table.len();
            let mut data_table = Vec::new();
            let mut data_offsets = Vec::new();
            for (_, _, raw) in &self.entries {
                data_offsets.push(data_table.len() as u32);
                data_table.extend_from_slice(raw);
            }

            let mut out = vec![0u8; HEADER_BYTES];
            out[0..4].copy_from_slice(&[0x00, b'P', b'S', b'F']);
            out[4..8].copy_from_slice(&0x0101_u32.to_le_bytes());
            out[8..12].copy_from_slice(&(key_table_start as u32).to_le_bytes());
            out[12..16].copy_from_slice(&(data_table_start as u32).to_le_bytes());
            out[16..20].copy_from_slice(&(self.entries.len() as u32).to_le_bytes());

            for (index, (_, data_fmt, raw)) in self.entries.iter().enumerate() {
                let mut entry = [0u8; INDEX_ENTRY_BYTES];
                entry[0..2].copy_from_slice(&key_offsets[index].to_le_bytes());
                entry[2..4].copy_from_slice(&data_fmt.to_le_bytes());
                entry[4..8].copy_from_slice(&(raw.len() as u32).to_le_bytes());
                entry[8..12].copy_from_slice(&(raw.len() as u32).to_le_bytes());
                entry[12..16].copy_from_slice(&data_offsets[index].to_le_bytes());
                out.extend_from_slice(&entry);
            }
            out.extend_from_slice(&key_table);
            out.extend_from_slice(&data_table);
            out
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(bytes).unwrap();
    }

    fn write_sfo(path: &Path, title_id: &str, title: &str, app_ver: &str, category: &str) {
        let bytes = SfoBuilder::new()
            .text("TITLE_ID", title_id)
            .text("TITLE", title)
            .text("APP_VER", app_ver)
            .text("CATEGORY", category)
            .build();
        write_file(path, &bytes);
    }

    fn roots_for(config: &Path) -> Rpcs3ProfileDiscoveryRoots {
        Rpcs3ProfileDiscoveryRoots {
            home: config.to_path_buf(),
            xdg_config_home: config.to_path_buf(),
            xdg_data_home: config.join("data-unused"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        }
    }

    fn native_profile(dir: &TempDir) -> Rpcs3Profile {
        let config = dir.path().join("rpcs3");
        std::fs::create_dir_all(config.join("dev_hdd0")).unwrap();
        // `roots_for` supplies the XDG_CONFIG_HOME-style parent; discovery
        // itself appends the "rpcs3" segment.
        let roots = roots_for(dir.path());
        let discovery = discover_rpcs3_profiles(&roots);
        discovery
            .profiles
            .into_iter()
            .find(|profile| profile.configuration_path == config)
            .expect("native profile discovered")
    }

    // -------------------------------------------------------------------
    // 1-4: discovery / layouts
    // -------------------------------------------------------------------

    #[test]
    fn native_layout_is_discovered_and_eligible() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        assert!(profile.eligible, "{:?}", profile.blockers);
        assert_eq!(profile.installation_type, Rpcs3InstallationType::Native);
        assert!(profile.dev_hdd0_path.ends_with("dev_hdd0"));
    }

    #[test]
    fn appimage_layout_is_discovered_via_portable_root() {
        let dir = TempDir::new().unwrap();
        let config = dir.path().join("Portable/config");
        std::fs::create_dir_all(config.join("dev_hdd0")).unwrap();
        let mut roots = roots_for(&dir.path().join("unused"));
        roots.portable_configuration_roots = vec![config.clone()];
        let discovery = discover_rpcs3_profiles(&roots);
        let profile = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_path == config)
            .unwrap();
        assert!(profile.eligible);
        assert_eq!(profile.installation_type, Rpcs3InstallationType::Portable);
    }

    #[test]
    fn flatpak_layout_is_discovered() {
        let dir = TempDir::new().unwrap();
        let config = dir
            .path()
            .join(".var/app")
            .join(FLATPAK_APP_ID)
            .join("config/rpcs3");
        std::fs::create_dir_all(config.join("dev_hdd0")).unwrap();
        let roots = roots_for(dir.path());
        let discovery = discover_rpcs3_profiles(&roots);
        let profile = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_path == config)
            .unwrap();
        assert!(profile.eligible);
        assert_eq!(
            profile.installation_type,
            Rpcs3InstallationType::FlatpakUser
        );
    }

    #[test]
    fn a_profile_with_neither_evidence_nor_an_executable_is_reported_not_detected() {
        let dir = TempDir::new().unwrap();
        // No dev_hdd0/config.yml evidence and no executable anywhere -
        // discovery must not report this candidate as eligible or found.
        let roots = roots_for(dir.path());
        let discovery = discover_rpcs3_profiles(&roots);
        assert!(discovery.profiles.is_empty());
    }

    #[test]
    fn an_eligible_profile_is_detected_even_without_a_discovered_executable() {
        // Mirrors PPSSPP's own `detected = eligible || executable found`
        // semantics: a real dev_hdd0/ is itself enough evidence, since
        // discovery never executes a binary to confirm this.
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        assert!(profile.executable_candidates.is_empty());
        let request = Rpcs3GameRequest::default();
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert!(inspection.health.detected);
    }

    // -------------------------------------------------------------------
    // 5-6: version parsing
    // -------------------------------------------------------------------

    #[test]
    fn known_version_string_parses() {
        assert_eq!(
            parse_rpcs3_version("rpcs3-v0.0.31-15000-abcdef1 Alpha"),
            Some("0.0.31".to_string())
        );
    }

    #[test]
    fn unknown_version_shape_fails_soft() {
        assert_eq!(parse_rpcs3_version("some unrelated tool v9"), None);
        assert_eq!(parse_rpcs3_version(""), None);
    }

    // -------------------------------------------------------------------
    // 7-8: PARAM.SFO parsing (valid / malformed)
    // -------------------------------------------------------------------

    #[test]
    fn valid_param_sfo_is_read_for_the_base_game() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Test PS3 Game",
            "01.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        let base = inspection.base_game.expect("base game found");
        assert_eq!(base.display_title.as_deref(), Some("Test PS3 Game"));
    }

    #[test]
    fn malformed_param_sfo_fails_soft_without_a_base_game() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_file(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            b"not an sfo file",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert!(inspection.base_game.is_none());
    }

    // -------------------------------------------------------------------
    // 9-11: identity mapping / safety
    // -------------------------------------------------------------------

    #[test]
    fn verified_title_id_maps_assets_but_emulator_id_has_no_identity_authority() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Real Verified Title",
            "01.00",
            "HG",
        );
        write_sfo(
            &profile.games_path.join("BLES99999/PARAM.SFO"),
            "BLES99999",
            "Wrong Emulator-Observed Title",
            "01.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: Some("BLES99999".to_string()),
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(
            inspection.title_id_mapping,
            Rpcs3GameIdMapping::VerifiedPs3TitleId
        );
        assert_eq!(inspection.title_id.as_deref(), Some("BLUS30000"));
        assert_eq!(
            inspection.base_game.unwrap().display_title.as_deref(),
            Some("Real Verified Title")
        );
    }

    #[test]
    fn unresolved_identity_still_maps_via_emulator_metadata_only() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLES99999/PARAM.SFO"),
            "BLES99999",
            "Emulator Observed Title",
            "01.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: None,
            emulator_game_id: Some("BLES99999".to_string()),
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(
            inspection.title_id_mapping,
            Rpcs3GameIdMapping::EmulatorMetadataOnly
        );
        assert!(inspection.base_game.is_some());
    }

    #[test]
    fn conflicting_preservation_identity_yields_no_mapping_at_all() {
        // A caller whose own identity resolution is in Conflict must pass
        // no `verified_ps3_title_id` - this module never chooses a side.
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        let request = Rpcs3GameRequest::default();
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(inspection.title_id_mapping, Rpcs3GameIdMapping::Unavailable);
        assert!(inspection.title_id.is_none());
        assert!(inspection.base_game.is_none());
    }

    #[test]
    fn param_sfo_title_field_has_zero_verification_authority() {
        // `PpssppGameInspection`'s PS3 twin: `Rpcs3InstalledGame` carries a
        // display_title, but nothing in this module ever promotes it to a
        // `title_id`/identity value.
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "BLES99999", // TITLE field maliciously names a different serial
            "01.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(inspection.title_id.as_deref(), Some("BLUS30000"));
    }

    #[test]
    fn directory_name_has_zero_identity_authority() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        // The directory is named after a plausible-looking but different
        // serial than the SFO's own TITLE_ID.
        write_sfo(
            &profile.games_path.join("NOT-A-REAL-ID/PARAM.SFO"),
            "BLUS30000",
            "Test",
            "01.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        // Lookup is keyed by the verified title id as a path component,
        // never a directory name scan - the mismatched directory is never
        // found or trusted.
        assert!(inspection.base_game.is_none());
    }

    // -------------------------------------------------------------------
    // 12-13: base game / disc-style game
    // -------------------------------------------------------------------

    #[test]
    fn installed_base_game_is_detected() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Test",
            "01.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert!(inspection.base_game.is_some());
        assert!(inspection.disc_game.is_none());
    }

    #[test]
    fn disc_style_game_is_detected_when_no_hdd_install_exists() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.configuration_path.join("disc/BLUS30001/PARAM.SFO"),
            "BLUS30001",
            "Disc Style Game",
            "01.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30001".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert!(inspection.base_game.is_none());
        let disc = inspection.disc_game.expect("disc-style game found");
        assert!(disc.disc_style);
        assert_eq!(disc.display_title.as_deref(), Some("Disc Style Game"));
    }

    // -------------------------------------------------------------------
    // 14-15: update detection
    // -------------------------------------------------------------------

    #[test]
    fn an_updated_app_version_is_reported_as_an_installed_update() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Test",
            "01.05",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert!(inspection.update.detected);
        assert_eq!(inspection.update.version.as_deref(), Some("01.05"));
    }

    #[test]
    fn a_baseline_app_version_is_not_reported_as_an_update() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Test",
            "01.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert!(!inspection.update.detected);
    }

    #[test]
    fn update_metadata_never_rewrites_base_game_title_id() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Test",
            "02.00",
            "HG",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(inspection.title_id.as_deref(), Some("BLUS30000"));
        assert_eq!(inspection.base_game.as_ref().unwrap().title_id, "BLUS30000");
    }

    // -------------------------------------------------------------------
    // 16: DLC detection
    // -------------------------------------------------------------------

    #[test]
    fn dlc_content_is_detected_via_content_id_never_directory_name() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Base Game",
            "01.00",
            "HG",
        );
        // A content-ID whose derived title id matches the base game -
        // real DLC. Directory name is unrelated to the title id on
        // purpose.
        let sfo = SfoBuilder::new()
            .text("CONTENT_ID", "UP0001-BLUS30000_00-DLCPACK0000001")
            .text("TITLE_ID", "BLUS30000DLC1")
            .text("TITLE", "Cool DLC Pack")
            .build();
        write_file(
            &profile.games_path.join("some-random-dlc-dir/PARAM.SFO"),
            &sfo,
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(inspection.dlc.count, 1);
        assert_eq!(
            inspection.dlc.entries[0].display_title.as_deref(),
            Some("Cool DLC Pack")
        );
    }

    #[test]
    fn dlc_metadata_never_rewrites_base_game_identity() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Base Game",
            "01.00",
            "HG",
        );
        let sfo = SfoBuilder::new()
            .text("CONTENT_ID", "UP0001-BLUS30000_00-DLCPACK0000001")
            .text("TITLE_ID", "BLUS30000DLC1")
            .text("TITLE", "Not The Real Base Game")
            .build();
        write_file(&profile.games_path.join("dlc-dir/PARAM.SFO"), &sfo);
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(
            inspection.base_game.unwrap().display_title.as_deref(),
            Some("Base Game")
        );
    }

    // -------------------------------------------------------------------
    // 17-18: per-game YAML config
    // -------------------------------------------------------------------

    #[test]
    fn per_game_yaml_config_is_parsed() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_file(
            &profile.custom_configs_path.join("BLUS30000.yml"),
            b"Core:\n  PPU Decoder: Recompiler (LLVM)\n  SPU Decoder: Recompiler (LLVM)\nVideo:\n  Renderer: Vulkan\n  Resolution Scale: 150\n  VSync: false\n",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        let config = inspection.per_game_config.unwrap();
        assert_eq!(
            config.settings.cpu_decoder.as_deref(),
            Some("Recompiler (LLVM)")
        );
        assert_eq!(config.settings.renderer.as_deref(), Some("Vulkan"));
        assert_eq!(config.settings.vsync, Some(false));
    }

    #[test]
    fn malformed_yaml_fails_soft() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_file(
            &profile.custom_configs_path.join("BLUS30000.yml"),
            b"Core:\n  this line has no colon separator at all\n  SPU Decoder: Interpreter\n",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        let config = inspection.per_game_config.unwrap();
        assert!(config.readable);
        assert_eq!(config.settings.spu_decoder.as_deref(), Some("Interpreter"));
        assert!(
            config
                .warnings
                .iter()
                .any(|warning| warning.kind == Rpcs3InspectionWarningKind::MalformedConfig)
        );
    }

    // -------------------------------------------------------------------
    // 19-20: patches
    // -------------------------------------------------------------------

    #[test]
    fn local_patch_data_is_inspected() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_file(
            &profile.patches_path.join("patch.yml"),
            b"BLUS30000:\n  - Name: 60 FPS Patch\n    Enabled: true\n  - Name: Widescreen Patch\n    Enabled: false\n",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        let patches = inspection.patches.unwrap();
        assert_eq!(patches.entries.len(), 2);
        assert_eq!(patches.enabled_count, 1);
    }

    #[test]
    fn patch_title_key_has_zero_identity_authority() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Real Base Game",
            "01.00",
            "HG",
        );
        // A patch.yml block keyed by a totally different id - must never
        // influence the selected title.
        write_file(
            &profile.patches_path.join("patch.yml"),
            b"BLES99999:\n  - Name: Unrelated Patch\n    Enabled: true\n",
        );
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(inspection.title_id.as_deref(), Some("BLUS30000"));
        assert_eq!(
            inspection.base_game.unwrap().display_title.as_deref(),
            Some("Real Base Game")
        );
        // No patches for BLUS30000's own block exist, so none are found.
        assert!(inspection.patches.unwrap().entries.is_empty());
    }

    // -------------------------------------------------------------------
    // 21-22: firmware
    // -------------------------------------------------------------------

    #[test]
    fn firmware_present_is_detected() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_file(&profile.dev_flash_path.join("vsh/module/vsh.self"), b"x");
        write_file(
            &profile.dev_flash_path.join("vsh/etc/version.txt"),
            b"4.91\n",
        );
        let inspection = inspect_rpcs3_game(&profile, &Rpcs3GameRequest::default());
        assert_eq!(
            inspection.health.firmware,
            Rpcs3FirmwareStatus::Present(Some("4.91".to_string()))
        );
    }

    #[test]
    fn firmware_missing_is_detected() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        std::fs::create_dir_all(&profile.dev_flash_path).unwrap();
        let inspection = inspect_rpcs3_game(&profile, &Rpcs3GameRequest::default());
        assert_eq!(inspection.health.firmware, Rpcs3FirmwareStatus::Missing);
    }

    #[test]
    fn firmware_unknown_when_dev_flash_absent() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        let inspection = inspect_rpcs3_game(&profile, &Rpcs3GameRequest::default());
        assert_eq!(inspection.health.firmware, Rpcs3FirmwareStatus::Unknown);
    }

    // -------------------------------------------------------------------
    // 23: save/trophy presence
    // -------------------------------------------------------------------

    #[test]
    fn save_and_trophy_presence_is_detected_conservatively() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_file(
            &profile
                .dev_hdd0_path
                .join("home/00000001/savedata/BLUS30000-SAVE0/PARAM.SFO"),
            b"x",
        );
        write_file(
            &profile
                .dev_hdd0_path
                .join("home/00000001/trophy/TROPHY.TRP"),
            b"x",
        );
        let inspection = inspect_rpcs3_game(&profile, &Rpcs3GameRequest::default());
        assert!(inspection.save_trophy.save_data_found);
        assert!(inspection.save_trophy.trophies_found);
    }

    #[test]
    fn no_save_or_trophy_data_is_reported_honestly() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        let inspection = inspect_rpcs3_game(&profile, &Rpcs3GameRequest::default());
        assert!(!inspection.save_trophy.save_data_found);
        assert!(!inspection.save_trophy.trophies_found);
    }

    // -------------------------------------------------------------------
    // 24: bounded traversal
    // -------------------------------------------------------------------

    #[test]
    fn dlc_scan_is_bounded_and_reports_incompleteness() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        write_sfo(
            &profile.games_path.join("BLUS30000/PARAM.SFO"),
            "BLUS30000",
            "Base Game",
            "01.00",
            "HG",
        );
        for index in 0..(RPCS3_MAX_DLC_ENTRIES + 5) {
            let sfo = SfoBuilder::new()
                .text(
                    "CONTENT_ID",
                    &format!("UP0001-BLUS30000_00-DLCPACK{index:07}"),
                )
                .text("TITLE_ID", &format!("BLUS30000DLC{index}"))
                .text("TITLE", "A DLC Pack")
                .build();
            write_file(
                &profile.games_path.join(format!("dlc-{index:04}/PARAM.SFO")),
                &sfo,
            );
        }
        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let inspection = inspect_rpcs3_game(&profile, &request);
        assert_eq!(inspection.dlc.count, RPCS3_MAX_DLC_ENTRIES);
        assert!(!inspection.dlc.complete);
    }

    // -------------------------------------------------------------------
    // No mutation
    // -------------------------------------------------------------------

    #[test]
    fn inspection_never_mutates_any_file_it_reads() {
        let dir = TempDir::new().unwrap();
        let profile = native_profile(&dir);
        let sfo_path = profile.games_path.join("BLUS30000/PARAM.SFO");
        write_sfo(&sfo_path, "BLUS30000", "Test", "01.00", "HG");
        let before = std::fs::read(&sfo_path).unwrap();
        let before_meta = std::fs::metadata(&sfo_path).unwrap();

        let request = Rpcs3GameRequest {
            verified_ps3_title_id: Some("BLUS30000".to_string()),
            emulator_game_id: None,
        };
        let _ = inspect_rpcs3_game(&profile, &request);

        let after = std::fs::read(&sfo_path).unwrap();
        let after_meta = std::fs::metadata(&sfo_path).unwrap();
        assert_eq!(before, after);
        assert_eq!(before_meta.len(), after_meta.len());
    }
}
