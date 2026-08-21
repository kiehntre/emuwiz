//! Bounded, read-only Flycast discovery and local-state inspection.
//!
//! Flycast is a consumer of core identity.  A Dreamcast product code, Naomi
//! key, filename, GDI/CDI/CHD name, VMU name, or texture/cheat directory is
//! never emitted as preservation evidence by this module.

use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::Serialize;

use crate::platform_evidence_fusion::evidence_lineage::{ClaimType, Representation};

pub const FLYCAST_MAX_PROFILES: usize = 16;
pub const FLYCAST_MAX_CONFIG_BYTES: u64 = 256 * 1024;
pub const FLYCAST_MAX_CHEAT_BYTES: u64 = 512 * 1024;
pub const FLYCAST_MAX_DIRECTORY_ENTRIES: usize = 10_000;
pub const FLYCAST_MAX_TEXTURE_FILES: usize = 2_048;
pub const FLYCAST_MAX_TEXTURE_DEPTH: usize = 2;
pub const FLYCAST_MAX_SAVE_CANDIDATES: usize = 128;
const FLATPAK_APP_ID: &str = "org.flycast.Flycast";
const MAX_LINES: usize = 8_192;
const MAX_LINE_BYTES: usize = 8 * 1024;
const MAX_UNKNOWN_SETTINGS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlycastInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlycastPlatform {
    Dreamcast,
    Naomi,
    Naomi2,
    Atomiswave,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlycastWarningKind {
    UnreadablePath,
    SymlinkSkipped,
    SpecialFileSkipped,
    FileTooLarge,
    InvalidUtf8,
    MalformedConfig,
    EntryLimitReached,
    FileCountLimitReached,
    DepthLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastWarning {
    pub kind: FlycastWarningKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastExecutable {
    pub path: PathBuf,
    pub installation_type: FlycastInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastProfile {
    pub profile_id: String,
    pub installation_type: FlycastInstallationType,
    pub configuration_path: PathBuf,
    pub data_path: PathBuf,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<FlycastExecutable>,
    pub config_path: PathBuf,
    pub system_path: PathBuf,
    pub game_settings_path: PathBuf,
    pub cheats_path: PathBuf,
    pub textures_path: PathBuf,
    pub vmu_path: PathBuf,
    pub save_states_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastProfileDiscovery {
    pub profiles: Vec<FlycastProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
}

impl FlycastProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, FlycastDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(FlycastDiscoveryError::HomeUnavailable)?;
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
pub enum FlycastDiscoveryError {
    HomeUnavailable,
}
impl std::fmt::Display for FlycastDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set")
    }
}
impl std::error::Error for FlycastDiscoveryError {}

/// One core-supplied disc fact.  CDI is deliberately merely represented here;
/// it is never considered preservation-equivalent to a Redump/GDI dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastDiscContext {
    pub disc_number: u8,
    pub representation: Representation,
    pub claim: ClaimType,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlycastGameRequest {
    pub canonical_platform: Option<String>,
    pub flycast_platform: Option<FlycastPlatform>,
    pub verified_dreamcast_product_code: Option<String>,
    pub emulator_game_key: Option<String>,
    pub disc_contexts: Vec<FlycastDiscContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlycastGameKeyMapping {
    VerifiedProductCode,
    EmulatorMetadataOnly,
    ConflictingEmulatorMetadata,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlycastSettings {
    pub renderer: Option<String>,
    pub internal_resolution: Option<String>,
    pub widescreen: Option<bool>,
    pub vsync: Option<bool>,
    pub texture_filtering: Option<String>,
    pub texture_upscaling: Option<String>,
    pub per_pixel_sorting: Option<bool>,
    pub audio_backend: Option<String>,
    pub region: Option<String>,
    pub language: Option<String>,
    pub cable_type: Option<String>,
    pub network_mode: Option<String>,
    pub cheats_enabled: Option<bool>,
    pub texture_replacements_enabled: Option<bool>,
    pub controller_profile_present: Option<bool>,
    pub unknown: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastConfigInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub settings: FlycastSettings,
    pub warnings: Vec<FlycastWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlycastSystemFileState {
    PresentUnverified,
    Missing,
    Unreadable,
    NotConfigured,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastSystemHealth {
    pub dreamcast_bios: FlycastSystemFileState,
    pub dreamcast_flash: FlycastSystemFileState,
    pub arcade_system_roms: FlycastSystemFileState,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCheatInventory {
    pub path: PathBuf,
    pub exists: bool,
    pub entries: usize,
    pub enabled_entries: usize,
    pub warnings: Vec<FlycastWarning>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastTextureInventory {
    pub path: PathBuf,
    pub present: bool,
    pub enabled: Option<bool>,
    pub file_count: usize,
    pub total_size_bytes: u64,
    pub complete: bool,
    pub warnings: Vec<FlycastWarning>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastVmuInventory {
    pub vmu_images: Vec<PathBuf>,
    pub complete: bool,
    pub warnings: Vec<FlycastWarning>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastSaveStateInventory {
    pub candidate_paths: Vec<PathBuf>,
    pub complete: bool,
    pub warnings: Vec<FlycastWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastHealth {
    pub detected: bool,
    pub config_readable: bool,
    pub system: FlycastSystemHealth,
    pub game_key_mapping: FlycastGameKeyMapping,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastGameInspection {
    pub game_key: Option<String>,
    pub emulator_game_key_context: Option<String>,
    pub game_key_mapping: FlycastGameKeyMapping,
    pub identity_mismatch: Option<String>,
    pub canonical_platform: Option<String>,
    pub flycast_platform: Option<FlycastPlatform>,
    pub disc_contexts: Vec<FlycastDiscContext>,
    pub global_config: FlycastConfigInspection,
    pub per_game_config: Option<FlycastConfigInspection>,
    pub overridden_setting_keys: Vec<String>,
    pub cheats: Option<FlycastCheatInventory>,
    pub textures: Option<FlycastTextureInventory>,
    pub vmu: FlycastVmuInventory,
    pub save_states: Option<FlycastSaveStateInventory>,
    pub health: FlycastHealth,
}

#[derive(Clone)]
struct Candidate {
    kind: FlycastInstallationType,
    config: PathBuf,
    data: PathBuf,
}

/// Uses only known XDG/Flatpak locations and exact caller-provided roots.
pub fn discover_flycast_profiles(roots: &FlycastProfileDiscoveryRoots) -> FlycastProfileDiscovery {
    let mut candidates = vec![
        Candidate {
            kind: FlycastInstallationType::Native,
            config: roots.xdg_config_home.join("flycast"),
            data: roots.xdg_data_home.join("flycast"),
        },
        Candidate {
            kind: FlycastInstallationType::FlatpakUser,
            config: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("config/flycast"),
            data: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("data/flycast"),
        },
    ];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|config| Candidate {
                kind: FlycastInstallationType::Portable,
                data: config.join("data"),
                config,
            }),
    );
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|config| Candidate {
                kind: FlycastInstallationType::Explicit,
                data: config.join("data"),
                config,
            }),
    );
    if let Some(root) = &roots.appimage_directory {
        candidates.push(Candidate {
            kind: FlycastInstallationType::Portable,
            config: root.join("flycast"),
            data: root.join("data"),
        });
    }
    candidates.sort_by(|a, b| a.config.cmp(&b.config));
    candidates.dedup_by(|a, b| a.config == b.config);
    let executables = discover_executables(roots);
    let profiles = candidates
        .into_iter()
        .filter(|candidate| {
            candidate.config.exists()
                || matches!(
                    candidate.kind,
                    FlycastInstallationType::Explicit | FlycastInstallationType::Portable
                )
        })
        .take(FLYCAST_MAX_PROFILES)
        .map(|candidate| profile(candidate, &executables))
        .collect();
    FlycastProfileDiscovery {
        profiles,
        complete: true,
    }
}

pub fn parse_flycast_version(output: &str) -> Option<String> {
    let text = output.trim();
    let index = text.to_ascii_lowercase().find("flycast")?;
    let tail = text[index + 7..]
        .trim_start()
        .strip_prefix('v')
        .unwrap_or(text[index + 7..].trim_start());
    let version: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (version.split('.').count() >= 2).then_some(version)
}

pub fn inspect_flycast_game(
    profile: &FlycastProfile,
    request: &FlycastGameRequest,
) -> FlycastGameInspection {
    let global_config = inspect_config(&profile.config_path);
    let (game_key, mapping, mismatch) = select_game_key(request);
    let per_game_config = game_key.as_ref().and_then(|key| {
        let path = profile.game_settings_path.join(format!("{key}.cfg"));
        is_regular_file(&path).then(|| inspect_config(&path))
    });
    let overrides = per_game_config
        .as_ref()
        .map(|config| differing_keys(&global_config.settings, &config.settings))
        .unwrap_or_default();
    let cheats = game_key
        .as_ref()
        .map(|key| inspect_cheats(&profile.cheats_path.join(format!("{key}.cht"))));
    let textures = game_key.as_ref().map(|key| {
        inspect_textures(
            &profile.textures_path.join(key),
            global_config.settings.texture_replacements_enabled,
        )
    });
    let vmu = inspect_vmu(&profile.vmu_path);
    let save_states = game_key
        .as_ref()
        .map(|key| inspect_saves(&profile.save_states_path, key));
    let system = inspect_system(profile);
    let mut warnings: Vec<String> = global_config
        .warnings
        .iter()
        .map(|warning| warning.detail.clone())
        .collect();
    if let Some(mismatch) = &mismatch {
        warnings.push(mismatch.clone());
    }
    let health = FlycastHealth {
        detected: profile.eligible || !profile.executable_candidates.is_empty(),
        config_readable: global_config.readable,
        system,
        game_key_mapping: mapping,
        warnings,
    };
    FlycastGameInspection {
        game_key,
        emulator_game_key_context: request.emulator_game_key.clone(),
        game_key_mapping: mapping,
        identity_mismatch: mismatch,
        canonical_platform: request.canonical_platform.clone(),
        flycast_platform: request.flycast_platform,
        disc_contexts: request.disc_contexts.iter().take(32).cloned().collect(),
        global_config,
        per_game_config,
        overridden_setting_keys: overrides,
        cheats,
        textures,
        vmu,
        save_states,
        health,
    }
}

fn profile(candidate: Candidate, executables: &[FlycastExecutable]) -> FlycastProfile {
    let config_path = candidate.config.join("emu.cfg");
    let blocker = if !candidate.config.is_absolute() {
        Some("configuration path is not absolute".to_string())
    } else if !is_real_directory(&candidate.config) {
        Some("configuration directory is absent, unsafe, or not a real directory".to_string())
    } else if !is_regular_file(&config_path) {
        Some("emu.cfg was not found as a regular file".to_string())
    } else {
        None
    };
    FlycastProfile {
        profile_id: format!("flycast:{}", candidate.config.display()),
        installation_type: candidate.kind,
        configuration_path: candidate.config.clone(),
        data_path: candidate.data.clone(),
        eligible: blocker.is_none(),
        blocker,
        executable_candidates: executables.to_vec(),
        config_path,
        system_path: candidate.data.join("data"),
        game_settings_path: candidate.data.join("gamesettings"),
        cheats_path: candidate.data.join("cheats"),
        textures_path: candidate.data.join("tex"),
        vmu_path: candidate.data.join("vmu"),
        save_states_path: candidate.data.join("states"),
    }
}

fn discover_executables(roots: &FlycastProfileDiscoveryRoots) -> Vec<FlycastExecutable> {
    let mut paths = roots.explicit_executables.clone();
    if let Some(root) = &roots.appimage_directory {
        paths.extend([
            root.join("Flycast.AppImage"),
            root.join("flycast.AppImage"),
            root.join("flycast"),
        ]);
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path).take(128) {
            paths.push(directory.join("flycast"));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| is_regular_file(path))
        .map(|path| FlycastExecutable {
            installation_type: if roots.explicit_executables.contains(&path) {
                FlycastInstallationType::Explicit
            } else if roots
                .appimage_directory
                .as_ref()
                .is_some_and(|root| path.starts_with(root))
            {
                FlycastInstallationType::Portable
            } else {
                FlycastInstallationType::Native
            },
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|text| parse_flycast_version(text)),
            path,
        })
        .collect()
}

fn normalize_key(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')))
    .then(|| value.to_ascii_uppercase())
}
fn select_game_key(
    request: &FlycastGameRequest,
) -> (Option<String>, FlycastGameKeyMapping, Option<String>) {
    let verified = request
        .verified_dreamcast_product_code
        .as_deref()
        .and_then(normalize_key);
    let emulator = request.emulator_game_key.as_deref().and_then(normalize_key);
    match (verified, emulator) {
        (Some(v), Some(e)) if v != e => (
            Some(v.clone()),
            FlycastGameKeyMapping::ConflictingEmulatorMetadata,
            Some(format!(
                "Flycast game key {e} conflicts with authoritative product code {v}"
            )),
        ),
        (Some(v), _) => (Some(v), FlycastGameKeyMapping::VerifiedProductCode, None),
        (None, Some(e)) => (Some(e), FlycastGameKeyMapping::EmulatorMetadataOnly, None),
        _ => (None, FlycastGameKeyMapping::Unavailable, None),
    }
}

fn inspect_config(path: &Path) -> FlycastConfigInspection {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, FLYCAST_MAX_CONFIG_BYTES, &mut warnings) else {
        return FlycastConfigInspection {
            path: path.to_path_buf(),
            exists,
            readable: false,
            settings: FlycastSettings::default(),
            warnings,
        };
    };
    let settings = parse_settings(&text, path, &mut warnings);
    FlycastConfigInspection {
        path: path.to_path_buf(),
        exists,
        readable: true,
        settings,
        warnings,
    }
}
fn parse_settings(text: &str, path: &Path, warnings: &mut Vec<FlycastWarning>) -> FlycastSettings {
    let mut settings = FlycastSettings::default();
    for (index, raw) in text.lines().enumerate() {
        if index >= MAX_LINES {
            warn(
                warnings,
                FlycastWarningKind::EntryLimitReached,
                path,
                "configuration line limit reached",
            );
            break;
        }
        if raw.len() > MAX_LINE_BYTES {
            warn(
                warnings,
                FlycastWarningKind::FileTooLarge,
                path,
                "configuration line too long",
            );
            continue;
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warn(
                warnings,
                FlycastWarningKind::MalformedConfig,
                path,
                "setting has no '=' separator",
            );
            continue;
        };
        apply_setting(&mut settings, key.trim(), value.trim());
    }
    settings
}
fn apply_setting(settings: &mut FlycastSettings, key: &str, value: &str) {
    let key = key.to_ascii_lowercase();
    let boolean = parse_bool(value);
    match key.as_str() {
        "renderer" | "rend" => settings.renderer = Some(value.to_string()),
        "resolution" | "internal_resolution" => {
            settings.internal_resolution = Some(value.to_string())
        }
        "widescreen" => settings.widescreen = boolean,
        "vsync" => settings.vsync = boolean,
        "texture_filtering" => settings.texture_filtering = Some(value.to_string()),
        "texture_upscaling" => settings.texture_upscaling = Some(value.to_string()),
        "per_pixel" | "per_pixel_sorting" => settings.per_pixel_sorting = boolean,
        "audio_backend" => settings.audio_backend = Some(value.to_string()),
        "region" => settings.region = Some(value.to_string()),
        "language" => settings.language = Some(value.to_string()),
        "cable" | "cable_type" => settings.cable_type = Some(value.to_string()),
        "network" | "network_mode" => settings.network_mode = Some(value.to_string()),
        "cheats" | "cheats_enabled" => settings.cheats_enabled = boolean,
        "texture_replacements" | "texture_replacements_enabled" => {
            settings.texture_replacements_enabled = boolean
        }
        "controller_profile" => settings.controller_profile_present = Some(!value.is_empty()),
        _ if settings.unknown.len() < MAX_UNKNOWN_SETTINGS => {
            settings.unknown.insert(key, value.to_string());
        }
        _ => {}
    }
}
fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}
fn differing_keys(global: &FlycastSettings, game: &FlycastSettings) -> Vec<String> {
    let a = flatten(global);
    let b = flatten(game);
    b.into_iter()
        .filter_map(|(key, value)| (a.get(&key) != Some(&value)).then_some(key))
        .collect()
}
fn flatten(settings: &FlycastSettings) -> BTreeMap<String, String> {
    let mut result = settings.unknown.clone();
    for (key, value) in [
        ("renderer", settings.renderer.clone()),
        ("internal_resolution", settings.internal_resolution.clone()),
        ("texture_filtering", settings.texture_filtering.clone()),
        ("texture_upscaling", settings.texture_upscaling.clone()),
        ("audio_backend", settings.audio_backend.clone()),
        ("region", settings.region.clone()),
        ("language", settings.language.clone()),
        ("cable_type", settings.cable_type.clone()),
        ("network_mode", settings.network_mode.clone()),
    ] {
        if let Some(value) = value {
            result.insert(key.to_string(), value);
        }
    }
    result
}

fn inspect_system(profile: &FlycastProfile) -> FlycastSystemHealth {
    let bios = state(&profile.system_path.join("dc_boot.bin"));
    let flash = state(&profile.system_path.join("dc_flash.bin"));
    let arcade = if is_real_directory(&profile.system_path) {
        FlycastSystemFileState::Unknown
    } else {
        FlycastSystemFileState::NotConfigured
    };
    FlycastSystemHealth {
        dreamcast_bios: bios,
        dreamcast_flash: flash,
        arcade_system_roms: arcade,
    }
}
fn state(path: &Path) -> FlycastSystemFileState {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => FlycastSystemFileState::Unreadable,
        Ok(metadata) if metadata.is_file() => FlycastSystemFileState::PresentUnverified,
        Ok(_) => FlycastSystemFileState::Unreadable,
        Err(error) if error.kind() == io::ErrorKind::NotFound => FlycastSystemFileState::Missing,
        Err(_) => FlycastSystemFileState::Unreadable,
    }
}
fn inspect_cheats(path: &Path) -> FlycastCheatInventory {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, FLYCAST_MAX_CHEAT_BYTES, &mut warnings) else {
        return FlycastCheatInventory {
            path: path.to_path_buf(),
            exists,
            entries: 0,
            enabled_entries: 0,
            warnings,
        };
    };
    let mut entries = 0;
    let mut enabled_entries = 0;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            entries += 1;
        }
        if line.to_ascii_lowercase().starts_with("enabled=") && parse_bool(&line[8..]) == Some(true)
        {
            enabled_entries += 1;
        }
    }
    FlycastCheatInventory {
        path: path.to_path_buf(),
        exists,
        entries,
        enabled_entries,
        warnings,
    }
}
fn inspect_textures(path: &Path, enabled: Option<bool>) -> FlycastTextureInventory {
    let mut out = FlycastTextureInventory {
        path: path.to_path_buf(),
        present: is_real_directory(path),
        enabled,
        file_count: 0,
        total_size_bytes: 0,
        complete: true,
        warnings: Vec::new(),
    };
    if !out.present {
        return out;
    }
    let mut todo = VecDeque::from([(path.to_path_buf(), 0usize)]);
    let mut visited = 0;
    while let Some((dir, depth)) = todo.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            out.complete = false;
            continue;
        };
        for entry in entries.flatten() {
            if visited >= FLYCAST_MAX_DIRECTORY_ENTRIES {
                out.complete = false;
                return out;
            }
            visited += 1;
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                out.complete = false;
                continue;
            };
            if meta.file_type().is_symlink() {
                warn(
                    &mut out.warnings,
                    FlycastWarningKind::SymlinkSkipped,
                    &path,
                    "symlink was not followed",
                )
            } else if meta.is_file() {
                if out.file_count >= FLYCAST_MAX_TEXTURE_FILES {
                    out.complete = false;
                    return out;
                }
                out.file_count += 1;
                out.total_size_bytes = out.total_size_bytes.saturating_add(meta.len())
            } else if meta.is_dir() && depth < FLYCAST_MAX_TEXTURE_DEPTH {
                todo.push_back((path, depth + 1))
            } else if meta.is_dir() {
                out.complete = false;
                warn(
                    &mut out.warnings,
                    FlycastWarningKind::DepthLimitReached,
                    &path,
                    "texture depth limit reached",
                )
            }
        }
    }
    out
}
fn inspect_vmu(path: &Path) -> FlycastVmuInventory {
    let mut out = FlycastVmuInventory {
        vmu_images: Vec::new(),
        complete: true,
        warnings: Vec::new(),
    };
    if !is_real_directory(path) {
        return out;
    }
    let Ok(entries) = fs::read_dir(path) else {
        out.complete = false;
        return out;
    };
    for (i, entry) in entries.flatten().enumerate() {
        if i >= FLYCAST_MAX_DIRECTORY_ENTRIES {
            out.complete = false;
            break;
        }
        let path = entry.path();
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
            out.vmu_images.push(path)
        }
    }
    out.vmu_images.sort();
    out
}
fn inspect_saves(path: &Path, key: &str) -> FlycastSaveStateInventory {
    let mut out = FlycastSaveStateInventory {
        candidate_paths: Vec::new(),
        complete: true,
        warnings: Vec::new(),
    };
    if !is_real_directory(path) {
        return out;
    }
    let Ok(entries) = fs::read_dir(path) else {
        out.complete = false;
        return out;
    };
    for (i, entry) in entries.flatten().enumerate() {
        if i >= FLYCAST_MAX_DIRECTORY_ENTRIES
            || out.candidate_paths.len() >= FLYCAST_MAX_SAVE_CANDIDATES
        {
            out.complete = false;
            break;
        }
        let path = entry.path();
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
            && entry
                .file_name()
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with(key)
        {
            out.candidate_paths.push(path)
        }
    }
    out.candidate_paths.sort();
    out
}
fn read_text(path: &Path, max: u64, warnings: &mut Vec<FlycastWarning>) -> Option<String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            warn(
                warnings,
                FlycastWarningKind::UnreadablePath,
                path,
                format!("cannot inspect file: {error}"),
            );
            return None;
        }
    };
    if meta.file_type().is_symlink() {
        warn(
            warnings,
            FlycastWarningKind::SymlinkSkipped,
            path,
            "symlink was not followed",
        );
        return None;
    }
    if !meta.is_file() {
        warn(
            warnings,
            FlycastWarningKind::SpecialFileSkipped,
            path,
            "non-regular file was skipped",
        );
        return None;
    }
    if meta.len() > max {
        warn(
            warnings,
            FlycastWarningKind::FileTooLarge,
            path,
            "file exceeds bound",
        );
        return None;
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    opts.custom_flags(libc::O_NOFOLLOW);
    let mut file = opts.open(path).ok()?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.by_ref().take(max + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > max {
        warn(
            warnings,
            FlycastWarningKind::FileTooLarge,
            path,
            "file grew beyond bound",
        );
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) => {
            warn(
                warnings,
                FlycastWarningKind::InvalidUtf8,
                path,
                "invalid UTF-8 replaced",
            );
            Some(String::from_utf8_lossy(error.as_bytes()).into_owned())
        }
    }
}
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}
fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}
fn warn(
    w: &mut Vec<FlycastWarning>,
    kind: FlycastWarningKind,
    path: &Path,
    detail: impl Into<String>,
) {
    w.push(FlycastWarning {
        kind,
        path: path.to_path_buf(),
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn roots(t: &TempDir) -> FlycastProfileDiscoveryRoots {
        FlycastProfileDiscoveryRoots {
            home: t.path().join("home"),
            xdg_config_home: t.path().join("config"),
            xdg_data_home: t.path().join("data"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        }
    }
    fn write(root: &Path, text: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("emu.cfg"), text).unwrap()
    }
    fn profile(root: PathBuf) -> FlycastProfile {
        let home = root.parent().unwrap().to_path_buf();
        discover_flycast_profiles(&FlycastProfileDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home,
            xdg_data_home: root.parent().unwrap().join("data"),
            explicit_configuration_roots: vec![root.clone()],
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        })
        .profiles
        .into_iter()
        .find(|p| p.configuration_path == root && p.eligible)
        .unwrap()
    }
    fn verified(key: &str) -> FlycastGameRequest {
        FlycastGameRequest {
            verified_dreamcast_product_code: Some(key.to_string()),
            ..Default::default()
        }
    }
    #[test]
    fn discovery_version_and_missing_executable() {
        let t = TempDir::new().unwrap();
        let mut r = roots(&t);
        write(&r.xdg_config_home.join("flycast"), "");
        let flat = r
            .home
            .join(".var/app")
            .join(FLATPAK_APP_ID)
            .join("config/flycast");
        write(&flat, "");
        let portable = t.path().join("portable");
        write(&portable, "");
        r.portable_configuration_roots.push(portable);
        let app = t.path().join("Flycast.AppImage");
        fs::write(&app, b"not run").unwrap();
        r.explicit_executables.push(app.clone());
        r.known_version_outputs
            .insert(app, "Flycast v2.3-1".to_string());
        let d = discover_flycast_profiles(&r);
        assert!(
            d.profiles
                .iter()
                .any(|p| p.installation_type == FlycastInstallationType::Native && p.eligible)
        );
        assert!(
            d.profiles
                .iter()
                .any(|p| p.installation_type == FlycastInstallationType::FlatpakUser && p.eligible)
        );
        assert_eq!(
            parse_flycast_version("Flycast v2.3-1"),
            Some("2.3".to_string())
        );
        assert_eq!(parse_flycast_version("unknown"), None)
    }
    #[test]
    fn config_identity_and_disc_representations_stay_separate() {
        let t = TempDir::new().unwrap();
        let root = t.path().join("fly");
        write(&root, "renderer=Vulkan\nwidescreen=true\n");
        fs::create_dir_all(root.join("data/gamesettings")).unwrap();
        fs::write(
            root.join("data/gamesettings/T-8109N.cfg"),
            "renderer=OpenGL\n",
        )
        .unwrap();
        let i = inspect_flycast_game(
            &profile(root),
            &FlycastGameRequest {
                canonical_platform: Some("Dreamcast".into()),
                flycast_platform: Some(FlycastPlatform::Dreamcast),
                verified_dreamcast_product_code: Some("T-8109N".into()),
                emulator_game_key: Some("OTHER".into()),
                disc_contexts: vec![
                    FlycastDiscContext {
                        disc_number: 1,
                        representation: Representation::DiscTrack,
                        claim: ClaimType::ExactTrackMatch,
                    },
                    FlycastDiscContext {
                        disc_number: 2,
                        representation: Representation::LogicalChd,
                        claim: ClaimType::ExactLogicalDiscMatch,
                    },
                ],
            },
        );
        assert_eq!(i.game_key.as_deref(), Some("T-8109N"));
        assert_eq!(
            i.game_key_mapping,
            FlycastGameKeyMapping::ConflictingEmulatorMetadata
        );
        assert_eq!(i.disc_contexts.len(), 2);
        assert_eq!(i.disc_contexts[0].representation, Representation::DiscTrack);
        assert_eq!(
            i.disc_contexts[1].representation,
            Representation::LogicalChd
        );
        assert_eq!(
            i.per_game_config.unwrap().settings.renderer.as_deref(),
            Some("OpenGL")
        )
    }
    #[test]
    fn cdi_naomi_atomiswave_and_metadata_have_no_identity_authority() {
        let t = TempDir::new().unwrap();
        let root = t.path().join("fly");
        write(&root, "bad line\n");
        let p = profile(root);
        let unresolved = inspect_flycast_game(
            &p,
            &FlycastGameRequest {
                flycast_platform: Some(FlycastPlatform::Naomi),
                emulator_game_key: Some("NAOMI_KEY".into()),
                disc_contexts: vec![FlycastDiscContext {
                    disc_number: 1,
                    representation: Representation::RawDisc,
                    claim: ClaimType::ExactBytesMatch,
                }],
                ..Default::default()
            },
        );
        assert_eq!(
            unresolved.game_key_mapping,
            FlycastGameKeyMapping::EmulatorMetadataOnly
        );
        assert_eq!(unresolved.flycast_platform, Some(FlycastPlatform::Naomi));
        assert!(
            unresolved
                .global_config
                .warnings
                .iter()
                .any(|w| w.kind == FlycastWarningKind::MalformedConfig)
        );
        assert_ne!(
            Some(FlycastPlatform::Atomiswave),
            unresolved.flycast_platform
        );
        let naomi2 = inspect_flycast_game(
            &p,
            &FlycastGameRequest {
                canonical_platform: Some("Unrelated Platform".to_string()),
                flycast_platform: Some(FlycastPlatform::Naomi2),
                ..Default::default()
            },
        );
        assert_eq!(
            naomi2.canonical_platform.as_deref(),
            Some("Unrelated Platform")
        );
        assert_eq!(naomi2.flycast_platform, Some(FlycastPlatform::Naomi2));
        assert_eq!(naomi2.game_key_mapping, FlycastGameKeyMapping::Unavailable)
    }
    #[test]
    fn system_cheats_textures_vmu_and_saves_are_bounded() {
        let t = TempDir::new().unwrap();
        let root = t.path().join("fly");
        write(&root, "texture_replacements=true\n");
        fs::create_dir_all(root.join("data/data")).unwrap();
        fs::write(root.join("data/data/dc_boot.bin"), b"bios").unwrap();
        fs::create_dir_all(root.join("data/cheats")).unwrap();
        fs::write(
            root.join("data/cheats/T-8109N.cht"),
            "[one]\nenabled=true\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("data/tex/T-8109N/n")).unwrap();
        fs::write(root.join("data/tex/T-8109N/n/a.png"), b"t").unwrap();
        fs::create_dir_all(root.join("data/vmu")).unwrap();
        fs::write(root.join("data/vmu/a1.bin"), b"vmu").unwrap();
        fs::create_dir_all(root.join("data/states")).unwrap();
        fs::write(root.join("data/states/T-8109N_0.state"), b"state").unwrap();
        let i = inspect_flycast_game(&profile(root), &verified("T-8109N"));
        assert_eq!(
            i.health.system.dreamcast_bios,
            FlycastSystemFileState::PresentUnverified
        );
        assert_eq!(
            i.health.system.dreamcast_flash,
            FlycastSystemFileState::Missing
        );
        assert_eq!(i.cheats.unwrap().enabled_entries, 1);
        assert_eq!(i.textures.unwrap().file_count, 1);
        assert_eq!(i.vmu.vmu_images.len(), 1);
        assert_eq!(i.save_states.unwrap().candidate_paths.len(), 1)
    }
    #[test]
    fn oversized_files_and_deep_textures_fail_soft() {
        let t = TempDir::new().unwrap();
        let root = t.path().join("fly");
        write(&root, "");
        fs::create_dir_all(root.join("data/cheats")).unwrap();
        fs::write(
            root.join("data/cheats/T-8109N.cht"),
            vec![b'x'; FLYCAST_MAX_CHEAT_BYTES as usize + 1],
        )
        .unwrap();
        let mut deep = root.join("data/tex/T-8109N");
        for _ in 0..FLYCAST_MAX_TEXTURE_DEPTH + 2 {
            deep = deep.join("n");
            fs::create_dir_all(&deep).unwrap()
        }
        let i = inspect_flycast_game(&profile(root), &verified("T-8109N"));
        assert_eq!(i.cheats.unwrap().entries, 0);
        assert!(!i.textures.unwrap().complete)
    }
}
