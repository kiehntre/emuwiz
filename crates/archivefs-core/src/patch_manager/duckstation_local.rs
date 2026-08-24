//! Bounded, read-only inspection of local DuckStation profiles.
//!
//! DuckStation consumes identity that core has already established.  In
//! particular, its serials, profile names, cheat filenames, texture-pack
//! directories, and M3U playlists are emulator context only: this module does
//! not emit identity evidence or select a winner between emulator metadata and
//! Redump/CHD evidence.  It never starts DuckStation, writes a file, follows a
//! symlink, or recursively searches a home directory.

use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::Serialize;

use crate::game_identity::serial_from_boot_path;
use crate::platform_evidence_fusion::evidence_lineage::{ClaimType, Representation};

pub const DUCKSTATION_MAX_PROFILES: usize = 16;
pub const DUCKSTATION_MAX_CONFIG_BYTES: u64 = 256 * 1024;
pub const DUCKSTATION_MAX_CHEAT_BYTES: u64 = 512 * 1024;
pub const DUCKSTATION_MAX_PATCH_BYTES: u64 = 512 * 1024;
pub const DUCKSTATION_MAX_PLAYLIST_BYTES: u64 = 256 * 1024;
pub const DUCKSTATION_MAX_DIRECTORY_ENTRIES: usize = 10_000;
pub const DUCKSTATION_MAX_TEXTURE_FILES: usize = 2_048;
pub const DUCKSTATION_MAX_TEXTURE_DEPTH: usize = 2;
pub const DUCKSTATION_MAX_SAVE_STATE_CANDIDATES: usize = 128;
pub const DUCKSTATION_MAX_PLAYLIST_ENTRIES: usize = 64;

const FLATPAK_APP_ID: &str = "org.duckstation.DuckStation";
const MAX_INI_LINES: usize = 8_192;
const MAX_INI_LINE_BYTES: usize = 8 * 1024;
const MAX_UNKNOWN_SETTINGS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DuckStationInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DuckStationWarningKind {
    UnsafePath,
    UnreadablePath,
    SymlinkSkipped,
    SpecialFileSkipped,
    FileTooLarge,
    InvalidUtf8,
    MalformedIni,
    EntryLimitReached,
    FileCountLimitReached,
    DepthLimitReached,
    PlaylistEntryLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationWarning {
    pub kind: DuckStationWarningKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationExecutable {
    pub path: PathBuf,
    pub installation_type: DuckStationInstallationType,
    /// Discovery never executes a binary.  An authorized outer probe may pass
    /// its text to [`parse_duckstation_version`] through discovery roots.
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationProfile {
    pub profile_id: String,
    pub installation_type: DuckStationInstallationType,
    pub configuration_path: PathBuf,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<DuckStationExecutable>,
    pub global_config_path: PathBuf,
    pub game_settings_path: PathBuf,
    pub cheats_path: PathBuf,
    pub patches_path: PathBuf,
    pub textures_path: PathBuf,
    pub bios_path: PathBuf,
    pub memory_cards_path: PathBuf,
    pub save_states_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationProfileDiscovery {
    pub profiles: Vec<DuckStationProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    /// Exact roots deliberately supplied by the caller.  They are not found
    /// by a broad filesystem walk.
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
}

impl DuckStationProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, DuckStationDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(DuckStationDiscoveryError::HomeUnavailable)?;
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
pub enum DuckStationDiscoveryError {
    HomeUnavailable,
}

impl std::fmt::Display for DuckStationDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable => formatter.write_str("HOME is not set"),
        }
    }
}

impl std::error::Error for DuckStationDiscoveryError {}

/// A preservation fact passed in by core.  This remains descriptive context;
/// DuckStation never compares its serial with either hash value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationDiscContext {
    pub disc_number: u8,
    pub representation: Representation,
    pub claim: ClaimType,
}

/// Identity input deliberately separates an authoritative serial from one
/// merely exposed by DuckStation.  `disc_contexts` preserves separate discs;
/// no M3U title or filename can collapse them into one exact disc claim.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DuckStationGameRequest {
    pub verified_ps1_serial: Option<String>,
    pub emulator_serial: Option<String>,
    pub disc_contexts: Vec<DuckStationDiscContext>,
    pub playlist_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DuckStationSerialMapping {
    VerifiedPs1Serial,
    EmulatorMetadataOnly,
    ConflictingEmulatorMetadata,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DuckStationSettings {
    pub renderer: Option<String>,
    pub internal_resolution_scale: Option<String>,
    pub texture_filtering: Option<String>,
    pub pgxp_enabled: Option<bool>,
    pub widescreen_enabled: Option<bool>,
    pub vsync_enabled: Option<bool>,
    pub frame_pacing: Option<String>,
    pub frame_limit: Option<String>,
    pub cpu_execution_mode: Option<String>,
    pub audio_backend: Option<String>,
    pub rewind_enabled: Option<bool>,
    pub runahead_enabled: Option<bool>,
    pub cheats_enabled: Option<bool>,
    pub texture_replacements_enabled: Option<bool>,
    pub controller_profile_present: Option<bool>,
    pub bios_directory: Option<String>,
    pub bios_filename: Option<String>,
    pub memory_card_mode: Option<String>,
    pub unknown: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationConfigInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub settings: DuckStationSettings,
    pub warnings: Vec<DuckStationWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DuckStationBiosState {
    /// A trusted hash source confirmed this exact file - see
    /// `patch_manager::duckstation_firmware::resolve_duckstation_bios`,
    /// which is the only place this variant is ever produced. Never
    /// produced by this module itself (filename/existence alone never
    /// verifies a BIOS) - see [`Self::PresentUnverified`].
    Verified,
    Unknown,
    Missing,
    PresentUnverified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationBiosInventory {
    pub configured_path: Option<PathBuf>,
    pub state: DuckStationBiosState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationCheatInventory {
    pub path: PathBuf,
    pub exists: bool,
    pub entries: usize,
    pub enabled_entries: usize,
    pub warnings: Vec<DuckStationWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationPatchInventory {
    pub path: PathBuf,
    pub exists: bool,
    pub entries: usize,
    pub warnings: Vec<DuckStationWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationTextureInventory {
    pub path: PathBuf,
    pub present: bool,
    pub enabled: Option<bool>,
    pub file_count: usize,
    pub total_size_bytes: u64,
    pub complete: bool,
    pub warnings: Vec<DuckStationWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationMemoryCardInventory {
    /// A serial-named candidate, not proof that a memory card belongs
    /// exclusively to this title.  Only explicit DuckStation configuration
    /// could establish that stronger relationship.
    pub per_game_card_candidate: Option<PathBuf>,
    pub shared_cards: Vec<PathBuf>,
    pub complete: bool,
    pub warnings: Vec<DuckStationWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationSaveStateInventory {
    pub candidate_paths: Vec<PathBuf>,
    pub complete: bool,
    pub warnings: Vec<DuckStationWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationPlaylistInventory {
    pub path: PathBuf,
    pub exists: bool,
    pub entries: Vec<String>,
    pub complete: bool,
    pub warnings: Vec<DuckStationWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationHealth {
    pub detected: bool,
    pub config_readable: bool,
    pub bios: DuckStationBiosState,
    pub serial_mapping: DuckStationSerialMapping,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationGameInspection {
    pub serial: Option<String>,
    /// The raw emulator value is retained only as context.  It is not
    /// normalized into a preservation claim when unresolved or conflicting.
    pub emulator_serial_context: Option<String>,
    pub serial_mapping: DuckStationSerialMapping,
    pub identity_mismatch: Option<String>,
    pub disc_contexts: Vec<DuckStationDiscContext>,
    pub playlist: Option<DuckStationPlaylistInventory>,
    pub global_config: DuckStationConfigInspection,
    pub per_game_config: Option<DuckStationConfigInspection>,
    pub overridden_setting_keys: Vec<String>,
    pub cheats: Option<DuckStationCheatInventory>,
    pub patches: Option<DuckStationPatchInventory>,
    pub textures: Option<DuckStationTextureInventory>,
    pub memory_cards: Option<DuckStationMemoryCardInventory>,
    pub save_states: Option<DuckStationSaveStateInventory>,
    /// The configured BIOS path (if any) and its presence-only state -
    /// exposed here (mirroring `Pcsx2GameInspection::bios`) so a firmware
    /// verifier can locate the exact file to hash without re-parsing
    /// configuration itself. See `patch_manager::duckstation_firmware`.
    pub bios: DuckStationBiosInventory,
    pub health: DuckStationHealth,
}

#[derive(Debug, Clone)]
struct ProfileCandidate {
    installation_type: DuckStationInstallationType,
    path: PathBuf,
}

/// Discover documented native and Flatpak roots plus exact caller-provided
/// portable/custom roots.  The executable search is bounded to PATH entries
/// and exact supplied/AppImage-adjacent paths; no binary is executed.
pub fn discover_duckstation_profiles(
    roots: &DuckStationProfileDiscoveryRoots,
) -> DuckStationProfileDiscovery {
    let mut candidates = vec![
        ProfileCandidate {
            installation_type: DuckStationInstallationType::Native,
            path: roots.xdg_config_home.join("duckstation"),
        },
        ProfileCandidate {
            installation_type: DuckStationInstallationType::Native,
            path: roots.xdg_data_home.join("duckstation"),
        },
        ProfileCandidate {
            installation_type: DuckStationInstallationType::FlatpakUser,
            path: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("config/duckstation"),
        },
        ProfileCandidate {
            installation_type: DuckStationInstallationType::FlatpakUser,
            path: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("data/duckstation"),
        },
    ];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|path| ProfileCandidate {
                installation_type: DuckStationInstallationType::Portable,
                path,
            }),
    );
    if let Some(directory) = &roots.appimage_directory {
        candidates.push(ProfileCandidate {
            installation_type: DuckStationInstallationType::Portable,
            path: directory.join("duckstation"),
        });
    }
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|path| ProfileCandidate {
                installation_type: DuckStationInstallationType::Explicit,
                path,
            }),
    );
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by(|left, right| left.path == right.path);

    let executables = discover_executables(roots);
    let mut profiles = Vec::new();
    for candidate in candidates.into_iter().take(DUCKSTATION_MAX_PROFILES) {
        if !candidate.path.exists()
            && !matches!(
                candidate.installation_type,
                DuckStationInstallationType::Explicit | DuckStationInstallationType::Portable
            )
        {
            continue;
        }
        profiles.push(validate_profile(candidate, &executables));
    }
    DuckStationProfileDiscovery {
        complete: true,
        profiles,
    }
}

/// Parses externally obtained version output; discovery itself never runs an
/// executable.  Non-version text is valid and simply reports `None`.
pub fn parse_duckstation_version(output: &str) -> Option<String> {
    let text = output.trim();
    let position = text.to_ascii_lowercase().find("duckstation")?;
    let tail = text[position + "duckstation".len()..].trim_start();
    let tail = tail.strip_prefix('v').unwrap_or(tail);
    let version: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    (version.split('.').count() >= 2 && version.chars().any(|character| character.is_ascii_digit()))
        .then_some(version)
}

/// Uses the existing reviewed boot-path serial grammar rather than a local
/// prefix list.  It accepts only core's canonical `XXXX-12345` output shape.
pub fn normalize_duckstation_ps1_serial(value: &str) -> Option<String> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || !bytes[..4].iter().all(u8::is_ascii_alphanumeric)
        || !bytes[5..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    // `serial_from_boot_path` defines the accepted grammar.  It expects the
    // original executable filename (`SLUS_123.45`), while core hands this
    // adapter its canonical output (`SLUS-12345`), so reconstruct only that
    // reversible shape before asking the existing parser to validate it.
    let boot_name = format!("{}_{}.{}", &value[..4], &value[5..8], &value[8..10]);
    serial_from_boot_path(boot_name.as_bytes())
}

/// Inspects local settings/assets after core supplies identity context.  It
/// intentionally returns no `EvidenceObservation` and has no mutation path.
pub fn inspect_duckstation_game(
    profile: &DuckStationProfile,
    request: &DuckStationGameRequest,
) -> DuckStationGameInspection {
    let global_config = inspect_config(&profile.global_config_path);
    let (serial, serial_mapping, mismatch) = select_serial(request);
    let per_game_config = serial.as_ref().and_then(|serial| {
        let path = profile.game_settings_path.join(format!("{serial}.ini"));
        is_regular_file(&path).then(|| inspect_config(&path))
    });
    let overridden_setting_keys = per_game_config
        .as_ref()
        .map(|config| differing_keys(&global_config.settings, &config.settings))
        .unwrap_or_default();
    let cheats = serial
        .as_ref()
        .map(|serial| inspect_cheats(&profile.cheats_path.join(format!("{serial}.cht"))));
    let patches = serial
        .as_ref()
        .map(|serial| inspect_patches(&profile.patches_path, serial));
    let textures = serial.as_ref().map(|serial| {
        inspect_textures(
            &profile.textures_path.join(serial),
            global_config.settings.texture_replacements_enabled,
        )
    });
    let memory_cards = serial
        .as_ref()
        .map(|serial| inspect_memory_cards(&profile.memory_cards_path, serial));
    let save_states = serial
        .as_ref()
        .map(|serial| inspect_save_states(&profile.save_states_path, serial));
    let playlist = request.playlist_path.as_deref().map(inspect_playlist);
    let bios = inspect_bios(profile, &global_config.settings);
    let mut warning_text: Vec<String> = global_config
        .warnings
        .iter()
        .map(|warning| warning.detail.clone())
        .collect();
    if let Some(mismatch) = &mismatch {
        warning_text.push(mismatch.clone());
    }
    let health = DuckStationHealth {
        detected: profile.eligible || !profile.executable_candidates.is_empty(),
        config_readable: global_config.readable,
        bios: bios.state,
        serial_mapping,
        warnings: warning_text,
    };
    DuckStationGameInspection {
        serial,
        emulator_serial_context: request.emulator_serial.clone(),
        serial_mapping,
        identity_mismatch: mismatch,
        disc_contexts: request
            .disc_contexts
            .iter()
            .take(DUCKSTATION_MAX_PLAYLIST_ENTRIES)
            .cloned()
            .collect(),
        playlist,
        global_config,
        per_game_config,
        overridden_setting_keys,
        cheats,
        patches,
        textures,
        memory_cards,
        save_states,
        bios,
        health,
    }
}

fn validate_profile(
    candidate: ProfileCandidate,
    executables: &[DuckStationExecutable],
) -> DuckStationProfile {
    let global_config_path = candidate.path.join("settings.ini");
    let blocker = if !candidate.path.is_absolute() {
        Some("configuration path is not absolute".to_string())
    } else if candidate.path.parent().is_none() {
        Some("a filesystem root cannot be a DuckStation profile".to_string())
    } else if !is_real_directory(&candidate.path) {
        Some("configuration directory is absent, unsafe, or not a real directory".to_string())
    } else if !is_regular_file(&global_config_path) {
        Some("settings.ini was not found as a regular file".to_string())
    } else {
        None
    };
    DuckStationProfile {
        profile_id: format!("duckstation:{}", candidate.path.display()),
        installation_type: candidate.installation_type,
        configuration_path: candidate.path.clone(),
        eligible: blocker.is_none(),
        blocker,
        executable_candidates: executables.to_vec(),
        global_config_path,
        game_settings_path: candidate.path.join("gamesettings"),
        cheats_path: candidate.path.join("cheats"),
        patches_path: candidate.path.join("patches"),
        textures_path: candidate.path.join("textures"),
        bios_path: candidate.path.join("bios"),
        memory_cards_path: candidate.path.join("memcards"),
        save_states_path: candidate.path.join("savestates"),
    }
}

fn discover_executables(roots: &DuckStationProfileDiscoveryRoots) -> Vec<DuckStationExecutable> {
    let mut candidates = roots.explicit_executables.clone();
    if let Some(directory) = &roots.appimage_directory {
        candidates.extend([
            directory.join("DuckStation.AppImage"),
            directory.join("duckstation-qt.AppImage"),
            directory.join("duckstation-qt"),
        ]);
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path).take(128) {
            candidates.extend([
                directory.join("duckstation-qt"),
                directory.join("duckstation"),
            ]);
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
        .into_iter()
        .filter(|path| is_regular_file(path))
        .map(|path| DuckStationExecutable {
            installation_type: if roots.explicit_executables.contains(&path) {
                DuckStationInstallationType::Explicit
            } else if roots
                .appimage_directory
                .as_ref()
                .is_some_and(|root| path.starts_with(root))
            {
                DuckStationInstallationType::Portable
            } else {
                DuckStationInstallationType::Native
            },
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|text| parse_duckstation_version(text)),
            path,
        })
        .collect()
}

fn select_serial(
    request: &DuckStationGameRequest,
) -> (Option<String>, DuckStationSerialMapping, Option<String>) {
    let verified = request
        .verified_ps1_serial
        .as_deref()
        .and_then(normalize_duckstation_ps1_serial);
    let emulator = request
        .emulator_serial
        .as_deref()
        .and_then(normalize_duckstation_ps1_serial);
    match (verified, emulator) {
        (Some(verified), Some(emulator)) if verified != emulator => (
            Some(verified.clone()),
            DuckStationSerialMapping::ConflictingEmulatorMetadata,
            Some(format!(
                "DuckStation serial {emulator} conflicts with authoritative PS1 serial {verified}"
            )),
        ),
        (Some(verified), _) => (
            Some(verified),
            DuckStationSerialMapping::VerifiedPs1Serial,
            None,
        ),
        (None, Some(emulator)) => (
            Some(emulator),
            DuckStationSerialMapping::EmulatorMetadataOnly,
            None,
        ),
        (None, None) => (None, DuckStationSerialMapping::Unavailable, None),
    }
}

fn inspect_config(path: &Path) -> DuckStationConfigInspection {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, DUCKSTATION_MAX_CONFIG_BYTES, &mut warnings) else {
        return DuckStationConfigInspection {
            path: path.to_path_buf(),
            exists,
            readable: false,
            settings: DuckStationSettings::default(),
            warnings,
        };
    };
    DuckStationConfigInspection {
        path: path.to_path_buf(),
        exists,
        readable: true,
        settings: parse_settings(&text, path, &mut warnings),
        warnings,
    }
}

fn parse_settings(
    text: &str,
    path: &Path,
    warnings: &mut Vec<DuckStationWarning>,
) -> DuckStationSettings {
    let mut settings = DuckStationSettings::default();
    let mut section = String::new();
    for (index, raw) in text.lines().enumerate() {
        if index >= MAX_INI_LINES {
            warn(
                warnings,
                DuckStationWarningKind::EntryLimitReached,
                path,
                format!("INI parsing stopped at {MAX_INI_LINES} lines"),
            );
            break;
        }
        if raw.len() > MAX_INI_LINE_BYTES {
            warn(
                warnings,
                DuckStationWarningKind::FileTooLarge,
                path,
                format!("INI line exceeds {MAX_INI_LINE_BYTES} bytes"),
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
                    DuckStationWarningKind::MalformedIni,
                    path,
                    "INI section does not end with ']'",
                );
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warn(
                warnings,
                DuckStationWarningKind::MalformedIni,
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
                DuckStationWarningKind::MalformedIni,
                path,
                "INI setting has an empty key",
            );
            continue;
        }
        apply_setting(&mut settings, &section, key, value);
    }
    settings
}

fn apply_setting(settings: &mut DuckStationSettings, section: &str, key: &str, value: &str) {
    let key = key.to_ascii_lowercase();
    let boolean = parse_bool(value);
    match key.as_str() {
        "renderer" | "gpu_renderer" => settings.renderer = Some(value.to_string()),
        "resolutionscale" | "resolution_scale" | "internal_resolution_scale" => {
            settings.internal_resolution_scale = Some(value.to_string())
        }
        "texturefiltering" | "texture_filtering" => {
            settings.texture_filtering = Some(value.to_string())
        }
        "pgxpenable" | "pgxp_enable" | "pgxp_enabled" => settings.pgxp_enabled = boolean,
        "widescreenhack" | "widescreen_hack" | "widescreen_enabled" => {
            settings.widescreen_enabled = boolean
        }
        "vsync" | "vsyncenabled" | "vsync_enabled" => settings.vsync_enabled = boolean,
        "framepacing" | "frame_pacing" => settings.frame_pacing = Some(value.to_string()),
        "framelimit" | "frame_limit" => settings.frame_limit = Some(value.to_string()),
        "cpuexecutionmode" | "cpu_execution_mode" | "cpu_mode" => {
            settings.cpu_execution_mode = Some(value.to_string())
        }
        "audiobackend" | "audio_backend" => settings.audio_backend = Some(value.to_string()),
        "rewindenable" | "rewind_enable" | "rewind_enabled" => settings.rewind_enabled = boolean,
        "runaheadenable" | "runahead_enable" | "runahead_enabled" => {
            settings.runahead_enabled = boolean
        }
        "enablecheats" | "enable_cheats" | "cheats_enabled" => settings.cheats_enabled = boolean,
        "texturereplacements" | "texture_replacements" | "texture_replacements_enabled" => {
            settings.texture_replacements_enabled = boolean
        }
        "controllerprofile" | "controller_profile" | "controller_profile_name" => {
            settings.controller_profile_present = Some(!value.is_empty())
        }
        "biosdirectory" | "bios_directory" | "bios_path" => {
            settings.bios_directory = Some(value.to_string())
        }
        "biosfilename" | "bios_filename" | "bios" => {
            settings.bios_filename = Some(value.to_string())
        }
        "memorycardmode" | "memory_card_mode" => {
            settings.memory_card_mode = Some(value.to_string())
        }
        _ if settings.unknown.len() < MAX_UNKNOWN_SETTINGS => {
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

fn differing_keys(global: &DuckStationSettings, game: &DuckStationSettings) -> Vec<String> {
    let global = flattened_settings(global);
    let game = flattened_settings(game);
    game.into_iter()
        .filter_map(|(key, value)| (global.get(&key) != Some(&value)).then_some(key))
        .collect()
}

fn flattened_settings(settings: &DuckStationSettings) -> BTreeMap<String, String> {
    let mut values = settings.unknown.clone();
    for (key, value) in [
        ("renderer", settings.renderer.clone()),
        (
            "internal_resolution_scale",
            settings.internal_resolution_scale.clone(),
        ),
        ("texture_filtering", settings.texture_filtering.clone()),
        ("frame_pacing", settings.frame_pacing.clone()),
        ("frame_limit", settings.frame_limit.clone()),
        ("cpu_execution_mode", settings.cpu_execution_mode.clone()),
        ("audio_backend", settings.audio_backend.clone()),
        ("bios_directory", settings.bios_directory.clone()),
        ("bios_filename", settings.bios_filename.clone()),
        ("memory_card_mode", settings.memory_card_mode.clone()),
    ] {
        if let Some(value) = value {
            values.insert(key.to_string(), value);
        }
    }
    for (key, value) in [
        ("pgxp_enabled", settings.pgxp_enabled),
        ("widescreen_enabled", settings.widescreen_enabled),
        ("vsync_enabled", settings.vsync_enabled),
        ("rewind_enabled", settings.rewind_enabled),
        ("runahead_enabled", settings.runahead_enabled),
        ("cheats_enabled", settings.cheats_enabled),
        (
            "texture_replacements_enabled",
            settings.texture_replacements_enabled,
        ),
        (
            "controller_profile_present",
            settings.controller_profile_present,
        ),
    ] {
        if let Some(value) = value {
            values.insert(key.to_string(), value.to_string());
        }
    }
    values
}

fn inspect_bios(
    profile: &DuckStationProfile,
    settings: &DuckStationSettings,
) -> DuckStationBiosInventory {
    let directory = settings
        .bios_directory
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| profile.bios_path.clone());
    let configured_path = settings
        .bios_filename
        .as_deref()
        .filter(|name| !name.is_empty())
        .map(|name| directory.join(name));
    let state = match configured_path.as_deref() {
        Some(path) if is_regular_file(path) => DuckStationBiosState::PresentUnverified,
        Some(_) => DuckStationBiosState::Missing,
        None => DuckStationBiosState::Unknown,
    };
    DuckStationBiosInventory {
        configured_path,
        state,
    }
}

fn inspect_cheats(path: &Path) -> DuckStationCheatInventory {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, DUCKSTATION_MAX_CHEAT_BYTES, &mut warnings) else {
        return DuckStationCheatInventory {
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
        if line.starts_with("[Cheat") || line.starts_with("Cheat") {
            entries += 1;
        }
        if line.to_ascii_lowercase().starts_with("enabled")
            && line
                .split_once('=')
                .and_then(|(_, value)| parse_bool(value))
                == Some(true)
        {
            enabled_entries += 1;
        }
    }
    DuckStationCheatInventory {
        path: path.to_path_buf(),
        exists,
        entries,
        enabled_entries,
        warnings,
    }
}

fn inspect_patches(directory: &Path, serial: &str) -> DuckStationPatchInventory {
    let mut warnings = Vec::new();
    let mut entries = 0;
    if is_real_directory(directory) {
        let candidates = [
            directory.join(format!("{serial}.ini")),
            directory.join(format!("{serial}.txt")),
            directory.join(format!("{serial}.xml")),
        ];
        for candidate in candidates {
            if is_regular_file(&candidate) {
                let _ = read_text(&candidate, DUCKSTATION_MAX_PATCH_BYTES, &mut warnings);
                entries += 1;
            }
        }
    }
    DuckStationPatchInventory {
        path: directory.to_path_buf(),
        exists: is_real_directory(directory),
        entries,
        warnings,
    }
}

fn inspect_textures(path: &Path, enabled: Option<bool>) -> DuckStationTextureInventory {
    let mut result = DuckStationTextureInventory {
        path: path.to_path_buf(),
        present: is_real_directory(path),
        enabled,
        file_count: 0,
        total_size_bytes: 0,
        complete: true,
        warnings: Vec::new(),
    };
    if !result.present {
        return result;
    }
    let mut pending = VecDeque::from([(path.to_path_buf(), 0usize)]);
    let mut visited = 0usize;
    while let Some((directory, depth)) = pending.pop_front() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                result.complete = false;
                warn(
                    &mut result.warnings,
                    DuckStationWarningKind::UnreadablePath,
                    &directory,
                    format!("directory cannot be read: {error}"),
                );
                continue;
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            if visited >= DUCKSTATION_MAX_DIRECTORY_ENTRIES {
                result.complete = false;
                warn(
                    &mut result.warnings,
                    DuckStationWarningKind::EntryLimitReached,
                    &directory,
                    format!(
                        "texture inspection stopped at {DUCKSTATION_MAX_DIRECTORY_ENTRIES} entries"
                    ),
                );
                return result;
            }
            visited += 1;
            if let Ok(entry) = entry {
                paths.push(entry.path());
            }
        }
        paths.sort();
        for entry in paths {
            let metadata = match fs::symlink_metadata(&entry) {
                Ok(metadata) => metadata,
                Err(error) => {
                    result.complete = false;
                    warn(
                        &mut result.warnings,
                        DuckStationWarningKind::UnreadablePath,
                        &entry,
                        format!("entry cannot be inspected: {error}"),
                    );
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                warn(
                    &mut result.warnings,
                    DuckStationWarningKind::SymlinkSkipped,
                    &entry,
                    "symlink was not followed",
                );
            } else if metadata.is_file() {
                if result.file_count >= DUCKSTATION_MAX_TEXTURE_FILES {
                    result.complete = false;
                    warn(
                        &mut result.warnings,
                        DuckStationWarningKind::FileCountLimitReached,
                        &entry,
                        format!(
                            "texture inspection stopped at {DUCKSTATION_MAX_TEXTURE_FILES} files"
                        ),
                    );
                    return result;
                }
                result.file_count += 1;
                result.total_size_bytes = result.total_size_bytes.saturating_add(metadata.len());
            } else if metadata.is_dir() && depth < DUCKSTATION_MAX_TEXTURE_DEPTH {
                pending.push_back((entry, depth + 1));
            } else if metadata.is_dir() {
                result.complete = false;
                warn(
                    &mut result.warnings,
                    DuckStationWarningKind::DepthLimitReached,
                    &entry,
                    format!("texture inspection stopped at depth {DUCKSTATION_MAX_TEXTURE_DEPTH}"),
                );
            } else {
                warn(
                    &mut result.warnings,
                    DuckStationWarningKind::SpecialFileSkipped,
                    &entry,
                    "non-regular entry was skipped",
                );
            }
        }
    }
    result
}

fn inspect_memory_cards(directory: &Path, serial: &str) -> DuckStationMemoryCardInventory {
    let mut result = DuckStationMemoryCardInventory {
        per_game_card_candidate: None,
        shared_cards: Vec::new(),
        complete: true,
        warnings: Vec::new(),
    };
    if !is_real_directory(directory) {
        return result;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            result.complete = false;
            warn(
                &mut result.warnings,
                DuckStationWarningKind::UnreadablePath,
                directory,
                format!("memory-card directory cannot be read: {error}"),
            );
            return result;
        }
    };
    for (index, entry) in entries.enumerate() {
        if index >= DUCKSTATION_MAX_DIRECTORY_ENTRIES {
            result.complete = false;
            break;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_uppercase();
        if name == format!("{serial}_1.MCD") || name == format!("{serial}_2.MCD") {
            result.per_game_card_candidate.get_or_insert(path);
        } else if name.ends_with(".MCD") {
            result.shared_cards.push(path);
        }
    }
    result.shared_cards.sort();
    result
}

fn inspect_save_states(directory: &Path, serial: &str) -> DuckStationSaveStateInventory {
    let mut result = DuckStationSaveStateInventory {
        candidate_paths: Vec::new(),
        complete: true,
        warnings: Vec::new(),
    };
    if !is_real_directory(directory) {
        return result;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            result.complete = false;
            warn(
                &mut result.warnings,
                DuckStationWarningKind::UnreadablePath,
                directory,
                format!("save-state directory cannot be read: {error}"),
            );
            return result;
        }
    };
    for (index, entry) in entries.enumerate() {
        if index >= DUCKSTATION_MAX_DIRECTORY_ENTRIES {
            result.complete = false;
            break;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        if entry
            .file_name()
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with(serial)
        {
            if result.candidate_paths.len() >= DUCKSTATION_MAX_SAVE_STATE_CANDIDATES {
                result.complete = false;
                break;
            }
            result.candidate_paths.push(path);
        }
    }
    result.candidate_paths.sort();
    result
}

/// Parses an explicitly supplied M3U only as bounded structural membership.
/// The playlist basename and its entries are not identity evidence.
pub fn inspect_duckstation_playlist(path: &Path) -> DuckStationPlaylistInventory {
    inspect_playlist(path)
}

fn inspect_playlist(path: &Path) -> DuckStationPlaylistInventory {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, DUCKSTATION_MAX_PLAYLIST_BYTES, &mut warnings) else {
        return DuckStationPlaylistInventory {
            path: path.to_path_buf(),
            exists,
            entries: Vec::new(),
            complete: false,
            warnings,
        };
    };
    let mut entries = Vec::new();
    let mut complete = true;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if entries.len() >= DUCKSTATION_MAX_PLAYLIST_ENTRIES {
            complete = false;
            warn(
                &mut warnings,
                DuckStationWarningKind::PlaylistEntryLimitReached,
                path,
                format!("playlist parsing stopped at {DUCKSTATION_MAX_PLAYLIST_ENTRIES} entries"),
            );
            break;
        }
        entries.push(line.to_string());
    }
    DuckStationPlaylistInventory {
        path: path.to_path_buf(),
        exists,
        entries,
        complete,
        warnings,
    }
}

fn read_text(
    path: &Path,
    maximum_bytes: u64,
    warnings: &mut Vec<DuckStationWarning>,
) -> Option<String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            warn(
                warnings,
                DuckStationWarningKind::UnreadablePath,
                path,
                format!("file cannot be inspected: {error}"),
            );
            return None;
        }
    };
    if metadata.file_type().is_symlink() {
        warn(
            warnings,
            DuckStationWarningKind::SymlinkSkipped,
            path,
            "symlink was not followed",
        );
        return None;
    }
    if !metadata.is_file() {
        warn(
            warnings,
            DuckStationWarningKind::SpecialFileSkipped,
            path,
            "non-regular file was skipped",
        );
        return None;
    }
    if metadata.len() > maximum_bytes {
        warn(
            warnings,
            DuckStationWarningKind::FileTooLarge,
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
                DuckStationWarningKind::UnreadablePath,
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
            DuckStationWarningKind::UnreadablePath,
            path,
            format!("file cannot be read: {error}"),
        );
        return None;
    }
    if bytes.len() as u64 > maximum_bytes {
        warn(
            warnings,
            DuckStationWarningKind::FileTooLarge,
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
                DuckStationWarningKind::InvalidUtf8,
                path,
                "file is not valid UTF-8; invalid bytes were replaced",
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
    warnings: &mut Vec<DuckStationWarning>,
    kind: DuckStationWarningKind,
    path: &Path,
    detail: impl Into<String>,
) {
    warnings.push(DuckStationWarning {
        kind,
        path: path.to_path_buf(),
        detail: detail.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn roots(temp: &TempDir) -> DuckStationProfileDiscoveryRoots {
        DuckStationProfileDiscoveryRoots {
            home: temp.path().join("home"),
            xdg_config_home: temp.path().join("config"),
            xdg_data_home: temp.path().join("data"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        }
    }

    fn write_global(root: &Path, text: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("settings.ini"), text).unwrap();
    }

    fn eligible(root: PathBuf) -> DuckStationProfile {
        let expected = root.clone();
        let temp_home = root.parent().unwrap().to_path_buf();
        discover_duckstation_profiles(&DuckStationProfileDiscoveryRoots {
            home: temp_home.clone(),
            xdg_config_home: temp_home,
            xdg_data_home: root.parent().unwrap().join("data"),
            explicit_configuration_roots: vec![root],
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        })
        .profiles
        .into_iter()
        .find(|profile| profile.configuration_path == expected && profile.eligible)
        .unwrap()
    }

    fn verified(serial: &str) -> DuckStationGameRequest {
        DuckStationGameRequest {
            verified_ps1_serial: Some(serial.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn native_flatpak_and_portable_profiles_are_discovered_without_execution() {
        let temp = TempDir::new().unwrap();
        let mut roots = roots(&temp);
        write_global(&roots.xdg_config_home.join("duckstation"), "");
        let flatpak = roots
            .home
            .join(".var/app")
            .join(FLATPAK_APP_ID)
            .join("config/duckstation");
        write_global(&flatpak, "");
        let portable = temp.path().join("portable");
        write_global(&portable, "");
        let appimage = temp.path().join("DuckStation.AppImage");
        fs::write(&appimage, b"not executed").unwrap();
        roots.portable_configuration_roots.push(portable);
        roots.explicit_executables.push(appimage.clone());
        roots
            .known_version_outputs
            .insert(appimage.clone(), "DuckStation v0.1-9999".to_string());
        let discovery = discover_duckstation_profiles(&roots);
        assert!(
            discovery
                .profiles
                .iter()
                .any(
                    |profile| profile.installation_type == DuckStationInstallationType::Native
                        && profile.eligible
                )
        );
        assert!(
            discovery
                .profiles
                .iter()
                .any(|profile| profile.installation_type
                    == DuckStationInstallationType::FlatpakUser
                    && profile.eligible)
        );
        assert!(
            discovery
                .profiles
                .iter()
                .any(
                    |profile| profile.installation_type == DuckStationInstallationType::Portable
                        && profile.eligible
                )
        );
        assert_eq!(
            parse_duckstation_version("DuckStation v0.1-9999"),
            Some("0.1".to_string())
        );
        assert_eq!(parse_duckstation_version("unknown"), None);
    }

    #[test]
    fn missing_executable_does_not_block_a_valid_profile() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("duckstation");
        write_global(&root, "");
        let profile = eligible(root);
        assert!(profile.eligible);
        assert!(profile.executable_candidates.is_empty());
    }

    #[test]
    fn configuration_and_per_game_overrides_are_read_only_and_fail_soft() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("duckstation");
        write_global(
            &root,
            "[GPU]\nRenderer=Vulkan\nResolutionScale=2\nPGXPEnable=true\n[Cheats]\nEnableCheats=true\n[BIOS]\nBIOSFilename=scph1001.bin\n",
        );
        fs::create_dir_all(root.join("gamesettings")).unwrap();
        fs::write(
            root.join("gamesettings/SLUS-12345.ini"),
            "[GPU]\nResolutionScale=4\n",
        )
        .unwrap();
        let inspection = inspect_duckstation_game(&eligible(root), &verified("SLUS-12345"));
        assert_eq!(
            inspection.global_config.settings.renderer.as_deref(),
            Some("Vulkan")
        );
        assert_eq!(inspection.global_config.settings.pgxp_enabled, Some(true));
        assert_eq!(
            inspection
                .per_game_config
                .unwrap()
                .settings
                .internal_resolution_scale
                .as_deref(),
            Some("4")
        );
        assert!(
            inspection
                .overridden_setting_keys
                .contains(&"internal_resolution_scale".to_string())
        );

        let malformed = temp.path().join("malformed");
        write_global(&malformed, "[GPU\nnot-a-setting\n");
        assert!(
            inspect_duckstation_game(&eligible(malformed), &DuckStationGameRequest::default())
                .global_config
                .warnings
                .iter()
                .any(|warning| warning.kind == DuckStationWarningKind::MalformedIni)
        );
    }

    #[test]
    fn authoritative_serial_wins_and_emulator_serial_cannot_override_it() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("duckstation");
        write_global(&root, "");
        let inspection = inspect_duckstation_game(
            &eligible(root),
            &DuckStationGameRequest {
                verified_ps1_serial: Some("SLUS-12345".to_string()),
                emulator_serial: Some("SLES-54321".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(inspection.serial.as_deref(), Some("SLUS-12345"));
        assert_eq!(
            inspection.serial_mapping,
            DuckStationSerialMapping::ConflictingEmulatorMetadata
        );
        assert!(inspection.identity_mismatch.is_some());
        let unresolved = inspect_duckstation_game(
            &eligible(temp.path().join("duckstation")),
            &DuckStationGameRequest {
                emulator_serial: Some("SLES-54321".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            unresolved.serial_mapping,
            DuckStationSerialMapping::EmulatorMetadataOnly
        );
    }

    #[test]
    fn redump_tracks_and_logical_chds_remain_separate_per_disc_contexts() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("duckstation");
        write_global(&root, "");
        let inspection = inspect_duckstation_game(
            &eligible(root),
            &DuckStationGameRequest {
                verified_ps1_serial: Some("SLUS-12345".to_string()),
                disc_contexts: vec![
                    DuckStationDiscContext {
                        disc_number: 1,
                        representation: Representation::DiscTrack,
                        claim: ClaimType::ExactTrackMatch,
                    },
                    DuckStationDiscContext {
                        disc_number: 2,
                        representation: Representation::LogicalChd,
                        claim: ClaimType::ExactLogicalDiscMatch,
                    },
                ],
                ..Default::default()
            },
        );
        assert_eq!(inspection.disc_contexts.len(), 2);
        assert_eq!(
            inspection.disc_contexts[0].representation,
            Representation::DiscTrack
        );
        assert_eq!(
            inspection.disc_contexts[1].representation,
            Representation::LogicalChd
        );
    }

    #[test]
    fn playlists_cheats_profiles_and_textures_have_zero_identity_authority() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("duckstation");
        write_global(&root, "[GPU]\nTextureReplacements=true\n");
        fs::create_dir_all(root.join("cheats")).unwrap();
        fs::write(
            root.join("cheats/SLUS-12345.cht"),
            "[Cheat0]\nEnabled=true\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("textures/SLUS-12345/sub")).unwrap();
        fs::write(root.join("textures/SLUS-12345/sub/a.png"), b"texture").unwrap();
        fs::write(root.join("playlist.m3u"), "disc-one.chd\ndisc-two.chd\n").unwrap();
        let profile = eligible(root.clone());
        let unresolved = inspect_duckstation_game(
            &profile,
            &DuckStationGameRequest {
                playlist_path: Some(root.join("playlist.m3u")),
                ..Default::default()
            },
        );
        assert_eq!(
            unresolved.serial_mapping,
            DuckStationSerialMapping::Unavailable
        );
        assert_eq!(unresolved.playlist.unwrap().entries.len(), 2);
        assert!(unresolved.cheats.is_none());
        let verified = inspect_duckstation_game(&profile, &verified("SLUS-12345"));
        assert_eq!(verified.cheats.unwrap().enabled_entries, 1);
        assert_eq!(verified.textures.unwrap().file_count, 1);
    }

    #[test]
    fn patches_bios_memory_cards_and_save_states_are_inspected_conservatively() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("duckstation");
        write_global(&root, "[BIOS]\nBIOSFilename=scph1001.bin\n");
        fs::create_dir_all(root.join("bios")).unwrap();
        fs::write(root.join("bios/scph1001.bin"), b"bios").unwrap();
        fs::create_dir_all(root.join("patches")).unwrap();
        fs::write(root.join("patches/SLUS-12345.ini"), "patch").unwrap();
        fs::create_dir_all(root.join("memcards")).unwrap();
        fs::write(root.join("memcards/SLUS-12345_1.mcd"), b"card").unwrap();
        fs::write(root.join("memcards/shared.mcd"), b"shared").unwrap();
        fs::create_dir_all(root.join("savestates")).unwrap();
        fs::write(root.join("savestates/SLUS-12345_0.sav"), b"state").unwrap();
        let inspection = inspect_duckstation_game(&eligible(root), &verified("SLUS-12345"));
        assert_eq!(
            inspection.health.bios,
            DuckStationBiosState::PresentUnverified
        );
        assert_eq!(inspection.patches.unwrap().entries, 1);
        let cards = inspection.memory_cards.unwrap();
        assert!(cards.per_game_card_candidate.is_some());
        assert_eq!(cards.shared_cards.len(), 1);
        assert_eq!(inspection.save_states.unwrap().candidate_paths.len(), 1);

        let missing = temp.path().join("missing-bios");
        write_global(&missing, "[BIOS]\nBIOSFilename=missing.bin\n");
        assert_eq!(
            inspect_duckstation_game(&eligible(missing), &verified("SLUS-12345"))
                .health
                .bios,
            DuckStationBiosState::Missing
        );
    }

    #[test]
    fn traversal_and_oversized_input_are_bounded_without_writes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("duckstation");
        write_global(&root, "");
        fs::create_dir_all(root.join("cheats")).unwrap();
        fs::write(
            root.join("cheats/SLUS-12345.cht"),
            vec![b'x'; DUCKSTATION_MAX_CHEAT_BYTES as usize + 1],
        )
        .unwrap();
        let mut deep = root.join("textures/SLUS-12345");
        for _ in 0..DUCKSTATION_MAX_TEXTURE_DEPTH + 2 {
            deep = deep.join("nested");
            fs::create_dir_all(&deep).unwrap();
        }
        let inspection = inspect_duckstation_game(&eligible(root), &verified("SLUS-12345"));
        assert_eq!(inspection.cheats.unwrap().entries, 0);
        assert!(!inspection.textures.unwrap().complete);
    }
}
