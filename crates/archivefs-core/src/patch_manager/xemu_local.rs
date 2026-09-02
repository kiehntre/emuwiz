//! Bounded, read-only local xemu discovery and inspection.
//!
//! xemu's `xemu.toml` is emulator configuration, not preservation identity.
//! This module never launches xemu, follows symlinks, opens an HDD/DVD image,
//! writes configuration, or derives a game identity from a filename or title.

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

use super::destination_safety::{
    DestinationRootState, DestinationSafetyFailureReason, validate_destination_root,
};

pub const XEMU_MAX_PROFILES: usize = 16;
pub const XEMU_MAX_CONFIG_BYTES: u64 = 256 * 1024;
const FLATPAK_APP_ID: &str = "app.xemu.xemu";
const MAX_UNKNOWN_SETTINGS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XemuInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XemuProfileScope {
    User,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XemuProfileBlockerKind {
    PathNotAbsolute,
    FilesystemRoot,
    MissingConfiguration,
    UnsafePath,
    NotDirectory,
    Unreadable,
    MissingXemuEvidence,
    ProfileLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XemuProfileBlocker {
    pub kind: XemuProfileBlockerKind,
    pub path: EncodedPath,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XemuWarningKind {
    UnreadablePath,
    SymlinkSkipped,
    SpecialFileSkipped,
    FileTooLarge,
    InvalidUtf8,
    MalformedToml,
    InvalidConfiguredPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuWarning {
    pub kind: XemuWarningKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuExecutable {
    pub path: PathBuf,
    pub installation_type: XemuInstallationType,
    /// Parsed only from version text supplied by an already-authorized outer
    /// probe. This module does not execute user binaries.
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuProfile {
    pub profile_id: String,
    pub installation_type: XemuInstallationType,
    pub scope: XemuProfileScope,
    pub configuration_path: PathBuf,
    pub config_path: PathBuf,
    pub provenance: &'static str,
    pub eligible: bool,
    pub blockers: Vec<XemuProfileBlocker>,
    pub executable_candidates: Vec<XemuExecutable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuProfileDiscovery {
    pub profiles: Vec<XemuProfile>,
    pub warnings: Vec<XemuProfileBlocker>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
}

impl XemuProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, XemuDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(XemuDiscoveryError::HomeUnavailable)?;
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
pub enum XemuDiscoveryError {
    HomeUnavailable,
}

impl std::fmt::Display for XemuDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable => formatter.write_str("HOME is not set"),
        }
    }
}

impl std::error::Error for XemuDiscoveryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XemuSystemFileKind {
    McpxBootRom,
    FlashBios,
    Eeprom,
    HddImage,
    DvdImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XemuSystemFileState {
    Present,
    Missing,
    Unreadable,
    NotConfigured,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuSystemFile {
    pub kind: XemuSystemFileKind,
    pub configured_path: Option<PathBuf>,
    pub state: XemuSystemFileState,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XemuConfig {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub renderer: Option<String>,
    pub fullscreen: Option<bool>,
    pub audio_enabled: Option<bool>,
    pub networking_mode: Option<String>,
    pub controller_config_present: Option<bool>,
    pub screenshot_path: Option<PathBuf>,
    pub snapshot_settings_present: bool,
    pub system_files: Vec<XemuSystemFile>,
    pub unknown: BTreeMap<String, String>,
    pub warnings: Vec<XemuWarning>,
}

/// The caller must keep verified preservation identity separate from metadata
/// observed in xemu/XBE context. Neither field can manufacture verification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XemuGameRequest {
    pub verified_xbox_title_id: Option<String>,
    pub emulator_title_id: Option<String>,
    pub emulator_title_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XemuGameIdMapping {
    VerifiedXboxTitleId,
    EmulatorMetadataOnly,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuHealth {
    pub detected: bool,
    pub config_readable: bool,
    pub mcpx: XemuSystemFileState,
    pub flash_bios: XemuSystemFileState,
    pub eeprom: XemuSystemFileState,
    pub hdd: XemuSystemFileState,
    pub game_profile_mapping: XemuGameIdMapping,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuGameInspection {
    pub game_id: Option<String>,
    pub game_id_mapping: XemuGameIdMapping,
    /// Display-only XBE/xemu title metadata supplied by a caller that has
    /// already used the shared bounded XBE parser. It has zero identity power.
    pub emulator_title_name: Option<String>,
    pub config: XemuConfig,
    /// xemu has one global TOML configuration; no stable per-game config is
    /// manufactured by this adapter.
    pub per_game_configuration_supported: bool,
    /// HDD images are intentionally not mounted or parsed in this batch.
    pub save_data_inspected: bool,
    pub health: XemuHealth,
}

#[derive(Debug, Clone)]
struct Candidate {
    installation_type: XemuInstallationType,
    scope: XemuProfileScope,
    path: PathBuf,
    provenance: &'static str,
}

pub fn discover_xemu_profiles(roots: &XemuProfileDiscoveryRoots) -> XemuProfileDiscovery {
    let mut candidates = vec![
        Candidate {
            installation_type: XemuInstallationType::Native,
            scope: XemuProfileScope::User,
            path: roots.xdg_data_home.join("xemu/xemu"),
            provenance: "xemu SDL preference-data directory",
        },
        Candidate {
            installation_type: XemuInstallationType::Native,
            scope: XemuProfileScope::User,
            path: roots.xdg_config_home.join("xemu/xemu"),
            provenance: "XDG xemu configuration directory",
        },
        Candidate {
            installation_type: XemuInstallationType::FlatpakUser,
            scope: XemuProfileScope::User,
            path: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("data/xemu/xemu"),
            provenance: "Flatpak xemu SDL preference-data directory",
        },
    ];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|path| Candidate {
                installation_type: XemuInstallationType::Portable,
                scope: XemuProfileScope::Explicit,
                path,
                provenance: "caller-supplied xemu portable/AppImage directory",
            }),
    );
    if let Some(path) = &roots.appimage_directory {
        candidates.push(Candidate {
            installation_type: XemuInstallationType::Portable,
            scope: XemuProfileScope::Explicit,
            path: path.clone(),
            provenance: "APPIMAGE-adjacent xemu portable directory",
        });
    }
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|path| Candidate {
                installation_type: XemuInstallationType::Explicit,
                scope: XemuProfileScope::Explicit,
                path,
                provenance: "explicit xemu configuration directory",
            }),
    );
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by(|left, right| left.path == right.path);
    let executables = discover_executables(roots);
    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for candidate in candidates {
        if profiles.len() >= XEMU_MAX_PROFILES {
            warnings.push(blocker(
                XemuProfileBlockerKind::ProfileLimitReached,
                &candidate.path,
                format!("profile discovery stopped at the {XEMU_MAX_PROFILES}-profile limit"),
            ));
            break;
        }
        if !candidate.path.exists() && candidate.scope == XemuProfileScope::User {
            continue;
        }
        profiles.push(validate_profile(candidate, &executables));
    }
    XemuProfileDiscovery {
        profiles,
        warnings,
        complete: true,
    }
}

// ---------------------------------------------------------------------------
// Launch binding
// ---------------------------------------------------------------------------
//
// Proves, freshly and read-only, exactly which native `xemu` executable
// belongs to a discovered profile - the standalone-launch prerequisite
// `crate::launch::xemu_command` needs. This never launches xemu, never
// writes configuration, and never creates a directory. Mirrors
// `resolve_ppsspp_native_launch_binding`'s exact shape: only `Native`
// installations are supported (Flatpak's sandboxing and Portable/Explicit's
// unconfirmed user-data location are not yet provably safe to launch
// against, exactly as PPSSPP/DuckStation already decided for themselves).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuNativeLaunchBinding {
    pub executable: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XemuLaunchBlockerKind {
    /// Any installation type other than [`XemuInstallationType::Native`].
    UnsupportedInstallationType,
    /// The profile itself reports `eligible: false`.
    ProfileIneligible,
    /// More than one viable executable candidate matches the profile and no
    /// authority distinguishes them.
    AmbiguousExecutable,
    /// No candidate executable exists on disk.
    ExecutableMissing,
    /// A candidate executable exists but is a symlink, not a regular file,
    /// or is not an absolute path.
    ExecutableUnsafe,
    /// A candidate executable exists as a regular file but lacks the
    /// executable permission bit.
    ExecutableNotExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuLaunchBlocker {
    pub kind: XemuLaunchBlockerKind,
    pub detail: String,
}

fn launch_blocker(kind: XemuLaunchBlockerKind, detail: impl Into<String>) -> XemuLaunchBlocker {
    XemuLaunchBlocker {
        kind,
        detail: detail.into(),
    }
}

/// Freshly revalidates `profile` and either proves a launch binding or
/// returns a structured blocker. Pure and read-only: inspects only
/// filesystem metadata, never spawns a process, writes xemu configuration,
/// or creates a directory. Safe - and intended - to call again at future
/// launch time.
pub fn resolve_xemu_native_launch_binding(
    profile: &XemuProfile,
) -> Result<XemuNativeLaunchBinding, XemuLaunchBlocker> {
    if !profile.eligible {
        return Err(launch_blocker(
            XemuLaunchBlockerKind::ProfileIneligible,
            "profile is not eligible",
        ));
    }
    // A profile discovered at xemu's own standard XDG location
    // (`XemuInstallationType::Native`) may be launched by *either* a
    // PATH/name-matched `xemu` binary *or* an exact executable path the host
    // integration already confirmed through its own provenance (an
    // EmuWiz-managed AppImage supplied via
    // `XemuProfileDiscoveryRoots::explicit_executables`, classified
    // `XemuInstallationType::Explicit`). Both are held to the identical
    // `validate_native_xemu_executable` checks and the identical "exactly
    // one candidate" rule below - the same equivalence PPSSPP/PCSX2 already
    // make for their `explicit_executables`. `Portable` (a `*.AppImage`
    // merely found by name or beside `$APPIMAGE`), `FlatpakUser`, and a
    // caller-supplied `Explicit` *configuration root* stay refused: no
    // reviewed config-dir/argv contract exists for them here. The
    // executable fact proven here is independent of xemu's MCPX/BIOS/EEPROM/
    // HDD readiness, which `crate::launch::xemu_command` validates at
    // preflight.
    let acceptable: &[XemuInstallationType] = match profile.installation_type {
        XemuInstallationType::Native => {
            &[XemuInstallationType::Native, XemuInstallationType::Explicit]
        }
        other => {
            return Err(launch_blocker(
                XemuLaunchBlockerKind::UnsupportedInstallationType,
                format!(
                    "only native xemu installations (optionally launched by a caller-confirmed \
                     executable) are supported, got {other:?}"
                ),
            ));
        }
    };
    let matching: Vec<&XemuExecutable> = profile
        .executable_candidates
        .iter()
        .filter(|candidate| acceptable.contains(&candidate.installation_type))
        .collect();
    if matching.is_empty() {
        return Err(launch_blocker(
            XemuLaunchBlockerKind::ExecutableMissing,
            "no native xemu executable was discovered for this profile",
        ));
    }
    let mut valid = Vec::new();
    let mut last_error = None;
    for candidate in matching {
        match validate_native_xemu_executable(&candidate.path) {
            Ok(()) => valid.push(candidate.path.clone()),
            Err(error) => last_error = Some(error),
        }
    }
    match valid.len() {
        0 => Err(last_error.expect("at least one candidate was inspected")),
        1 => Ok(XemuNativeLaunchBinding {
            executable: valid.into_iter().next().expect("length checked above"),
        }),
        count => Err(launch_blocker(
            XemuLaunchBlockerKind::AmbiguousExecutable,
            format!(
                "{count} viable native xemu executables match this profile and none is \
                 distinguished as authoritative"
            ),
        )),
    }
}

fn validate_native_xemu_executable(path: &Path) -> Result<(), XemuLaunchBlocker> {
    if !path.is_absolute() {
        return Err(launch_blocker(
            XemuLaunchBlockerKind::ExecutableUnsafe,
            format!("{} is not an absolute path", path.display()),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        launch_blocker(
            XemuLaunchBlockerKind::ExecutableMissing,
            format!("{} does not exist", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(launch_blocker(
            XemuLaunchBlockerKind::ExecutableUnsafe,
            format!("{} is not a regular non-symlink file", path.display()),
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(launch_blocker(
            XemuLaunchBlockerKind::ExecutableNotExecutable,
            format!("{} is not executable", path.display()),
        ));
    }
    Ok(())
}

pub fn parse_xemu_version(output: &str) -> Option<String> {
    let marker = "xemu_version:";
    let tail = output
        .lines()
        .find_map(|line| line.trim().strip_prefix(marker))?
        .trim();
    let version: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (version.split('.').count() >= 2).then_some(version)
}

pub fn inspect_xemu_game(profile: &XemuProfile, request: &XemuGameRequest) -> XemuGameInspection {
    let config = inspect_config(&profile.config_path);
    let (game_id, game_id_mapping) = select_game_id(request);
    let file_state = |kind| {
        config
            .system_files
            .iter()
            .find(|file| file.kind == kind)
            .map(|file| file.state)
            .unwrap_or(XemuSystemFileState::Unknown)
    };
    let mut warning_text: Vec<String> = profile
        .blockers
        .iter()
        .map(|item| item.detail.clone())
        .collect();
    warning_text.extend(config.warnings.iter().map(|item| item.detail.clone()));
    let health = XemuHealth {
        detected: profile.eligible || !profile.executable_candidates.is_empty(),
        config_readable: config.readable,
        mcpx: file_state(XemuSystemFileKind::McpxBootRom),
        flash_bios: file_state(XemuSystemFileKind::FlashBios),
        eeprom: file_state(XemuSystemFileKind::Eeprom),
        hdd: file_state(XemuSystemFileKind::HddImage),
        game_profile_mapping: game_id_mapping,
        warnings: warning_text,
    };
    XemuGameInspection {
        game_id,
        game_id_mapping,
        emulator_title_name: request.emulator_title_name.clone(),
        config,
        per_game_configuration_supported: false,
        save_data_inspected: false,
        health,
    }
}

fn validate_profile(candidate: Candidate, executables: &[XemuExecutable]) -> XemuProfile {
    let config_path = candidate.path.join("xemu.toml");
    let mut blockers = Vec::new();
    let eligible = if !candidate.path.is_absolute() {
        blockers.push(blocker(
            XemuProfileBlockerKind::PathNotAbsolute,
            &candidate.path,
            "configuration path is not absolute",
        ));
        false
    } else if candidate.path.parent().is_none() {
        blockers.push(blocker(
            XemuProfileBlockerKind::FilesystemRoot,
            &candidate.path,
            "a filesystem root cannot be an xemu profile",
        ));
        false
    } else {
        match validate_destination_root(&candidate.path) {
            Ok(root) if root.state() == DestinationRootState::Absent => {
                blockers.push(blocker(
                    XemuProfileBlockerKind::MissingConfiguration,
                    &candidate.path,
                    "configuration directory does not exist",
                ));
                false
            }
            Ok(_) if !is_regular_file(&config_path) => {
                blockers.push(blocker(
                    XemuProfileBlockerKind::MissingXemuEvidence,
                    &candidate.path,
                    "xemu.toml was not found as a regular file",
                ));
                false
            }
            Ok(_) => true,
            Err(error) => {
                let kind = match error.reason {
                    DestinationSafetyFailureReason::RootNotDirectory
                    | DestinationSafetyFailureReason::NonDirectoryParent => {
                        XemuProfileBlockerKind::NotDirectory
                    }
                    DestinationSafetyFailureReason::InspectionFailed => {
                        XemuProfileBlockerKind::Unreadable
                    }
                    _ => XemuProfileBlockerKind::UnsafePath,
                };
                blockers.push(blocker(
                    kind,
                    &candidate.path,
                    format!("configuration path rejected: {:?}", error.reason),
                ));
                false
            }
        }
    };
    XemuProfile {
        profile_id: format!("xemu:{}", candidate.path.display()),
        installation_type: candidate.installation_type,
        scope: candidate.scope,
        configuration_path: candidate.path,
        config_path,
        provenance: candidate.provenance,
        eligible,
        blockers,
        executable_candidates: executables.to_vec(),
    }
}

fn discover_executables(roots: &XemuProfileDiscoveryRoots) -> Vec<XemuExecutable> {
    let mut paths = roots.explicit_executables.clone();
    for directory in [
        roots.home.join("Applications"),
        roots.home.join(".local/bin"),
        roots.home.join(".local/share/applications"),
        roots.home.join("AppImages"),
        roots.home.join("bin"),
    ] {
        paths.extend([
            directory.join("xemu"),
            directory.join("xemu.AppImage"),
            directory.join("Xemu.AppImage"),
            directory.join("xemu-kvm"),
        ]);
    }
    if let Some(directory) = &roots.appimage_directory {
        paths.extend([directory.join("xemu.AppImage"), directory.join("xemu")]);
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path).take(128) {
            paths.push(directory.join("xemu"));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| is_regular_file(path))
        .map(|path| XemuExecutable {
            installation_type: if roots.explicit_executables.contains(&path) {
                XemuInstallationType::Explicit
            } else if roots
                .appimage_directory
                .as_ref()
                .is_some_and(|root| path.starts_with(root))
                || path
                    .extension()
                    .is_some_and(|extension| extension == "AppImage")
            {
                XemuInstallationType::Portable
            } else {
                XemuInstallationType::Native
            },
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|text| parse_xemu_version(text)),
            path,
        })
        .collect()
}

fn inspect_config(path: &Path) -> XemuConfig {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, &mut warnings) else {
        return XemuConfig {
            path: path.to_path_buf(),
            exists,
            warnings,
            ..Default::default()
        };
    };
    let value: toml::Value = match text.parse() {
        Ok(value) => value,
        Err(error) => {
            warn(
                &mut warnings,
                XemuWarningKind::MalformedToml,
                path,
                format!("xemu.toml could not be parsed: {error}"),
            );
            return XemuConfig {
                path: path.to_path_buf(),
                exists,
                readable: true,
                warnings,
                ..Default::default()
            };
        }
    };
    let root = path.parent().unwrap_or_else(|| Path::new(""));
    let system_files = [
        (XemuSystemFileKind::McpxBootRom, "bootrom_path"),
        (XemuSystemFileKind::FlashBios, "flashrom_path"),
        (XemuSystemFileKind::Eeprom, "eeprom_path"),
        (XemuSystemFileKind::HddImage, "hdd_path"),
        // Current xemu accepts a DVD path independently. Retain it only if a
        // TOML row explicitly provides this path; never infer it from names.
        (XemuSystemFileKind::DvdImage, "dvd_path"),
    ]
    .into_iter()
    .map(|(kind, key)| inspect_system_file(&value, root, key, kind, &mut warnings))
    .collect();
    let mut unknown = BTreeMap::new();
    retain_unknown(&value, "", &mut unknown);
    XemuConfig {
        path: path.to_path_buf(),
        exists,
        readable: true,
        renderer: lookup_string(&value, &["display", "renderer"])
            .or_else(|| lookup_string(&value, &["display", "backend"])),
        fullscreen: lookup_bool(&value, &["display", "window", "fullscreen"]),
        audio_enabled: lookup_bool(&value, &["audio", "use_dsp"]),
        networking_mode: lookup_string(&value, &["net", "mode"]),
        controller_config_present: value
            .get("input")
            .and_then(toml::Value::as_table)
            .map(|table| !table.is_empty()),
        screenshot_path: lookup_string(&value, &["general", "screenshot_dir"])
            .and_then(|value| resolve_configured_path(root, &value)),
        snapshot_settings_present: value.get("snapshots").is_some(),
        system_files,
        unknown,
        warnings,
    }
}

fn inspect_system_file(
    value: &toml::Value,
    root: &Path,
    key: &str,
    kind: XemuSystemFileKind,
    warnings: &mut Vec<XemuWarning>,
) -> XemuSystemFile {
    let configured_path = value
        .get("sys")
        .and_then(|value| value.get("files"))
        .and_then(|value| value.get(key))
        .and_then(toml::Value::as_str)
        .and_then(|value| resolve_configured_path(root, value));
    let state = match &configured_path {
        None => XemuSystemFileState::NotConfigured,
        Some(path) => readable_regular_file(path, warnings),
    };
    XemuSystemFile {
        kind,
        configured_path,
        state,
    }
}

fn select_game_id(request: &XemuGameRequest) -> (Option<String>, XemuGameIdMapping) {
    if let Some(value) = request
        .verified_xbox_title_id
        .as_deref()
        .and_then(normalize_title_id)
    {
        return (Some(value), XemuGameIdMapping::VerifiedXboxTitleId);
    }
    if let Some(value) = request
        .emulator_title_id
        .as_deref()
        .and_then(normalize_title_id)
    {
        return (Some(value), XemuGameIdMapping::EmulatorMetadataOnly);
    }
    (None, XemuGameIdMapping::Unavailable)
}

fn normalize_title_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then(|| value.to_ascii_uppercase())
}

fn lookup_string(value: &toml::Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |current, part| current.get(*part))
        .and_then(toml::Value::as_str)
        .map(ToString::to_string)
}
fn lookup_bool(value: &toml::Value, path: &[&str]) -> Option<bool> {
    path.iter()
        .try_fold(value, |current, part| current.get(*part))
        .and_then(toml::Value::as_bool)
}

fn resolve_configured_path(root: &Path, raw: &str) -> Option<PathBuf> {
    let value = raw.trim();
    (!value.is_empty() && !value.contains('\0')).then(|| {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    })
}

fn retain_unknown(value: &toml::Value, prefix: &str, unknown: &mut BTreeMap<String, String>) {
    if unknown.len() >= MAX_UNKNOWN_SETTINGS {
        return;
    }
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                retain_unknown(child, &next, unknown);
            }
        }
        toml::Value::Array(_) => {
            unknown.insert(prefix.to_string(), "<array>".to_string());
        }
        other => {
            unknown.insert(prefix.to_string(), other.to_string());
        }
    }
}

fn read_text(path: &Path, warnings: &mut Vec<XemuWarning>) -> Option<String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            warn(
                warnings,
                XemuWarningKind::UnreadablePath,
                path,
                format!("config cannot be inspected: {error}"),
            );
            return None;
        }
    };
    if metadata.file_type().is_symlink() {
        warn(
            warnings,
            XemuWarningKind::SymlinkSkipped,
            path,
            "symlink was not followed",
        );
        return None;
    }
    if !metadata.is_file() {
        warn(
            warnings,
            XemuWarningKind::SpecialFileSkipped,
            path,
            "non-regular config was skipped",
        );
        return None;
    }
    if metadata.len() > XEMU_MAX_CONFIG_BYTES {
        warn(
            warnings,
            XemuWarningKind::FileTooLarge,
            path,
            format!("config exceeds {XEMU_MAX_CONFIG_BYTES} bytes"),
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
                XemuWarningKind::UnreadablePath,
                path,
                format!("config cannot be opened: {error}"),
            );
            return None;
        }
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if let Err(error) = file
        .by_ref()
        .take(XEMU_MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        warn(
            warnings,
            XemuWarningKind::UnreadablePath,
            path,
            format!("config cannot be read: {error}"),
        );
        return None;
    }
    if bytes.len() as u64 > XEMU_MAX_CONFIG_BYTES {
        warn(
            warnings,
            XemuWarningKind::FileTooLarge,
            path,
            "config grew beyond the input bound",
        );
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(value) => Some(value),
        Err(error) => {
            warn(
                warnings,
                XemuWarningKind::InvalidUtf8,
                path,
                "invalid UTF-8 was replaced for TOML parsing",
            );
            Some(String::from_utf8_lossy(error.as_bytes()).into_owned())
        }
    }
}

fn readable_regular_file(path: &Path, warnings: &mut Vec<XemuWarning>) -> XemuSystemFileState {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return XemuSystemFileState::Missing;
        }
        Err(error) => {
            warn(
                warnings,
                XemuWarningKind::UnreadablePath,
                path,
                format!("configured file cannot be inspected: {error}"),
            );
            return XemuSystemFileState::Unreadable;
        }
    };
    if metadata.file_type().is_symlink() {
        warn(
            warnings,
            XemuWarningKind::SymlinkSkipped,
            path,
            "symlink was not followed",
        );
        return XemuSystemFileState::Unreadable;
    }
    if !metadata.is_file() {
        warn(
            warnings,
            XemuWarningKind::SpecialFileSkipped,
            path,
            "configured path is not a regular file",
        );
        return XemuSystemFileState::Unreadable;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    match options.open(path) {
        Ok(_) => XemuSystemFileState::Present,
        Err(error) => {
            warn(
                warnings,
                XemuWarningKind::UnreadablePath,
                path,
                format!("configured file cannot be opened: {error}"),
            );
            XemuSystemFileState::Unreadable
        }
    }
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}
fn warn(
    warnings: &mut Vec<XemuWarning>,
    kind: XemuWarningKind,
    path: &Path,
    detail: impl Into<String>,
) {
    if !warnings
        .iter()
        .any(|warning| warning.kind == kind && warning.path == path)
    {
        warnings.push(XemuWarning {
            kind,
            path: path.to_path_buf(),
            detail: detail.into(),
        });
    }
}
fn blocker(
    kind: XemuProfileBlockerKind,
    path: &Path,
    detail: impl Into<String>,
) -> XemuProfileBlocker {
    XemuProfileBlocker {
        kind,
        path: EncodedPath::from_path(path),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn roots(temp: &TempDir) -> XemuProfileDiscoveryRoots {
        let home = temp.path().join("home");
        XemuProfileDiscoveryRoots {
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
    fn root(roots: &XemuProfileDiscoveryRoots) -> PathBuf {
        roots.xdg_data_home.join("xemu/xemu")
    }
    fn write_config(root: &Path, text: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("xemu.toml"), text).unwrap();
    }
    fn profile(roots: &XemuProfileDiscoveryRoots) -> XemuProfile {
        discover_xemu_profiles(roots)
            .profiles
            .into_iter()
            .find(|profile| profile.eligible)
            .unwrap()
    }

    #[test]
    fn native_and_flatpak_configuration_roots_are_discovered() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let flatpak = roots
            .home
            .join(".var/app")
            .join(FLATPAK_APP_ID)
            .join("data/xemu/xemu");
        write_config(&flatpak, "");
        let discovery = discover_xemu_profiles(&roots);
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
                .any(|profile| profile.installation_type == XemuInstallationType::FlatpakUser)
        );
    }

    #[test]
    fn config_and_required_system_file_health_are_read_only() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let base = root(&roots);
        fs::create_dir_all(base.join("system")).unwrap();
        for file in ["mcpx.bin", "bios.bin", "eeprom.bin", "hdd.qcow2"] {
            fs::write(base.join("system").join(file), b"x").unwrap();
        }
        write_config(
            &base,
            "[display]\nrenderer = 'vulkan'\n[display.window]\nfullscreen = true\n[audio]\nuse_dsp = true\n[general]\nscreenshot_dir = 'shots'\n[net]\nmode = 'nat'\n[input]\nbackground_input_capture = true\n[sys.files]\nbootrom_path = 'system/mcpx.bin'\nflashrom_path = 'system/bios.bin'\neeprom_path = 'system/eeprom.bin'\nhdd_path = 'system/hdd.qcow2'\ndvd_path = 'games/game.xiso'\n",
        );
        let inspection = inspect_xemu_game(&profile(&roots), &XemuGameRequest::default());
        assert_eq!(inspection.config.renderer.as_deref(), Some("vulkan"));
        assert_eq!(inspection.config.fullscreen, Some(true));
        assert_eq!(inspection.config.networking_mode.as_deref(), Some("nat"));
        assert!(
            inspection
                .config
                .system_files
                .iter()
                .filter(|file| file.kind != XemuSystemFileKind::DvdImage)
                .all(|file| file.state == XemuSystemFileState::Present)
        );
        assert_eq!(inspection.health.hdd, XemuSystemFileState::Present);
        assert!(!inspection.save_data_inspected);
    }

    #[test]
    fn malformed_and_oversized_configs_fail_soft() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        let base = root(&roots);
        write_config(&base, "[sys.files\n");
        let malformed = inspect_xemu_game(&profile(&roots), &XemuGameRequest::default());
        assert!(
            malformed
                .config
                .warnings
                .iter()
                .any(|warning| warning.kind == XemuWarningKind::MalformedToml)
        );
        fs::write(
            base.join("xemu.toml"),
            vec![b'x'; XEMU_MAX_CONFIG_BYTES as usize + 1],
        )
        .unwrap();
        let oversized = inspect_xemu_game(&profile(&roots), &XemuGameRequest::default());
        assert!(
            oversized
                .config
                .warnings
                .iter()
                .any(|warning| warning.kind == XemuWarningKind::FileTooLarge)
        );
    }

    #[test]
    fn verified_identity_wins_over_xemu_or_xbe_metadata() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let profile = profile(&roots);
        let inspection = inspect_xemu_game(
            &profile,
            &XemuGameRequest {
                verified_xbox_title_id: Some("4d5a0058".to_string()),
                emulator_title_id: Some("11223344".to_string()),
                emulator_title_name: Some("Filename Title".to_string()),
            },
        );
        assert_eq!(inspection.game_id.as_deref(), Some("4D5A0058"));
        assert_eq!(
            inspection.game_id_mapping,
            XemuGameIdMapping::VerifiedXboxTitleId
        );
        assert_eq!(
            inspection.emulator_title_name.as_deref(),
            Some("Filename Title")
        );
        let metadata_only = inspect_xemu_game(
            &profile,
            &XemuGameRequest {
                emulator_title_id: Some("11223344".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            metadata_only.game_id_mapping,
            XemuGameIdMapping::EmulatorMetadataOnly
        );
    }

    #[test]
    fn explicit_portable_executable_and_version_are_preserved_without_execution() {
        let temp = TempDir::new().unwrap();
        let mut roots = roots(&temp);
        let portable = temp.path().join("portable");
        write_config(&portable, "");
        let executable = portable.join("xemu.AppImage");
        fs::write(&executable, b"never run").unwrap();
        roots.portable_configuration_roots.push(portable);
        roots.explicit_executables.push(executable.clone());
        roots
            .known_version_outputs
            .insert(executable.clone(), "xemu_version: 0.8.136\n".to_string());
        let profile = discover_xemu_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| profile.installation_type == XemuInstallationType::Portable)
            .unwrap();
        assert_eq!(
            profile.executable_candidates[0].version.as_deref(),
            Some("0.8.136")
        );
    }

    #[test]
    fn no_per_game_or_patch_mechanism_is_invented() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let inspection = inspect_xemu_game(&profile(&roots), &XemuGameRequest::default());
        assert!(!inspection.per_game_configuration_supported);
        assert!(!inspection.save_data_inspected);
        assert_eq!(inspection.game_id_mapping, XemuGameIdMapping::Unavailable);
    }

    // -----------------------------------------------------------------
    // Launch binding
    // -----------------------------------------------------------------

    #[test]
    fn native_launch_binding_requires_one_safe_executable() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let executable = temp.path().join("xemu");
        fs::write(&executable, b"native executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut candidate = profile(&roots);
        candidate.executable_candidates = vec![XemuExecutable {
            path: executable.clone(),
            installation_type: XemuInstallationType::Native,
            version: None,
        }];
        let binding = resolve_xemu_native_launch_binding(&candidate).unwrap();
        assert_eq!(binding.executable, executable);
    }

    #[test]
    fn native_profile_binds_a_caller_confirmed_explicit_executable() {
        // The managed-AppImage seam: a path fed via
        // `roots.explicit_executables` is classified `Explicit`, and a
        // Native XDG profile accepts it under the identical safety and
        // single-candidate rules. A guessed `Portable` `*.AppImage` is
        // never accepted.
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let appimage = temp.path().join("emulators/xemu/xemu.AppImage");
        fs::create_dir_all(appimage.parent().unwrap()).unwrap();
        fs::write(&appimage, b"managed appimage").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&appimage, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut candidate = profile(&roots);
        candidate.executable_candidates = vec![XemuExecutable {
            path: appimage.clone(),
            installation_type: XemuInstallationType::Explicit,
            version: None,
        }];
        let binding = resolve_xemu_native_launch_binding(&candidate).unwrap();
        assert_eq!(binding.executable, appimage);

        // Same path, but classified `Portable` (a guessed AppImage) - refused.
        candidate.executable_candidates[0].installation_type = XemuInstallationType::Portable;
        let blocker = resolve_xemu_native_launch_binding(&candidate).unwrap_err();
        assert_eq!(blocker.kind, XemuLaunchBlockerKind::ExecutableMissing);
    }

    #[test]
    fn native_launch_binding_refuses_ineligible_and_non_native_profiles() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let mut candidate = profile(&roots);
        candidate.installation_type = XemuInstallationType::FlatpakUser;
        let blocker = resolve_xemu_native_launch_binding(&candidate).unwrap_err();
        assert_eq!(
            blocker.kind,
            XemuLaunchBlockerKind::UnsupportedInstallationType
        );

        let mut ineligible = profile(&roots);
        ineligible.eligible = false;
        let blocker = resolve_xemu_native_launch_binding(&ineligible).unwrap_err();
        assert_eq!(blocker.kind, XemuLaunchBlockerKind::ProfileIneligible);
    }

    #[test]
    fn native_launch_binding_refuses_a_missing_executable() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let blocker = resolve_xemu_native_launch_binding(&profile(&roots)).unwrap_err();
        assert_eq!(blocker.kind, XemuLaunchBlockerKind::ExecutableMissing);
    }

    #[cfg(unix)]
    #[test]
    fn native_launch_binding_refuses_a_symlinked_executable() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let real = temp.path().join("real-xemu");
        fs::write(&real, b"native executable").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        let link = temp.path().join("xemu-link");
        symlink(&real, &link).unwrap();
        let mut candidate = profile(&roots);
        candidate.executable_candidates = vec![XemuExecutable {
            path: link,
            installation_type: XemuInstallationType::Native,
            version: None,
        }];
        let blocker = resolve_xemu_native_launch_binding(&candidate).unwrap_err();
        assert_eq!(blocker.kind, XemuLaunchBlockerKind::ExecutableUnsafe);
    }

    #[test]
    fn native_launch_binding_refuses_ambiguous_executables() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let mut candidate = profile(&roots);
        for name in ["xemu-one", "xemu-two"] {
            let executable = temp.path().join(name);
            fs::write(&executable, b"native executable").unwrap();
            #[cfg(unix)]
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            candidate.executable_candidates.push(XemuExecutable {
                path: executable,
                installation_type: XemuInstallationType::Native,
                version: None,
            });
        }
        let blocker = resolve_xemu_native_launch_binding(&candidate).unwrap_err();
        assert_eq!(blocker.kind, XemuLaunchBlockerKind::AmbiguousExecutable);
    }

    #[cfg(unix)]
    #[test]
    fn native_launch_binding_refuses_a_non_executable_file() {
        let temp = TempDir::new().unwrap();
        let roots = roots(&temp);
        write_config(&root(&roots), "");
        let executable = temp.path().join("xemu");
        fs::write(&executable, b"native executable").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();
        let mut candidate = profile(&roots);
        candidate.executable_candidates = vec![XemuExecutable {
            path: executable,
            installation_type: XemuInstallationType::Native,
            version: None,
        }];
        let blocker = resolve_xemu_native_launch_binding(&candidate).unwrap_err();
        assert_eq!(blocker.kind, XemuLaunchBlockerKind::ExecutableNotExecutable);
    }
}
