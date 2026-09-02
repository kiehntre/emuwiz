//! Bounded, read-only discovery and inspection of local PPSSPP profiles.
//!
//! PPSSPP is intentionally treated as a consumer of PSP identity.  Its
//! directory names and INI filenames are useful emulator context, never game
//! identity evidence.  This module never starts PPSSPP, writes a file, follows
//! a symlink, or traverses outside documented/configured local roots.

use std::collections::{BTreeMap, VecDeque};
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

use super::destination_safety::{
    DestinationRootState, DestinationSafetyFailureReason, validate_destination_root,
};

pub const PPSSPP_MAX_PROFILES: usize = 16;
pub const PPSSPP_MAX_CONFIG_BYTES: u64 = 256 * 1024;
pub const PPSSPP_MAX_CHEAT_BYTES: u64 = 512 * 1024;
pub const PPSSPP_MAX_CHEAT_ENTRIES: usize = 1_024;
pub const PPSSPP_MAX_ENTRIES_VISITED: usize = 10_000;
pub const PPSSPP_MAX_TEXTURE_FILES: usize = 2_048;
pub const PPSSPP_MAX_TEXTURE_DEPTH: usize = 2;
pub const PPSSPP_MAX_SAVEDATA_CANDIDATES: usize = 128;

const FLATPAK_APP_ID: &str = "org.ppsspp.PPSSPP";
const MAX_INI_LINES: usize = 8_192;
const MAX_INI_LINE_BYTES: usize = 8 * 1024;
const MAX_RETAINED_UNKNOWN_SETTINGS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppProfileScope {
    User,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppProfileBlockerKind {
    PathNotAbsolute,
    FilesystemRoot,
    MissingConfiguration,
    UnsafePath,
    NotDirectory,
    Unreadable,
    MissingPpssppEvidence,
    ProfileLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PpssppProfileBlocker {
    pub kind: PpssppProfileBlockerKind,
    pub path: EncodedPath,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppInspectionWarningKind {
    UnsafePath,
    UnreadablePath,
    SymlinkSkipped,
    SpecialFileSkipped,
    EntryLimitReached,
    FileCountLimitReached,
    DepthLimitReached,
    FileTooLarge,
    InvalidUtf8,
    MalformedIni,
    LineCountLimitReached,
    LineTooLong,
    CheatEntryLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppInspectionWarning {
    pub kind: PpssppInspectionWarningKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppExecutable {
    pub path: PathBuf,
    pub installation_type: PpssppInstallationType,
    /// Deliberately optional: discovery never executes a user binary.  A
    /// caller that has already obtained read-only version text can use
    /// [`parse_ppsspp_version`] without changing this safety boundary.
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppProfile {
    pub profile_id: String,
    pub installation_type: PpssppInstallationType,
    pub scope: PpssppProfileScope,
    /// The directory containing PPSSPP's `PSP` memstick directory.
    pub configuration_path: PathBuf,
    pub provenance: &'static str,
    pub eligible: bool,
    pub blockers: Vec<PpssppProfileBlocker>,
    pub executable_candidates: Vec<PpssppExecutable>,
    pub memstick_path: PathBuf,
    pub system_path: PathBuf,
    pub global_config_path: PathBuf,
    pub cheats_path: PathBuf,
    pub textures_path: PathBuf,
    pub savedata_path: PathBuf,
    pub game_path: PathBuf,
    pub state_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppProfileDiscovery {
    pub profiles: Vec<PpssppProfile>,
    pub warnings: Vec<PpssppProfileBlocker>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    /// Exact configuration roots explicitly configured by the user or a
    /// higher-level settings layer.  They are never discovered by scanning.
    pub explicit_configuration_roots: Vec<PathBuf>,
    /// Exact known portable/AppImage directories.  PPSSPP portable layouts
    /// are only inspected when supplied by the caller or `APPIMAGE`.
    pub portable_configuration_roots: Vec<PathBuf>,
    /// Exact known executables; useful for configured AppImage/custom paths.
    pub explicit_executables: Vec<PathBuf>,
    /// Version text obtained by an already-authorized outer probe.  Discovery
    /// itself never executes the binary: this keeps this local inspector
    /// read-only while still allowing a host integration to surface a parsed
    /// version when it has one.
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
}

impl PpssppProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, PpssppDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(PpssppDiscoveryError::HomeUnavailable)?;
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
pub enum PpssppDiscoveryError {
    HomeUnavailable,
}

impl std::fmt::Display for PpssppDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable => formatter.write_str("HOME is not set"),
        }
    }
}

impl std::error::Error for PpssppDiscoveryError {}

/// A deliberately separate input lane for identity supplied by core and an
/// identifier merely observed in PPSSPP context.  Neither filename nor any
/// inspected directory can construct `verified_psp_disc_id`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PpssppGameRequest {
    pub verified_psp_disc_id: Option<String>,
    pub emulator_game_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppGameIdMapping {
    VerifiedPspDiscId,
    EmulatorMetadataOnly,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PpssppSettings {
    pub backend: Option<String>,
    pub internal_resolution: Option<String>,
    pub frame_skip: Option<String>,
    pub speed_limit: Option<String>,
    pub alternative_speed: Option<String>,
    pub anisotropic_filtering: Option<String>,
    pub texture_filtering: Option<String>,
    pub texture_scaling: Option<String>,
    pub audio_enabled: Option<bool>,
    pub audio_latency: Option<String>,
    pub language: Option<String>,
    pub cheats_enabled: Option<bool>,
    pub texture_replacements_enabled: Option<bool>,
    pub controller_config_present: Option<bool>,
    /// Unknown values are retained in bounded form for later UI display.
    pub unknown: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppGlobalConfig {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub settings: PpssppSettings,
    pub warnings: Vec<PpssppInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppCheatInventory {
    pub path: PathBuf,
    pub exists: bool,
    pub entries: usize,
    pub enabled_entries: usize,
    pub warnings: Vec<PpssppInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppTextureInventory {
    pub path: PathBuf,
    pub present: bool,
    pub texture_ini_present: bool,
    pub file_count: usize,
    pub total_size_bytes: u64,
    pub complete: bool,
    pub warnings: Vec<PpssppInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppSaveDataInventory {
    pub path: PathBuf,
    /// These are only conservatively named candidates (exactly the verified
    /// disc ID or that ID followed by a title-specific suffix), not proof a
    /// directory maps one-to-one to a title.
    pub candidate_paths: Vec<PathBuf>,
    pub complete: bool,
    pub warnings: Vec<PpssppInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppHealth {
    pub detected: bool,
    pub config_readable: bool,
    pub memstick_readable: bool,
    pub cheats_directory_available: bool,
    pub textures_directory_available: bool,
    pub game_profile_mapping: PpssppGameIdMapping,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppGameInspection {
    pub game_id: Option<String>,
    pub game_id_mapping: PpssppGameIdMapping,
    pub global_config: PpssppGlobalConfig,
    pub per_game_config: Option<PpssppGlobalConfig>,
    pub overridden_setting_keys: Vec<String>,
    pub cheats: Option<PpssppCheatInventory>,
    pub textures: Option<PpssppTextureInventory>,
    pub savedata: Option<PpssppSaveDataInventory>,
    pub health: PpssppHealth,
}

// ---------------------------------------------------------------------------
// Native launch binding
// ---------------------------------------------------------------------------

/// A freshly checked PPSSPP executable - either a distro/native binary or a
/// caller-confirmed explicit path (for example a local AppImage the host
/// integration has already verified). PPSSPP's application accepts a game
/// path as a positional argument; this binding deliberately contains only
/// the executable fact and never a shell command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppNativeLaunchBinding {
    pub executable: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppLaunchBlockerKind {
    UnsupportedInstallationType,
    ProfileIneligible,
    AmbiguousExecutable,
    ExecutableMissing,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppLaunchBlocker {
    pub kind: PpssppLaunchBlockerKind,
    pub detail: String,
}

fn launch_blocker(kind: PpssppLaunchBlockerKind, detail: impl Into<String>) -> PpssppLaunchBlocker {
    PpssppLaunchBlocker {
        kind,
        detail: detail.into(),
    }
}

/// Revalidates one discovered profile and proves exactly one safe
/// executable.
///
/// Which executable provenances a profile may bind, all held to the
/// *identical* file-safety checks ([`validate_native_ppsspp_executable`]:
/// absolute path, present, regular non-symlink file, execute bit) and the
/// identical "exactly one candidate" rule:
///
/// * [`PpssppInstallationType::Native`] - a plausible PPSSPP binary name
///   discovered on `PATH` or in a documented user directory.
/// * [`PpssppInstallationType::Explicit`] - an exact executable path the
///   host integration already confirmed through its own provenance (for
///   example a local AppImage), supplied via
///   [`PpssppProfileDiscoveryRoots::explicit_executables`].
///
/// A profile discovered at PPSSPP's own standard config location
/// ([`PpssppInstallationType::Native`]) may be launched by *either* - a
/// caller-confirmed exact path is at least as trustworthy as a `PATH` name
/// match, and this is precisely the equivalence PCSX2's binding already
/// makes for its `explicit_executables`. An
/// [`PpssppInstallationType::Explicit`] *profile* (a caller-supplied
/// configuration root) still requires an equally explicit executable.
///
/// [`PpssppInstallationType::Portable`] stays refused on purpose: a
/// `*.AppImage` merely *found* beside `$APPIMAGE` or by name is never a
/// caller-confirmed path, so it must never become a trusted binding.
/// [`PpssppInstallationType::FlatpakUser`] is refused because this slice
/// invents no sandbox invocation.
pub fn resolve_ppsspp_native_launch_binding(
    profile: &PpssppProfile,
) -> Result<PpssppNativeLaunchBinding, PpssppLaunchBlocker> {
    if !profile.eligible {
        return Err(launch_blocker(
            PpssppLaunchBlockerKind::ProfileIneligible,
            "profile is not eligible",
        ));
    }
    let acceptable: &[PpssppInstallationType] = match profile.installation_type {
        PpssppInstallationType::Native => &[
            PpssppInstallationType::Native,
            PpssppInstallationType::Explicit,
        ],
        PpssppInstallationType::Explicit => &[PpssppInstallationType::Explicit],
        other @ (PpssppInstallationType::FlatpakUser | PpssppInstallationType::Portable) => {
            return Err(launch_blocker(
                PpssppLaunchBlockerKind::UnsupportedInstallationType,
                format!(
                    "only native or caller-confirmed explicit PPSSPP installations are \
                     supported, got {other:?}"
                ),
            ));
        }
    };
    let matching: Vec<&PpssppExecutable> = profile
        .executable_candidates
        .iter()
        .filter(|candidate| acceptable.contains(&candidate.installation_type))
        .collect();
    if matching.is_empty() {
        return Err(launch_blocker(
            PpssppLaunchBlockerKind::ExecutableMissing,
            "no native or caller-confirmed PPSSPP executable was discovered for this profile",
        ));
    }
    let mut valid = Vec::new();
    let mut last_error = None;
    for candidate in matching {
        match validate_native_ppsspp_executable(&candidate.path) {
            Ok(()) => valid.push(candidate.path.clone()),
            Err(error) => last_error = Some(error),
        }
    }
    match valid.len() {
        0 => Err(last_error.expect("at least one candidate was inspected")),
        1 => Ok(PpssppNativeLaunchBinding {
            executable: valid.into_iter().next().expect("length checked above"),
        }),
        count => Err(launch_blocker(
            PpssppLaunchBlockerKind::AmbiguousExecutable,
            format!(
                "{count} viable PPSSPP executables match this profile and none is distinguished as authoritative"
            ),
        )),
    }
}

fn validate_native_ppsspp_executable(path: &Path) -> Result<(), PpssppLaunchBlocker> {
    if !path.is_absolute() {
        return Err(launch_blocker(
            PpssppLaunchBlockerKind::ExecutableUnsafe,
            format!("{} is not an absolute path", path.display()),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        launch_blocker(
            PpssppLaunchBlockerKind::ExecutableMissing,
            format!("{} does not exist", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(launch_blocker(
            PpssppLaunchBlockerKind::ExecutableUnsafe,
            format!("{} is not a regular non-symlink file", path.display()),
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(launch_blocker(
            PpssppLaunchBlockerKind::ExecutableNotExecutable,
            format!("{} is not executable", path.display()),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ProfileCandidate {
    installation_type: PpssppInstallationType,
    scope: PpssppProfileScope,
    configuration_path: PathBuf,
    provenance: &'static str,
}

/// Discovers only documented XDG/Flatpak paths and exact caller-provided
/// portable/custom paths.  No home-directory recursion and no process launch
/// occurs here.
pub fn discover_ppsspp_profiles(roots: &PpssppProfileDiscoveryRoots) -> PpssppProfileDiscovery {
    let mut candidates = vec![
        ProfileCandidate {
            installation_type: PpssppInstallationType::Native,
            scope: PpssppProfileScope::User,
            configuration_path: roots.xdg_config_home.join("ppsspp"),
            provenance: "XDG PPSSPP configuration directory",
        },
        ProfileCandidate {
            installation_type: PpssppInstallationType::Native,
            scope: PpssppProfileScope::User,
            configuration_path: roots.xdg_data_home.join("ppsspp"),
            provenance: "XDG PPSSPP data directory",
        },
        ProfileCandidate {
            installation_type: PpssppInstallationType::FlatpakUser,
            scope: PpssppProfileScope::User,
            configuration_path: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("config/ppsspp"),
            provenance: "Flatpak PPSSPP configuration directory",
        },
        ProfileCandidate {
            installation_type: PpssppInstallationType::FlatpakUser,
            scope: PpssppProfileScope::User,
            configuration_path: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("data/ppsspp"),
            provenance: "Flatpak PPSSPP data directory",
        },
    ];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|path| ProfileCandidate {
                installation_type: PpssppInstallationType::Portable,
                scope: PpssppProfileScope::Explicit,
                configuration_path: path,
                provenance: "caller-supplied PPSSPP portable/AppImage configuration directory",
            }),
    );
    if let Some(directory) = &roots.appimage_directory {
        candidates.push(ProfileCandidate {
            installation_type: PpssppInstallationType::Portable,
            scope: PpssppProfileScope::Explicit,
            configuration_path: directory.join("ppsspp"),
            provenance: "APPIMAGE-adjacent PPSSPP portable configuration directory",
        });
    }
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|path| ProfileCandidate {
                installation_type: PpssppInstallationType::Explicit,
                scope: PpssppProfileScope::Explicit,
                configuration_path: path,
                provenance: "explicit PPSSPP configuration directory",
            }),
    );
    candidates.sort_by(|left, right| left.configuration_path.cmp(&right.configuration_path));
    candidates.dedup_by(|left, right| left.configuration_path == right.configuration_path);

    let executables = discover_executables(roots);
    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for candidate in candidates {
        if profiles.len() >= PPSSPP_MAX_PROFILES {
            warnings.push(blocker(
                PpssppProfileBlockerKind::ProfileLimitReached,
                &candidate.configuration_path,
                format!("profile discovery stopped at the {PPSSPP_MAX_PROFILES}-profile limit"),
            ));
            break;
        }
        if !candidate.configuration_path.exists() && candidate.scope == PpssppProfileScope::User {
            continue;
        }
        profiles.push(validate_profile(candidate, &executables));
    }
    PpssppProfileDiscovery {
        profiles,
        warnings,
        complete: true,
    }
}

/// Parses a PPSSPP version from output already obtained by a caller.  The
/// adapter itself does not execute binaries, keeping discovery read-only.
pub fn parse_ppsspp_version(output: &str) -> Option<String> {
    let normalized = output.trim();
    let index = normalized.find("PPSSPP")?;
    let tail = normalized[index + "PPSSPP".len()..].trim_start();
    let tail = tail.strip_prefix('v').unwrap_or(tail);
    let version: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    (version.split('.').count() >= 2 && version.chars().any(|character| character.is_ascii_digit()))
        .then_some(version)
}

/// Inspects PPSSPP assets only after the caller separates an already-verified
/// PSP disc ID from a PPSSPP-observed ID.  This function emits no core identity
/// evidence and cannot upgrade either value.
pub fn inspect_ppsspp_game(
    profile: &PpssppProfile,
    request: &PpssppGameRequest,
) -> PpssppGameInspection {
    let (game_id, game_id_mapping) = select_game_id(request);
    let global_config = inspect_config(&profile.global_config_path);
    let per_game_config = game_id.as_ref().and_then(|id| {
        [
            profile.system_path.join(format!("{id}.ini")),
            profile.system_path.join(format!("ppsspp_{id}.ini")),
        ]
        .into_iter()
        .find(|path| is_regular_file(path))
        .map(|path| inspect_config(&path))
    });
    let overridden_setting_keys = per_game_config
        .as_ref()
        .map(|config| differing_keys(&global_config.settings, &config.settings))
        .unwrap_or_default();
    let cheats = game_id
        .as_ref()
        .map(|id| inspect_cheats(&profile.cheats_path.join(format!("{id}.ini"))));
    let textures = game_id
        .as_ref()
        .map(|id| inspect_textures(&profile.textures_path.join(id)));
    let savedata = game_id
        .as_ref()
        .map(|id| inspect_savedata(&profile.savedata_path, id));
    let mut health_warnings: Vec<String> = profile
        .blockers
        .iter()
        .map(|blocker| blocker.detail.clone())
        .collect();
    for warning in &global_config.warnings {
        health_warnings.push(warning.detail.clone());
    }
    let health = PpssppHealth {
        detected: profile.eligible || !profile.executable_candidates.is_empty(),
        config_readable: global_config.readable,
        memstick_readable: is_real_directory(&profile.memstick_path),
        cheats_directory_available: is_real_directory(&profile.cheats_path),
        textures_directory_available: is_real_directory(&profile.textures_path),
        game_profile_mapping: game_id_mapping,
        warnings: health_warnings,
    };
    PpssppGameInspection {
        game_id,
        game_id_mapping,
        global_config,
        per_game_config,
        overridden_setting_keys,
        cheats,
        textures,
        savedata,
        health,
    }
}

fn validate_profile(
    candidate: ProfileCandidate,
    executables: &[PpssppExecutable],
) -> PpssppProfile {
    let path = candidate.configuration_path;
    let mut blockers = Vec::new();
    let eligible = if !path.is_absolute() {
        blockers.push(blocker(
            PpssppProfileBlockerKind::PathNotAbsolute,
            &path,
            "configuration path is not absolute",
        ));
        false
    } else if path.parent().is_none() {
        blockers.push(blocker(
            PpssppProfileBlockerKind::FilesystemRoot,
            &path,
            "a filesystem root cannot be a PPSSPP profile",
        ));
        false
    } else {
        match validate_destination_root(&path) {
            Ok(validated) if validated.state() == DestinationRootState::Absent => {
                blockers.push(blocker(
                    PpssppProfileBlockerKind::MissingConfiguration,
                    &path,
                    "configuration directory does not exist",
                ));
                false
            }
            Ok(_) if !is_regular_file(&path.join("PSP/SYSTEM/ppsspp.ini")) => {
                blockers.push(blocker(
                    PpssppProfileBlockerKind::MissingPpssppEvidence,
                    &path,
                    "PSP/SYSTEM/ppsspp.ini was not found as a regular file",
                ));
                false
            }
            Ok(_) => true,
            Err(error) => {
                let kind = match error.reason {
                    DestinationSafetyFailureReason::RootNotDirectory
                    | DestinationSafetyFailureReason::NonDirectoryParent => {
                        PpssppProfileBlockerKind::NotDirectory
                    }
                    DestinationSafetyFailureReason::InspectionFailed => {
                        PpssppProfileBlockerKind::Unreadable
                    }
                    _ => PpssppProfileBlockerKind::UnsafePath,
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
    let memstick_path = path.join("PSP");
    let system_path = memstick_path.join("SYSTEM");
    PpssppProfile {
        profile_id: format!("ppsspp:{}", path.display()),
        installation_type: candidate.installation_type,
        scope: candidate.scope,
        configuration_path: path.clone(),
        provenance: candidate.provenance,
        eligible,
        blockers,
        executable_candidates: executables.to_vec(),
        memstick_path: memstick_path.clone(),
        global_config_path: system_path.join("ppsspp.ini"),
        system_path,
        cheats_path: memstick_path.join("CHEATS"),
        textures_path: memstick_path.join("TEXTURES"),
        savedata_path: memstick_path.join("SAVEDATA"),
        game_path: memstick_path.join("GAME"),
        state_path: memstick_path.join("PPSSPP_STATE"),
    }
}

fn discover_executables(roots: &PpssppProfileDiscoveryRoots) -> Vec<PpssppExecutable> {
    let mut paths = roots.explicit_executables.clone();
    for directory in [
        roots.home.join("Applications"),
        roots.home.join(".local/bin"),
        roots.home.join(".local/share/applications"),
        roots.home.join("AppImages"),
        roots.home.join("bin"),
    ] {
        paths.extend([
            directory.join("PPSSPP"),
            directory.join("PPSSPPQt"),
            directory.join("PPSSPPSDL"),
            directory.join("ppsspp"),
            directory.join("PPSSPP.AppImage"),
            directory.join("ppsspp.AppImage"),
            directory.join("PPSSPP").join("PPSSPP.AppImage"),
            directory.join("PPSSPP").join("PPSSPPSDL"),
            directory.join("PPSSPP").join("PPSSPPQt"),
        ]);
    }
    if let Some(directory) = &roots.appimage_directory {
        paths.extend([directory.join("PPSSPP.AppImage"), directory.join("ppsspp")]);
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path).take(128) {
            paths.extend([
                directory.join("ppsspp"),
                directory.join("PPSSPPQt"),
                directory.join("ppsspp-qt"),
            ]);
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| is_regular_file(path))
        .map(|path| PpssppExecutable {
            installation_type: if roots.explicit_executables.contains(&path) {
                PpssppInstallationType::Explicit
            } else if roots
                .appimage_directory
                .as_ref()
                .is_some_and(|directory| path.starts_with(directory))
                || path
                    .extension()
                    .is_some_and(|extension| extension == "AppImage")
            {
                PpssppInstallationType::Portable
            } else {
                PpssppInstallationType::Native
            },
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|output| parse_ppsspp_version(output)),
            path,
        })
        .collect()
}

fn select_game_id(request: &PpssppGameRequest) -> (Option<String>, PpssppGameIdMapping) {
    if let Some(id) = request
        .verified_psp_disc_id
        .as_deref()
        .and_then(normalize_game_id)
    {
        return (Some(id), PpssppGameIdMapping::VerifiedPspDiscId);
    }
    if let Some(id) = request
        .emulator_game_id
        .as_deref()
        .and_then(normalize_game_id)
    {
        return (Some(id), PpssppGameIdMapping::EmulatorMetadataOnly);
    }
    (None, PpssppGameIdMapping::Unavailable)
}

fn normalize_game_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()
        && trimmed.len() <= 64
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then(|| trimmed.to_ascii_uppercase())
}

fn inspect_config(path: &Path) -> PpssppGlobalConfig {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, PPSSPP_MAX_CONFIG_BYTES, &mut warnings) else {
        return PpssppGlobalConfig {
            path: path.to_path_buf(),
            exists,
            readable: false,
            settings: PpssppSettings::default(),
            warnings,
        };
    };
    let settings = parse_settings(&text, path, &mut warnings);
    PpssppGlobalConfig {
        path: path.to_path_buf(),
        exists,
        readable: true,
        settings,
        warnings,
    }
}

fn parse_settings(
    text: &str,
    path: &Path,
    warnings: &mut Vec<PpssppInspectionWarning>,
) -> PpssppSettings {
    let mut settings = PpssppSettings::default();
    let mut section = String::new();
    for (index, raw) in text.lines().enumerate() {
        if index >= MAX_INI_LINES {
            warn(
                warnings,
                PpssppInspectionWarningKind::LineCountLimitReached,
                path,
                format!("INI parsing stopped at the {MAX_INI_LINES}-line limit"),
            );
            break;
        }
        if raw.len() > MAX_INI_LINE_BYTES {
            warn(
                warnings,
                PpssppInspectionWarningKind::LineTooLong,
                path,
                format!("INI contains a line over {MAX_INI_LINE_BYTES} bytes"),
            );
            continue;
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if let Some(value) = line.strip_suffix(']') {
                section = value[1..].trim().to_ascii_lowercase();
            } else {
                warn(
                    warnings,
                    PpssppInspectionWarningKind::MalformedIni,
                    path,
                    "INI section does not end with ']'",
                );
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warn(
                warnings,
                PpssppInspectionWarningKind::MalformedIni,
                path,
                "INI setting has no '=' separator",
            );
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            warn(
                warnings,
                PpssppInspectionWarningKind::MalformedIni,
                path,
                "INI setting has an empty key",
            );
            continue;
        }
        apply_setting(&mut settings, &section, key, value);
    }
    settings
}

fn apply_setting(settings: &mut PpssppSettings, section: &str, key: &str, value: &str) {
    let normalized = key.to_ascii_lowercase();
    let boolean = parse_bool(value);
    match normalized.as_str() {
        "graphicsbackend" | "backend" => settings.backend = Some(value.to_string()),
        "internalresolution" | "internalres" => {
            settings.internal_resolution = Some(value.to_string())
        }
        "frameskip" => settings.frame_skip = Some(value.to_string()),
        "speedlimit" => settings.speed_limit = Some(value.to_string()),
        "alternativespeed" => settings.alternative_speed = Some(value.to_string()),
        "anisotropiclevel" | "anisotropicfiltering" => {
            settings.anisotropic_filtering = Some(value.to_string())
        }
        "texturefiltering" => settings.texture_filtering = Some(value.to_string()),
        "texturescalinglevel" | "texturescaling" => {
            settings.texture_scaling = Some(value.to_string())
        }
        "audioenable" => settings.audio_enabled = boolean,
        "audiolatency" => settings.audio_latency = Some(value.to_string()),
        "systemlanguage" | "language" => settings.language = Some(value.to_string()),
        "enablecheats" | "cheatsenabled" => settings.cheats_enabled = boolean,
        "replacetextures" | "texturereplacement" => settings.texture_replacements_enabled = boolean,
        "controlsmapping" | "inputconfig" => {
            settings.controller_config_present = boolean.or(Some(true))
        }
        _ if settings.unknown.len() < MAX_RETAINED_UNKNOWN_SETTINGS => {
            settings
                .unknown
                .insert(format!("{section}.{key}"), value.to_string());
        }
        _ => {}
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn differing_keys(global: &PpssppSettings, game: &PpssppSettings) -> Vec<String> {
    let global = flattened_settings(global);
    let game = flattened_settings(game);
    game.into_iter()
        .filter_map(|(key, value)| (global.get(&key) != Some(&value)).then_some(key))
        .collect()
}

fn flattened_settings(settings: &PpssppSettings) -> BTreeMap<String, String> {
    let mut values = settings.unknown.clone();
    for (key, value) in [
        ("backend", settings.backend.clone()),
        ("internal_resolution", settings.internal_resolution.clone()),
        ("frame_skip", settings.frame_skip.clone()),
        ("speed_limit", settings.speed_limit.clone()),
        ("alternative_speed", settings.alternative_speed.clone()),
        (
            "anisotropic_filtering",
            settings.anisotropic_filtering.clone(),
        ),
        ("texture_filtering", settings.texture_filtering.clone()),
        ("texture_scaling", settings.texture_scaling.clone()),
        ("audio_latency", settings.audio_latency.clone()),
        ("language", settings.language.clone()),
    ] {
        if let Some(value) = value {
            values.insert(key.to_string(), value);
        }
    }
    for (key, value) in [
        ("audio_enabled", settings.audio_enabled),
        ("cheats_enabled", settings.cheats_enabled),
        (
            "texture_replacements_enabled",
            settings.texture_replacements_enabled,
        ),
        (
            "controller_config_present",
            settings.controller_config_present,
        ),
    ] {
        if let Some(value) = value {
            values.insert(key.to_string(), value.to_string());
        }
    }
    values
}

fn inspect_cheats(path: &Path) -> PpssppCheatInventory {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, PPSSPP_MAX_CHEAT_BYTES, &mut warnings) else {
        return PpssppCheatInventory {
            path: path.to_path_buf(),
            exists,
            entries: 0,
            enabled_entries: 0,
            warnings,
        };
    };
    let mut entries = 0;
    let mut enabled_entries = 0;
    for line in text.lines() {
        let line = line.trim_start();
        let enabled = line.starts_with("_C1");
        if enabled || line.starts_with("_C0") {
            if entries >= PPSSPP_MAX_CHEAT_ENTRIES {
                warn(
                    &mut warnings,
                    PpssppInspectionWarningKind::CheatEntryLimitReached,
                    path,
                    format!("cheat parsing stopped at the {PPSSPP_MAX_CHEAT_ENTRIES}-entry limit"),
                );
                break;
            }
            entries += 1;
            enabled_entries += usize::from(enabled);
        }
    }
    PpssppCheatInventory {
        path: path.to_path_buf(),
        exists,
        entries,
        enabled_entries,
        warnings,
    }
}

fn inspect_textures(path: &Path) -> PpssppTextureInventory {
    let mut inventory = PpssppTextureInventory {
        path: path.to_path_buf(),
        present: is_real_directory(path),
        texture_ini_present: is_regular_file(&path.join("textures.ini")),
        file_count: 0,
        total_size_bytes: 0,
        complete: true,
        warnings: Vec::new(),
    };
    if !inventory.present {
        return inventory;
    }
    let mut pending = VecDeque::from([(path.to_path_buf(), 0usize)]);
    let mut entries_visited = 0usize;
    while let Some((directory, depth)) = pending.pop_front() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                inventory.complete = false;
                warn(
                    &mut inventory.warnings,
                    PpssppInspectionWarningKind::UnreadablePath,
                    &directory,
                    format!("directory cannot be read: {error}"),
                );
                continue;
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            if entries_visited >= PPSSPP_MAX_ENTRIES_VISITED {
                inventory.complete = false;
                warn(
                    &mut inventory.warnings,
                    PpssppInspectionWarningKind::EntryLimitReached,
                    &directory,
                    format!("texture inspection stopped at {PPSSPP_MAX_ENTRIES_VISITED} entries"),
                );
                return inventory;
            }
            entries_visited += 1;
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(error) => {
                    inventory.complete = false;
                    warn(
                        &mut inventory.warnings,
                        PpssppInspectionWarningKind::UnreadablePath,
                        &directory,
                        format!("directory entry cannot be read: {error}"),
                    );
                }
            }
        }
        paths.sort();
        for entry_path in paths {
            let metadata = match fs::symlink_metadata(&entry_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    inventory.complete = false;
                    warn(
                        &mut inventory.warnings,
                        PpssppInspectionWarningKind::UnreadablePath,
                        &entry_path,
                        format!("entry cannot be inspected: {error}"),
                    );
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                warn(
                    &mut inventory.warnings,
                    PpssppInspectionWarningKind::SymlinkSkipped,
                    &entry_path,
                    "symlink was not followed",
                );
            } else if metadata.is_file() {
                if inventory.file_count >= PPSSPP_MAX_TEXTURE_FILES {
                    inventory.complete = false;
                    warn(
                        &mut inventory.warnings,
                        PpssppInspectionWarningKind::FileCountLimitReached,
                        &entry_path,
                        format!("texture inspection stopped at {PPSSPP_MAX_TEXTURE_FILES} files"),
                    );
                    return inventory;
                }
                inventory.file_count += 1;
                inventory.total_size_bytes =
                    inventory.total_size_bytes.saturating_add(metadata.len());
            } else if metadata.is_dir() {
                if depth >= PPSSPP_MAX_TEXTURE_DEPTH {
                    inventory.complete = false;
                    warn(
                        &mut inventory.warnings,
                        PpssppInspectionWarningKind::DepthLimitReached,
                        &entry_path,
                        format!("texture inspection stopped at depth {PPSSPP_MAX_TEXTURE_DEPTH}"),
                    );
                } else {
                    pending.push_back((entry_path, depth + 1));
                }
            } else {
                warn(
                    &mut inventory.warnings,
                    PpssppInspectionWarningKind::SpecialFileSkipped,
                    &entry_path,
                    "non-regular texture entry was skipped",
                );
            }
        }
    }
    inventory
}

fn inspect_savedata(path: &Path, game_id: &str) -> PpssppSaveDataInventory {
    let mut inventory = PpssppSaveDataInventory {
        path: path.to_path_buf(),
        candidate_paths: Vec::new(),
        complete: true,
        warnings: Vec::new(),
    };
    if !is_real_directory(path) {
        return inventory;
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            inventory.complete = false;
            warn(
                &mut inventory.warnings,
                PpssppInspectionWarningKind::UnreadablePath,
                path,
                format!("SAVEDATA directory cannot be read: {error}"),
            );
            return inventory;
        }
    };
    for (index, entry) in entries.enumerate() {
        if index >= PPSSPP_MAX_ENTRIES_VISITED {
            inventory.complete = false;
            warn(
                &mut inventory.warnings,
                PpssppInspectionWarningKind::EntryLimitReached,
                path,
                format!("SAVEDATA inspection stopped at {PPSSPP_MAX_ENTRIES_VISITED} entries"),
            );
            break;
        }
        let Ok(entry) = entry else { continue };
        let candidate = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&candidate) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with(game_id)
        {
            if inventory.candidate_paths.len() >= PPSSPP_MAX_SAVEDATA_CANDIDATES {
                inventory.complete = false;
                warn(
                    &mut inventory.warnings,
                    PpssppInspectionWarningKind::FileCountLimitReached,
                    path,
                    format!(
                        "SAVEDATA candidate retention stopped at {PPSSPP_MAX_SAVEDATA_CANDIDATES}"
                    ),
                );
                break;
            }
            inventory.candidate_paths.push(candidate);
        }
    }
    inventory.candidate_paths.sort();
    inventory
}

fn read_text(
    path: &Path,
    maximum_bytes: u64,
    warnings: &mut Vec<PpssppInspectionWarning>,
) -> Option<String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            warn(
                warnings,
                PpssppInspectionWarningKind::UnreadablePath,
                path,
                format!("file cannot be inspected: {error}"),
            );
            return None;
        }
    };
    if metadata.file_type().is_symlink() {
        warn(
            warnings,
            PpssppInspectionWarningKind::SymlinkSkipped,
            path,
            "symlink was not followed",
        );
        return None;
    }
    if !metadata.is_file() {
        warn(
            warnings,
            PpssppInspectionWarningKind::SpecialFileSkipped,
            path,
            "non-regular file was skipped",
        );
        return None;
    }
    if metadata.len() > maximum_bytes {
        warn(
            warnings,
            PpssppInspectionWarningKind::FileTooLarge,
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
                PpssppInspectionWarningKind::UnreadablePath,
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
            PpssppInspectionWarningKind::UnreadablePath,
            path,
            format!("file cannot be read: {error}"),
        );
        return None;
    }
    if bytes.len() as u64 > maximum_bytes {
        warn(
            warnings,
            PpssppInspectionWarningKind::FileTooLarge,
            path,
            format!("file grew beyond the {maximum_bytes}-byte limit while reading"),
        );
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) => {
            warn(
                warnings,
                PpssppInspectionWarningKind::InvalidUtf8,
                path,
                "file is not valid UTF-8; invalid bytes were replaced for INI parsing",
            );
            Some(String::from_utf8_lossy(error.as_bytes()).into_owned())
        }
    }
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

fn warn(
    warnings: &mut Vec<PpssppInspectionWarning>,
    kind: PpssppInspectionWarningKind,
    path: &Path,
    detail: impl Into<String>,
) {
    if !warnings
        .iter()
        .any(|warning| warning.kind == kind && warning.path == path)
    {
        warnings.push(PpssppInspectionWarning {
            kind,
            path: path.to_path_buf(),
            detail: detail.into(),
        });
    }
}

fn blocker(
    kind: PpssppProfileBlockerKind,
    path: &Path,
    detail: impl Into<String>,
) -> PpssppProfileBlocker {
    PpssppProfileBlocker {
        kind,
        path: EncodedPath::from_path(path),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;

    fn roots(temp: &TempDir) -> PpssppProfileDiscoveryRoots {
        let home = temp.path().join("home");
        PpssppProfileDiscoveryRoots {
            xdg_config_home: home.join(".config"),
            xdg_data_home: home.join(".local/share"),
            home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        }
    }

    fn profile_root(roots: &PpssppProfileDiscoveryRoots) -> PathBuf {
        roots.xdg_config_home.join("ppsspp")
    }

    fn write_global(root: &Path, text: &str) {
        let path = root.join("PSP/SYSTEM/ppsspp.ini");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn eligible(root: PathBuf) -> PpssppProfile {
        let discovery = discover_ppsspp_profiles(&PpssppProfileDiscoveryRoots {
            home: root.parent().unwrap().parent().unwrap().to_path_buf(),
            xdg_config_home: root.parent().unwrap().to_path_buf(),
            xdg_data_home: root.parent().unwrap().join("data"),
            explicit_configuration_roots: vec![root],
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        });
        discovery
            .profiles
            .into_iter()
            .find(|profile| profile.eligible)
            .unwrap()
    }

    #[test]
    fn native_and_flatpak_profiles_are_discovered_read_only() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_global(
            &profile_root(&roots),
            "[Graphics]\nGraphicsBackend = Vulkan\n",
        );
        let flatpak = roots
            .home
            .join(".var/app")
            .join(FLATPAK_APP_ID)
            .join("config/ppsspp");
        write_global(&flatpak, "[SystemParam]\nEnableCheats = True\n");
        let discovery = discover_ppsspp_profiles(&roots);
        assert_eq!(
            discovery
                .profiles
                .iter()
                .filter(|profile| profile.eligible)
                .count(),
            2
        );
        assert!(
            discovery
                .profiles
                .iter()
                .any(|profile| profile.installation_type == PpssppInstallationType::FlatpakUser)
        );
    }

    #[test]
    fn explicit_portable_path_and_supplied_version_text_are_preserved() {
        let temp = TempDir::new().unwrap();
        let mut roots = roots(&temp);
        let portable = temp.path().join("portable");
        write_global(&portable, "");
        let executable = temp.path().join("PPSSPP.AppImage");
        fs::write(&executable, b"not executed").unwrap();
        roots.portable_configuration_roots.push(portable);
        roots.explicit_executables.push(executable.clone());
        roots
            .known_version_outputs
            .insert(executable.clone(), "PPSSPP v1.18.0-12-gabc".to_string());
        let discovery = discover_ppsspp_profiles(&roots);
        let profile = discovery
            .profiles
            .iter()
            .find(|profile| profile.installation_type == PpssppInstallationType::Portable)
            .unwrap();
        assert!(profile.eligible);
        assert_eq!(
            profile.executable_candidates,
            vec![PpssppExecutable {
                path: executable,
                installation_type: PpssppInstallationType::Explicit,
                version: Some("1.18.0".to_string()),
            }]
        );
    }

    #[test]
    fn global_and_per_game_configuration_are_parsed() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(
            &root,
            "[Graphics]\nGraphicsBackend = Vulkan\nInternalResolution = 2\nReplaceTextures = True\n[Audio]\nAudioEnable = False\n",
        );
        fs::write(
            root.join("PSP/SYSTEM/ULUS10000.ini"),
            "[Graphics]\nInternalResolution = 4\n",
        )
        .unwrap();
        let profile = eligible(root);
        let inspection = inspect_ppsspp_game(
            &profile,
            &PpssppGameRequest {
                verified_psp_disc_id: Some("ULUS10000".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            inspection.global_config.settings.backend.as_deref(),
            Some("Vulkan")
        );
        assert_eq!(inspection.global_config.settings.audio_enabled, Some(false));
        assert_eq!(
            inspection
                .per_game_config
                .as_ref()
                .unwrap()
                .settings
                .internal_resolution
                .as_deref(),
            Some("4")
        );
        assert!(
            inspection
                .overridden_setting_keys
                .contains(&"internal_resolution".to_string())
        );
    }

    #[test]
    fn malformed_config_fails_soft_and_unknown_version_is_valid() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(&root, "[Graphics\nnot-a-setting\n");
        let inspection = inspect_ppsspp_game(&eligible(root), &PpssppGameRequest::default());
        assert!(
            inspection
                .global_config
                .warnings
                .iter()
                .any(|warning| warning.kind == PpssppInspectionWarningKind::MalformedIni)
        );
        assert_eq!(parse_ppsspp_version("not a version"), None);
        assert_eq!(
            parse_ppsspp_version("PPSSPP v1.17.1-42"),
            Some("1.17.1".to_string())
        );
    }

    #[test]
    fn verified_id_maps_assets_but_emulator_id_has_no_identity_authority() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(&root, "");
        let profile = eligible(root);
        let verified = inspect_ppsspp_game(
            &profile,
            &PpssppGameRequest {
                verified_psp_disc_id: Some("ulus10000".to_string()),
                emulator_game_id: Some("ULES99999".to_string()),
            },
        );
        assert_eq!(verified.game_id.as_deref(), Some("ULUS10000"));
        assert_eq!(
            verified.game_id_mapping,
            PpssppGameIdMapping::VerifiedPspDiscId
        );
        let metadata_only = inspect_ppsspp_game(
            &profile,
            &PpssppGameRequest {
                emulator_game_id: Some("ULES99999".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            metadata_only.game_id_mapping,
            PpssppGameIdMapping::EmulatorMetadataOnly
        );
        assert_ne!(
            metadata_only.game_id_mapping,
            PpssppGameIdMapping::VerifiedPspDiscId
        );
        // `PpssppGameInspection` deliberately has no platform or identity
        // field.  A PPSSPP namespace ID is context only, even if it names a
        // plausible PSP title.
        assert!(metadata_only.global_config.settings.unknown.is_empty());
    }

    #[test]
    fn cheat_and_texture_names_have_zero_identity_authority() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(&root, "[SystemParam]\nEnableCheats = True\n");
        fs::create_dir_all(root.join("PSP/CHEATS")).unwrap();
        fs::write(
            root.join("PSP/CHEATS/ULUS10000.ini"),
            "_C0 Off\n_L 0x0\n_C1 On\n_L 0x1\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("PSP/TEXTURES/ULUS10000/nested")).unwrap();
        fs::write(
            root.join("PSP/TEXTURES/ULUS10000/textures.ini"),
            "# texture config",
        )
        .unwrap();
        fs::write(root.join("PSP/TEXTURES/ULUS10000/nested/a.png"), b"texture").unwrap();
        let profile = eligible(root);
        let unresolved = inspect_ppsspp_game(&profile, &PpssppGameRequest::default());
        assert_eq!(unresolved.game_id_mapping, PpssppGameIdMapping::Unavailable);
        assert!(unresolved.cheats.is_none());
        assert!(unresolved.textures.is_none());
        let inspection = inspect_ppsspp_game(
            &profile,
            &PpssppGameRequest {
                verified_psp_disc_id: Some("ULUS10000".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(inspection.cheats.as_ref().unwrap().entries, 2);
        assert_eq!(inspection.cheats.as_ref().unwrap().enabled_entries, 1);
        assert!(inspection.textures.as_ref().unwrap().texture_ini_present);
        assert_eq!(inspection.textures.as_ref().unwrap().file_count, 2);
    }

    #[test]
    fn save_candidates_are_conservative_and_texture_walk_is_bounded() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(&root, "");
        fs::create_dir_all(root.join("PSP/SAVEDATA/ULUS10000DATA")).unwrap();
        fs::create_dir_all(root.join("PSP/SAVEDATA/OTHER")).unwrap();
        let deep = root.join("PSP/TEXTURES/ULUS10000");
        let mut current = deep.clone();
        for _ in 0..PPSSPP_MAX_TEXTURE_DEPTH + 2 {
            current = current.join("nested");
            fs::create_dir_all(&current).unwrap();
        }
        let inspection = inspect_ppsspp_game(
            &eligible(root),
            &PpssppGameRequest {
                verified_psp_disc_id: Some("ULUS10000".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            inspection.savedata.as_ref().unwrap().candidate_paths.len(),
            1
        );
        assert!(!inspection.textures.as_ref().unwrap().complete);
    }

    #[test]
    fn oversized_cheat_input_is_rejected_without_mutation() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(&root, "");
        fs::create_dir_all(root.join("PSP/CHEATS")).unwrap();
        fs::write(
            root.join("PSP/CHEATS/ULUS10000.ini"),
            vec![b'x'; PPSSPP_MAX_CHEAT_BYTES as usize + 1],
        )
        .unwrap();
        let inspection = inspect_ppsspp_game(
            &eligible(root),
            &PpssppGameRequest {
                verified_psp_disc_id: Some("ULUS10000".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(inspection.cheats.as_ref().unwrap().entries, 0);
        assert!(
            inspection
                .cheats
                .as_ref()
                .unwrap()
                .warnings
                .iter()
                .any(|warning| warning.kind == PpssppInspectionWarningKind::FileTooLarge)
        );
    }

    #[test]
    fn absent_optional_assets_degrade_health_without_requiring_an_executable() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(&root, "");
        let inspection = inspect_ppsspp_game(&eligible(root), &PpssppGameRequest::default());
        assert!(inspection.health.detected);
        assert!(inspection.health.config_readable);
        assert!(!inspection.health.cheats_directory_available);
        assert!(!inspection.health.textures_directory_available);
        assert!(inspection.cheats.is_none());
        assert!(inspection.textures.is_none());
    }

    #[test]
    fn native_launch_binding_requires_one_safe_executable() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(&root, "");
        let executable = temp.path().join("ppsspp");
        fs::write(&executable, b"native executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut profile = eligible(root);
        profile.executable_candidates = vec![PpssppExecutable {
            path: executable.clone(),
            installation_type: PpssppInstallationType::Native,
            version: None,
        }];
        let binding = resolve_ppsspp_native_launch_binding(&profile).unwrap();
        assert_eq!(binding.executable, executable);
    }

    #[test]
    fn native_launch_binding_refuses_ambiguous_or_non_native_profiles() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let root = profile_root(&roots);
        write_global(&root, "");
        let mut profile = eligible(root);
        profile.installation_type = PpssppInstallationType::FlatpakUser;
        let blocker = resolve_ppsspp_native_launch_binding(&profile).unwrap_err();
        assert_eq!(
            blocker.kind,
            PpssppLaunchBlockerKind::UnsupportedInstallationType
        );
    }

    // --- caller-confirmed explicit (local AppImage) launch binding ---------
    //
    // The trust anchor is `explicit_executables` + `explicit_configuration_roots`:
    // both are exact paths a host integration has already confirmed through
    // its own provenance (e.g. an EmuWiz-managed AppImage with an
    // `install.json`, or a user-picked `~/Applications/.../PPSSPP.AppImage`).
    // The launch binding then holds them to the *same* file-safety checks a
    // `Native` binary gets - it never trusts a path merely because it ends
    // in `.AppImage`.

    fn explicit_setup(temp: &TempDir) -> (PpssppProfileDiscoveryRoots, PathBuf) {
        // A configuration root that is deliberately *not* the XDG default, so
        // it survives `discover_ppsspp_profiles`' dedup as an `Explicit`
        // profile rather than collapsing into the standard `Native` one.
        let config_root = temp.path().join("apps/PPSSPP/config");
        write_global(&config_root, "[General]\n");
        let mut roots = roots(temp);
        roots.explicit_configuration_roots.push(config_root.clone());
        (roots, config_root)
    }

    fn discovered_explicit_profile(
        roots: &PpssppProfileDiscoveryRoots,
        config_root: &Path,
    ) -> PpssppProfile {
        discover_ppsspp_profiles(roots)
            .profiles
            .into_iter()
            .find(|profile| profile.configuration_path == config_root)
            .expect("the explicit configuration root must be discovered as a profile")
    }

    fn write_fake_appimage(temp: &TempDir, name: &str) -> PathBuf {
        let path = temp.path().join(name);
        fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn explicit_local_appimage_becomes_a_trusted_binding() {
        let temp = TempDir::new().unwrap();
        let (mut roots, config_root) = explicit_setup(&temp);
        let appimage = write_fake_appimage(&temp, "PPSSPP.AppImage");
        roots.explicit_executables.push(appimage.clone());

        let profile = discovered_explicit_profile(&roots, &config_root);
        assert_eq!(profile.installation_type, PpssppInstallationType::Explicit);
        assert!(profile.eligible);
        assert!(profile.executable_candidates.iter().any(|candidate| {
            candidate.path == appimage
                && candidate.installation_type == PpssppInstallationType::Explicit
        }));

        let binding = resolve_ppsspp_native_launch_binding(&profile).unwrap();
        assert_eq!(binding.executable, appimage);
    }

    #[test]
    fn explicit_appimage_missing_file_is_refused() {
        let temp = TempDir::new().unwrap();
        let (mut roots, config_root) = explicit_setup(&temp);
        roots
            .explicit_executables
            .push(temp.path().join("gone/PPSSPP.AppImage"));

        let profile = discovered_explicit_profile(&roots, &config_root);
        let blocker = resolve_ppsspp_native_launch_binding(&profile).unwrap_err();
        // A non-existent path never even becomes an executable candidate, so
        // the profile has no `Explicit` executable to bind.
        assert_eq!(blocker.kind, PpssppLaunchBlockerKind::ExecutableMissing);
    }

    #[test]
    fn explicit_appimage_directory_instead_of_file_is_refused() {
        let temp = TempDir::new().unwrap();
        let (mut roots, config_root) = explicit_setup(&temp);
        let dir = temp.path().join("PPSSPP.AppImage");
        fs::create_dir_all(&dir).unwrap();
        roots.explicit_executables.push(dir);

        let profile = discovered_explicit_profile(&roots, &config_root);
        let blocker = resolve_ppsspp_native_launch_binding(&profile).unwrap_err();
        assert_eq!(blocker.kind, PpssppLaunchBlockerKind::ExecutableMissing);
    }

    #[test]
    fn explicit_appimage_without_execute_bit_is_refused() {
        let temp = TempDir::new().unwrap();
        let (mut roots, config_root) = explicit_setup(&temp);
        let appimage = temp.path().join("PPSSPP.AppImage");
        fs::write(&appimage, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&appimage, fs::Permissions::from_mode(0o644)).unwrap();
        }
        roots.explicit_executables.push(appimage);

        let profile = discovered_explicit_profile(&roots, &config_root);
        let blocker = resolve_ppsspp_native_launch_binding(&profile).unwrap_err();
        assert_eq!(
            blocker.kind,
            PpssppLaunchBlockerKind::ExecutableNotExecutable
        );
    }

    #[test]
    fn explicit_appimage_symlink_is_refused_by_discovery_and_by_the_binding() {
        let temp = TempDir::new().unwrap();
        let (mut roots, config_root) = explicit_setup(&temp);
        let real = write_fake_appimage(&temp, "real-ppsspp");
        let link = temp.path().join("PPSSPP.AppImage");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        roots.explicit_executables.push(link.clone());

        // Discovery's `is_regular_file` (no-follow) already drops the symlink,
        // so it never becomes a candidate.
        let profile = discovered_explicit_profile(&roots, &config_root);
        assert_eq!(
            resolve_ppsspp_native_launch_binding(&profile)
                .unwrap_err()
                .kind,
            PpssppLaunchBlockerKind::ExecutableMissing
        );

        // Defence in depth: even a hand-forced `Explicit` candidate pointing
        // at that symlink is rejected by the binding's own file-safety check.
        let mut forced = profile.clone();
        forced.executable_candidates = vec![PpssppExecutable {
            path: link,
            installation_type: PpssppInstallationType::Explicit,
            version: None,
        }];
        assert_eq!(
            resolve_ppsspp_native_launch_binding(&forced)
                .unwrap_err()
                .kind,
            PpssppLaunchBlockerKind::ExecutableUnsafe
        );
    }

    #[test]
    fn a_guessed_portable_appimage_is_still_refused() {
        // A `*.AppImage` fed only through `portable_configuration_roots`
        // (never `explicit_executables`) is a guess, not a confirmed path.
        let temp = TempDir::new().unwrap();
        let mut roots = roots(&temp);
        let portable = temp.path().join("portable");
        write_global(&portable, "");
        roots.portable_configuration_roots.push(portable.clone());

        let profile = discover_ppsspp_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| profile.installation_type == PpssppInstallationType::Portable)
            .expect("portable profile must be discovered");
        let blocker = resolve_ppsspp_native_launch_binding(&profile).unwrap_err();
        assert_eq!(
            blocker.kind,
            PpssppLaunchBlockerKind::UnsupportedInstallationType
        );
    }

    #[test]
    fn a_native_profile_binds_a_caller_confirmed_appimage_and_untrusted_profiles_do_not() {
        // A PPSSPP install discovered at its own standard config location
        // (`Native`, eligible) *is* launchable by a caller-confirmed exact
        // executable path - this is the common managed-AppImage case, and
        // the equivalence PCSX2's binding already makes.
        let temp = TempDir::new().unwrap();
        let mut roots = roots(&temp);
        let native_root = profile_root(&roots);
        write_global(&native_root, "");
        let appimage = write_fake_appimage(&temp, "PPSSPP.AppImage");
        roots.explicit_executables.push(appimage.clone());

        let discovery = discover_ppsspp_profiles(&roots);
        let native_profile = discovery
            .profiles
            .iter()
            .find(|profile| {
                profile.configuration_path == native_root
                    && profile.installation_type == PpssppInstallationType::Native
            })
            .expect("the standard XDG profile is discovered as Native")
            .clone();
        assert_eq!(
            resolve_ppsspp_native_launch_binding(&native_profile)
                .unwrap()
                .executable,
            appimage,
        );

        // But the confirmed executable never leaks into an untrusted
        // installation form: a `Portable` (guessed) or `FlatpakUser` profile
        // still refuses it outright.
        for form in [
            PpssppInstallationType::Portable,
            PpssppInstallationType::FlatpakUser,
        ] {
            let mut untrusted = native_profile.clone();
            untrusted.installation_type = form;
            assert_eq!(
                resolve_ppsspp_native_launch_binding(&untrusted)
                    .unwrap_err()
                    .kind,
                PpssppLaunchBlockerKind::UnsupportedInstallationType,
            );
        }
    }

    #[test]
    fn two_confirmed_explicit_executables_are_ambiguous() {
        let temp = TempDir::new().unwrap();
        let (mut roots, config_root) = explicit_setup(&temp);
        roots
            .explicit_executables
            .push(write_fake_appimage(&temp, "PPSSPP.AppImage"));
        roots
            .explicit_executables
            .push(write_fake_appimage(&temp, "PPSSPP-dev.AppImage"));

        let profile = discovered_explicit_profile(&roots, &config_root);
        let blocker = resolve_ppsspp_native_launch_binding(&profile).unwrap_err();
        assert_eq!(blocker.kind, PpssppLaunchBlockerKind::AmbiguousExecutable);
    }
}
