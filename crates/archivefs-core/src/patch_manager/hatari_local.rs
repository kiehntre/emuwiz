//! Bounded, read-only inspection of local Hatari configuration.
//!
//! Hatari consumes preservation identity; it is never an identity authority.
//! In particular, configuration paths, disk names, machine selection, TOS file
//! names, cartridge names, and snapshots are context only.  This module never
//! launches Hatari, mounts an image, opens a MIDI device, follows a symlink, or
//! writes a file.

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::disk_format::{DiskFormatContext, DiskFormatEvidence, inspect_disk_format};
use crate::safe_read::TrustedRoots;

pub const HATARI_MAX_PROFILES: usize = 16;
pub const HATARI_MAX_CONFIG_BYTES: u64 = 256 * 1024;
pub const HATARI_MAX_TOS_BYTES: u64 = 2 * 1024 * 1024;
pub const HATARI_MAX_SAVE_STATE_CANDIDATES: usize = 64;
pub const HATARI_MAX_METADATA_BYTES: usize = 4 * 1024;
const FLATPAK_APP_ID: &str = "org.tuxfamily.Hatari";
const MAX_INI_LINES: usize = 8_192;
const MAX_INI_LINE_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HatariInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HatariMachineModel {
    St,
    Ste,
    Tt,
    Falcon,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HatariTosHealth {
    NotConfigured,
    Missing,
    Unreadable,
    PresentUnverified,
    Verified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HatariPathState {
    #[default]
    NotConfigured,
    Missing,
    Unreadable,
    Present,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HatariFloppyRepresentation {
    St,
    Msa,
    Stx,
    Ipf,
    Dim,
    Scp,
    Hfe,
    Zip,
    Other,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HatariStorageMechanism {
    Gemdos,
    Acsi,
    Scsi,
    IdeMaster,
    IdeSlave,
    Cartridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HatariIdentityState {
    Verified,
    Unresolved,
    Conflict,
    NonAtari,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HatariIdentityAssociation {
    CoreVerifiedAtari,
    Unresolved,
    Conflict,
    NonAtari,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HatariInspectionWarning {
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariExecutable {
    pub path: PathBuf,
    pub installation_type: HatariInstallationType,
    /// Parsed only from text a caller obtained under its own authority.
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariProfile {
    pub profile_id: String,
    pub installation_type: HatariInstallationType,
    pub config_path: PathBuf,
    pub provenance: &'static str,
    pub eligible: bool,
    pub executable_candidates: Vec<HatariExecutable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariProfileDiscovery {
    pub profiles: Vec<HatariProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub explicit_config_roots: Vec<PathBuf>,
    pub portable_config_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
}

impl HatariProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, HatariDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(HatariDiscoveryError::HomeUnavailable)?;
        let xdg_config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let xdg_data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Ok(Self {
            home,
            xdg_config_home,
            xdg_data_home,
            explicit_config_roots: Vec::new(),
            portable_config_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: env::var_os("APPIMAGE")
                .map(PathBuf::from)
                .and_then(|p| p.parent().map(Path::to_path_buf)),
        })
    }
}

#[derive(Debug)]
pub enum HatariDiscoveryError {
    HomeUnavailable,
}
impl std::fmt::Display for HatariDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set")
    }
}
impl std::error::Error for HatariDiscoveryError {}

/// An external trusted hash/reference catalogue.  The adapter deliberately has
/// no embedded TOS filename table: names have zero verification authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariTosReference {
    pub sha256: String,
    pub version: Option<String>,
    pub region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariTosRom {
    pub path: Option<PathBuf>,
    pub state: HatariPathState,
    pub health: HatariTosHealth,
    pub sha256: Option<String>,
    pub version: Option<String>,
    pub region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HatariFloppy {
    pub drive: char,
    pub path: Option<PathBuf>,
    pub state: HatariPathState,
    /// Extension-derived representation context; never identity evidence.
    pub representation: HatariFloppyRepresentation,
    pub structural_format: Option<DiskFormatEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HatariStorage {
    pub mechanism: HatariStorageMechanism,
    pub path: Option<PathBuf>,
    pub state: HatariPathState,
    pub read_only: Option<bool>,
    pub boot_preferred: Option<bool>,
    pub drive: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HatariMachineSettings {
    pub model: HatariMachineModel,
    pub cpu_family: Option<String>,
    pub cpu_frequency_mhz: Option<String>,
    pub fpu: Option<String>,
    pub mmu: Option<String>,
    pub memory_setting: Option<String>,
    pub monitor: Option<String>,
    pub dsp: Option<String>,
    pub blitter: Option<bool>,
    pub rtc: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HatariInputSettings {
    pub joystick_ports: Vec<Option<String>>,
    pub keyboard_emulation: Option<bool>,
    pub mouse_mode: Option<String>,
    pub autofire: Vec<Option<String>>,
    pub midi_enabled: Option<bool>,
    pub midi_input: Option<PathBuf>,
    pub midi_output: Option<PathBuf>,
    pub midi_input_state: HatariPathState,
    pub midi_output_state: HatariPathState,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HatariAudioSettings {
    pub enabled: Option<bool>,
    pub sample_frequency: Option<String>,
    pub buffer: Option<String>,
    pub ym_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HatariVideoSettings {
    pub fullscreen: Option<bool>,
    pub monitor: Option<String>,
    pub borders: Option<bool>,
    pub vsync: Option<bool>,
    pub frame_skip: Option<String>,
    pub max_width: Option<String>,
    pub max_height: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HatariConfig {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub malformed: bool,
    pub machine: HatariMachineSettings,
    pub floppies: Vec<HatariFloppy>,
    pub storage: Vec<HatariStorage>,
    pub cartridge: HatariStorage,
    pub input: HatariInputSettings,
    pub audio: HatariAudioSettings,
    pub video: HatariVideoSettings,
    pub tos_path: Option<PathBuf>,
    pub save_state_path: Option<PathBuf>,
    pub warnings: Vec<HatariInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariSaveStateInventory {
    pub configured_path: Option<PathBuf>,
    pub configured_state: HatariPathState,
    pub candidates: Vec<PathBuf>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariSelectedGameRequest {
    pub canonical_platform: Option<String>,
    pub identity_state: HatariIdentityState,
    pub verified_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariSelectedGame {
    pub canonical_platform: Option<String>,
    pub identity: HatariIdentityAssociation,
    pub verified_title: Option<String>,
    pub per_game_profile_available: bool,
    pub save_states: HatariSaveStateInventory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariHealth {
    pub detected: bool,
    pub config_readable: bool,
    pub tos: HatariTosRom,
    pub machine: HatariMachineSettings,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HatariGameInspection {
    pub config: HatariConfig,
    pub health: HatariHealth,
    pub selected_game: HatariSelectedGame,
}

#[derive(Debug, Clone)]
struct Candidate {
    kind: HatariInstallationType,
    config_path: PathBuf,
    provenance: &'static str,
}

/// Discovers exact documented XDG, legacy, Flatpak, AppImage-adjacent and
/// caller-supplied paths.  It never scans a home directory or launches a binary.
pub fn discover_hatari_profiles(roots: &HatariProfileDiscoveryRoots) -> HatariProfileDiscovery {
    let mut candidates = vec![
        Candidate {
            kind: HatariInstallationType::Native,
            config_path: roots.xdg_config_home.join("hatari/hatari.cfg"),
            provenance: "XDG Hatari configuration",
        },
        Candidate {
            kind: HatariInstallationType::Native,
            config_path: roots.home.join(".hatari/hatari.cfg"),
            provenance: "legacy Hatari configuration",
        },
        Candidate {
            kind: HatariInstallationType::FlatpakUser,
            config_path: roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("config/hatari/hatari.cfg"),
            provenance: "Flatpak Hatari configuration",
        },
    ];
    candidates.extend(
        roots
            .portable_config_roots
            .iter()
            .cloned()
            .map(|p| Candidate {
                kind: HatariInstallationType::Portable,
                config_path: config_file(p),
                provenance: "caller-supplied Hatari portable/AppImage configuration",
            }),
    );
    candidates.extend(
        roots
            .explicit_config_roots
            .iter()
            .cloned()
            .map(|p| Candidate {
                kind: HatariInstallationType::Explicit,
                config_path: config_file(p),
                provenance: "explicit Hatari configuration",
            }),
    );
    if let Some(dir) = &roots.appimage_directory {
        candidates.push(Candidate {
            kind: HatariInstallationType::Portable,
            config_path: dir.join("hatari.cfg"),
            provenance: "APPIMAGE-adjacent Hatari configuration",
        });
    }
    candidates.sort_by(|a, b| a.config_path.cmp(&b.config_path));
    candidates.dedup_by(|a, b| a.config_path == b.config_path);
    let executables = discover_executables(roots);
    let profiles = candidates
        .into_iter()
        .filter(|c| {
            is_regular(c.config_path.as_path()) || c.kind == HatariInstallationType::Explicit
        })
        .take(HATARI_MAX_PROFILES)
        .map(|c| HatariProfile {
            profile_id: format!("hatari:{}", c.config_path.display()),
            installation_type: c.kind,
            eligible: is_regular(&c.config_path),
            config_path: c.config_path,
            provenance: c.provenance,
            executable_candidates: executables.clone(),
        })
        .collect();
    HatariProfileDiscovery {
        profiles,
        complete: true,
    }
}

/// Parses caller-supplied `hatari --version` text without executing Hatari.
pub fn parse_hatari_version(output: &str) -> Option<String> {
    let text = output.trim();
    let index = text.to_ascii_lowercase().find("hatari")?;
    let value: String = text[index + 6..]
        .trim_start_matches(|c: char| c == 'v' || c.is_whitespace() || c == ':')
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (value.split('.').count() >= 2 && value.len() <= 32).then_some(value)
}

/// Reads a selected profile.  `trusted_tos` is optional external evidence;
/// without it every present ROM is deliberately `PresentUnverified`.
pub fn inspect_hatari_game(
    profile: &HatariProfile,
    request: &HatariSelectedGameRequest,
    trusted_tos: &[HatariTosReference],
) -> HatariGameInspection {
    let config = inspect_config(&profile.config_path);
    let tos = inspect_tos(config.tos_path.clone(), trusted_tos);
    let save_states = inspect_save_states(config.save_state_path.as_deref());
    let selected_game = HatariSelectedGame {
        canonical_platform: request.canonical_platform.clone(),
        identity: associate_identity(request),
        verified_title: request.verified_title.clone().filter(|_| {
            request.identity_state == HatariIdentityState::Verified
                && request
                    .canonical_platform
                    .as_deref()
                    .is_some_and(is_atari_platform)
        }),
        per_game_profile_available: false,
        save_states,
    };
    let mut warnings = config
        .warnings
        .iter()
        .map(|w| w.detail.clone())
        .collect::<Vec<_>>();
    if config.malformed {
        warnings
            .push("Hatari configuration was partially parsed after malformed lines".to_string());
    }
    let health = HatariHealth {
        detected: profile.eligible || !profile.executable_candidates.is_empty(),
        config_readable: config.readable,
        tos,
        machine: config.machine.clone(),
        warnings,
    };
    HatariGameInspection {
        config,
        health,
        selected_game,
    }
}

fn config_file(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e == "cfg") {
        path
    } else {
        path.join("hatari.cfg")
    }
}

fn discover_executables(roots: &HatariProfileDiscoveryRoots) -> Vec<HatariExecutable> {
    let mut paths = roots.explicit_executables.clone();
    if let Some(dir) = &roots.appimage_directory {
        paths.extend([
            dir.join("Hatari.AppImage"),
            dir.join("hatari.AppImage"),
            dir.join("hatari"),
        ]);
    }
    if let Some(value) = env::var_os("PATH") {
        for dir in env::split_paths(&value).take(128) {
            paths.push(dir.join("hatari"));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|p| is_regular(p))
        .map(|path| HatariExecutable {
            installation_type: if roots.explicit_executables.contains(&path) {
                HatariInstallationType::Explicit
            } else if roots
                .appimage_directory
                .as_ref()
                .is_some_and(|d| path.starts_with(d))
            {
                HatariInstallationType::Portable
            } else {
                HatariInstallationType::Native
            },
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|v| parse_hatari_version(v)),
            path,
        })
        .collect()
}

fn inspect_config(path: &Path) -> HatariConfig {
    let (ini, warnings) = read_ini(path);
    let exists = fs::symlink_metadata(path).is_ok();
    let readable = read_bounded(path, HATARI_MAX_CONFIG_BYTES).is_some();
    let malformed = warnings.iter().any(|w| w.detail.contains("malformed"));
    let get = |section: &str, names: &[&str]| value(&ini, section, names);
    let model = get("System", &["nModelType"])
        .and_then(machine_model)
        .unwrap_or(HatariMachineModel::Unknown);
    let floppies = ['A', 'B']
        .into_iter()
        .map(|drive| {
            let key = if drive == 'A' {
                "szDiskAFileName"
            } else {
                "szDiskBFileName"
            };
            let path = config_value_path(path, &ini, "Floppy", &[key]);
            let state = path_state(path.as_deref());
            let structural_format = path
                .as_deref()
                .filter(|p| {
                    matches!(
                        representation(p),
                        HatariFloppyRepresentation::St | HatariFloppyRepresentation::Stx
                    ) && path_state(Some(p)) == HatariPathState::Present
                })
                .map(inspect_floppy_format);
            HatariFloppy {
                drive,
                representation: path
                    .as_deref()
                    .map(representation)
                    .unwrap_or(HatariFloppyRepresentation::Unknown),
                path,
                state,
                structural_format,
            }
        })
        .collect();
    let storage = [
        (
            HatariStorageMechanism::Gemdos,
            "szHardDiskDirectory",
            "bUseHardDiskDirectory",
        ),
        (
            HatariStorageMechanism::Acsi,
            "szHardDiskImage",
            "bUseHardDiskImage",
        ),
        (
            HatariStorageMechanism::Scsi,
            "szScsiImage",
            "bUseScsiHardDiskImage",
        ),
        (
            HatariStorageMechanism::IdeMaster,
            "szIdeMasterHardDiskImage",
            "bUseIdeMasterHardDiskImage",
        ),
        (
            HatariStorageMechanism::IdeSlave,
            "szIdeSlaveHardDiskImage",
            "bUseIdeSlaveHardDiskImage",
        ),
    ]
    .into_iter()
    .map(|(mechanism, key, enabled)| {
        let configured = get("HardDisk", &[enabled])
            .and_then(parse_bool)
            .unwrap_or_else(|| get("HardDisk", &[key]).is_some());
        let value_path = configured
            .then(|| config_value_path(path, &ini, "HardDisk", &[key]))
            .flatten();
        HatariStorage {
            mechanism,
            state: if configured {
                path_state(value_path.as_deref())
            } else {
                HatariPathState::NotConfigured
            },
            path: value_path,
            read_only: get("HardDisk", &["nWriteProtection", "bWriteProtection"])
                .and_then(parse_write_protection),
            boot_preferred: get("HardDisk", &["bBootFromHardDisk"]).and_then(parse_bool),
            drive: (mechanism == HatariStorageMechanism::Gemdos)
                .then(|| get("HardDisk", &["nGemdosDrive"]).map(str::to_string))
                .flatten(),
        }
    })
    .collect();
    let cartridge_path = config_value_path(
        path,
        &ini,
        "ROM",
        &["szCartridgeImageFileName", "szCartridgeFileName"],
    );
    let midi_input = config_value_path(path, &ini, "Midi", &["sMidiInFileName"]);
    let midi_output = config_value_path(path, &ini, "Midi", &["sMidiOutFileName"]);
    HatariConfig {
        path: path.to_path_buf(),
        exists,
        readable,
        malformed,
        machine: HatariMachineSettings {
            model,
            cpu_family: get("System", &["nCpuLevel"]).and_then(cpu_family),
            cpu_frequency_mhz: get("System", &["nCpuFreq"]).map(str::to_string),
            fpu: get("System", &["nFPUType"]).map(str::to_string),
            mmu: get("System", &["nMMUType"]).map(str::to_string),
            memory_setting: get("Memory", &["nMemorySize"]).map(str::to_string),
            monitor: get("Screen", &["nMonitorType"]).and_then(monitor_name),
            dsp: get("System", &["bDSP"]).map(str::to_string),
            blitter: get("System", &["bBlitter"]).and_then(parse_bool),
            rtc: get("System", &["bRealTimeClock"]).and_then(parse_bool),
        },
        floppies,
        storage,
        cartridge: HatariStorage {
            mechanism: HatariStorageMechanism::Cartridge,
            state: path_state(cartridge_path.as_deref()),
            path: cartridge_path,
            read_only: None,
            boot_preferred: None,
            drive: None,
        },
        input: HatariInputSettings {
            joystick_ports: (0..2)
                .map(|n| get(&format!("Joystick{n}"), &["nJoystickMode"]).map(str::to_string))
                .collect(),
            keyboard_emulation: get("Keyboard", &["bDisableKeyRepeat"])
                .and_then(parse_bool)
                .map(|v| !v),
            mouse_mode: get("Mouse", &["nMouseMode"]).map(str::to_string),
            autofire: (0..2)
                .map(|n| {
                    get(&format!("Joystick{n}"), &["nJoyId", "bEnableAutoFire"]).map(str::to_string)
                })
                .collect(),
            midi_enabled: get("Midi", &["bEnableMidi"]).and_then(parse_bool),
            midi_input_state: path_state(midi_input.as_deref()),
            midi_output_state: path_state(midi_output.as_deref()),
            midi_input,
            midi_output,
        },
        audio: HatariAudioSettings {
            enabled: get("Sound", &["bEnableSound"]).and_then(parse_bool),
            sample_frequency: get("Sound", &["nPlaybackFreq"]).map(str::to_string),
            buffer: get("Sound", &["nSdlAudioBufferSize"]).map(str::to_string),
            ym_model: get("Sound", &["YmVolumeMixing", "nYmVolumeMixing"]).map(str::to_string),
        },
        video: HatariVideoSettings {
            fullscreen: get("Screen", &["bFullScreen"]).and_then(parse_bool),
            monitor: get("Screen", &["nMonitorType"]).and_then(monitor_name),
            borders: get("Screen", &["bAllowOverscan"]).and_then(parse_bool),
            vsync: get("Screen", &["bVSync"]).and_then(parse_bool),
            frame_skip: get("Screen", &["nFrameSkips"]).map(str::to_string),
            max_width: get("Screen", &["nMaxWidth"]).map(str::to_string),
            max_height: get("Screen", &["nMaxHeight"]).map(str::to_string),
        },
        tos_path: config_value_path(path, &ini, "ROM", &["szTosImageFileName"]),
        save_state_path: config_value_path(path, &ini, "Memory", &["szMemoryCaptureFileName"]),
        warnings,
    }
}

fn inspect_tos(path: Option<PathBuf>, references: &[HatariTosReference]) -> HatariTosRom {
    let state = path_state(path.as_deref());
    if path.is_none() {
        return HatariTosRom {
            path,
            state,
            health: HatariTosHealth::NotConfigured,
            sha256: None,
            version: None,
            region: None,
        };
    }
    if state == HatariPathState::Missing {
        return HatariTosRom {
            path,
            state,
            health: HatariTosHealth::Missing,
            sha256: None,
            version: None,
            region: None,
        };
    }
    if state != HatariPathState::Present {
        return HatariTosRom {
            path,
            state,
            health: HatariTosHealth::Unreadable,
            sha256: None,
            version: None,
            region: None,
        };
    }
    let Some(digest) = path.as_deref().and_then(sha256_bounded) else {
        return HatariTosRom {
            path,
            state,
            health: HatariTosHealth::PresentUnverified,
            sha256: None,
            version: None,
            region: None,
        };
    };
    let reference = references
        .iter()
        .find(|item| item.sha256.eq_ignore_ascii_case(&digest));
    HatariTosRom {
        path,
        state,
        sha256: Some(digest),
        health: if reference.is_some() {
            HatariTosHealth::Verified
        } else {
            HatariTosHealth::PresentUnverified
        },
        version: reference.and_then(|r| r.version.clone()),
        region: reference.and_then(|r| r.region.clone()),
    }
}

fn inspect_save_states(configured: Option<&Path>) -> HatariSaveStateInventory {
    let configured_state = path_state(configured);
    let mut candidates = Vec::new();
    if let Some(parent) = configured.and_then(Path::parent).filter(|p| is_real_dir(p))
        && let Ok(entries) = fs::read_dir(parent)
    {
        for entry in entries.flatten().take(HATARI_MAX_SAVE_STATE_CANDIDATES) {
            let path = entry.path();
            if is_regular(&path) {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    HatariSaveStateInventory {
        configured_path: configured.map(Path::to_path_buf),
        configured_state,
        complete: candidates.len() < HATARI_MAX_SAVE_STATE_CANDIDATES,
        candidates,
    }
}

fn inspect_floppy_format(path: &Path) -> DiskFormatEvidence {
    inspect_disk_format(
        path,
        &TrustedRoots::from_paths(path.parent()),
        DiskFormatContext::default(),
        None,
    )
}

fn associate_identity(request: &HatariSelectedGameRequest) -> HatariIdentityAssociation {
    match request.identity_state {
        HatariIdentityState::Verified
            if request
                .canonical_platform
                .as_deref()
                .is_some_and(is_atari_platform) =>
        {
            HatariIdentityAssociation::CoreVerifiedAtari
        }
        HatariIdentityState::Verified => HatariIdentityAssociation::NonAtari,
        HatariIdentityState::Unresolved => HatariIdentityAssociation::Unresolved,
        HatariIdentityState::Conflict => HatariIdentityAssociation::Conflict,
        HatariIdentityState::NonAtari => HatariIdentityAssociation::NonAtari,
    }
}
fn is_atari_platform(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "atarist"
            | "atari st"
            | "atari ste"
            | "atari stf"
            | "atari stfm"
            | "mega st"
            | "mega ste"
            | "atari tt"
            | "atari falcon"
    )
}
fn representation(path: &Path) -> HatariFloppyRepresentation {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "st" => HatariFloppyRepresentation::St,
        "msa" => HatariFloppyRepresentation::Msa,
        "stx" => HatariFloppyRepresentation::Stx,
        "ipf" => HatariFloppyRepresentation::Ipf,
        "dim" => HatariFloppyRepresentation::Dim,
        "scp" => HatariFloppyRepresentation::Scp,
        "hfe" => HatariFloppyRepresentation::Hfe,
        "zip" => HatariFloppyRepresentation::Zip,
        "" => HatariFloppyRepresentation::Unknown,
        _ => HatariFloppyRepresentation::Other,
    }
}
fn machine_model(value: &str) -> Option<HatariMachineModel> {
    match value.trim().to_ascii_lowercase().as_str() {
        "0" | "st" => Some(HatariMachineModel::St),
        "1" | "ste" => Some(HatariMachineModel::Ste),
        "2" | "tt" => Some(HatariMachineModel::Tt),
        "3" | "falcon" => Some(HatariMachineModel::Falcon),
        _ => None,
    }
}
fn cpu_family(value: &str) -> Option<String> {
    match value.trim() {
        "0" => Some("68000".to_string()),
        "1" => Some("68010".to_string()),
        "2" => Some("68020".to_string()),
        "3" => Some("68030".to_string()),
        "4" => Some("68040".to_string()),
        "5" => Some("68060".to_string()),
        _ => None,
    }
}
fn monitor_name(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "0" | "rgb" => Some("RGB".to_string()),
        "1" | "mono" | "monochrome" => Some("Monochrome".to_string()),
        "2" | "vga" => Some("VGA".to_string()),
        _ => None,
    }
}
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}
fn parse_write_protection(value: &str) -> Option<bool> {
    parse_bool(value).or_else(|| value.trim().parse::<u8>().ok().map(|v| v != 0))
}
fn value<'a>(
    ini: &'a BTreeMap<(String, String), String>,
    section: &str,
    names: &[&str],
) -> Option<&'a str> {
    names.iter().find_map(|name| {
        ini.get(&(section.to_ascii_lowercase(), name.to_ascii_lowercase()))
            .map(String::as_str)
    })
}
fn config_value_path(
    config: &Path,
    ini: &BTreeMap<(String, String), String>,
    section: &str,
    names: &[&str],
) -> Option<PathBuf> {
    let raw = value(ini, section, names)?.trim().trim_matches('"');
    (!raw.is_empty() && raw.len() <= HATARI_MAX_METADATA_BYTES).then(|| {
        let p = PathBuf::from(raw);
        if p.is_absolute() {
            p
        } else {
            config.parent().unwrap_or_else(|| Path::new(".")).join(p)
        }
    })
}
fn read_ini(
    path: &Path,
) -> (
    BTreeMap<(String, String), String>,
    Vec<HatariInspectionWarning>,
) {
    let mut warnings = Vec::new();
    let Some(bytes) = read_bounded(path, HATARI_MAX_CONFIG_BYTES) else {
        return (BTreeMap::new(), warnings);
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        warnings.push(HatariInspectionWarning {
            path: path.to_path_buf(),
            detail: "configuration is not valid UTF-8".to_string(),
        });
        return (BTreeMap::new(), warnings);
    };
    let mut map = BTreeMap::new();
    let mut section = String::new();
    for (index, line) in text.lines().take(MAX_INI_LINES).enumerate() {
        if line.len() > MAX_INI_LINE_BYTES {
            warnings.push(HatariInspectionWarning {
                path: path.to_path_buf(),
                detail: format!("line {} exceeds bounded length", index + 1),
            });
            continue;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(HatariInspectionWarning {
                path: path.to_path_buf(),
                detail: format!("malformed configuration line {}", index + 1),
            });
            continue;
        };
        if section.is_empty() || key.trim().is_empty() {
            warnings.push(HatariInspectionWarning {
                path: path.to_path_buf(),
                detail: format!("malformed configuration line {}", index + 1),
            });
            continue;
        }
        map.insert(
            (section.clone(), key.trim().to_ascii_lowercase()),
            value
                .trim()
                .chars()
                .take(HATARI_MAX_METADATA_BYTES)
                .collect(),
        );
    }
    if text.lines().count() > MAX_INI_LINES {
        warnings.push(HatariInspectionWarning {
            path: path.to_path_buf(),
            detail: "configuration line limit reached".to_string(),
        });
    }
    (map, warnings)
}
fn read_bounded(path: &Path, maximum: u64) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > maximum {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).ok()?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(maximum + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= maximum).then_some(bytes)
}
fn sha256_bounded(path: &Path) -> Option<String> {
    let bytes = read_bounded(path, HATARI_MAX_TOS_BYTES)?;
    Some(
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}
fn is_regular(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}
fn path_state(path: Option<&Path>) -> HatariPathState {
    match path {
        None => HatariPathState::NotConfigured,
        Some(p) if is_regular(p) || is_real_dir(p) => HatariPathState::Present,
        Some(p) if fs::symlink_metadata(p).is_ok() => HatariPathState::Unreadable,
        Some(_) => HatariPathState::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn roots(tree: &TempDir) -> HatariProfileDiscoveryRoots {
        HatariProfileDiscoveryRoots {
            home: tree.path().join("home"),
            xdg_config_home: tree.path().join("config"),
            xdg_data_home: tree.path().join("data"),
            explicit_config_roots: Vec::new(),
            portable_config_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        }
    }
    fn write(path: &Path, contents: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    fn profile(config: PathBuf) -> HatariProfile {
        HatariProfile {
            profile_id: "test".to_string(),
            installation_type: HatariInstallationType::Explicit,
            config_path: config,
            provenance: "test",
            eligible: true,
            executable_candidates: Vec::new(),
        }
    }
    fn request(platform: &str, state: HatariIdentityState) -> HatariSelectedGameRequest {
        HatariSelectedGameRequest {
            canonical_platform: Some(platform.to_string()),
            identity_state: state,
            verified_title: Some("Verified title".to_string()),
        }
    }
    #[test]
    fn discovers_native_flatpak_and_explicit_without_scanning() {
        let t = TempDir::new().unwrap();
        let mut r = roots(&t);
        let native = r.xdg_config_home.join("hatari/hatari.cfg");
        let flatpak = r
            .home
            .join(".var/app/org.tuxfamily.Hatari/config/hatari/hatari.cfg");
        let explicit = t.path().join("custom/hatari.cfg");
        write(&native, b"[System]\nnModelType=0\n");
        write(&flatpak, b"[System]\nnModelType=1\n");
        write(&explicit, b"[System]\nnModelType=2\n");
        r.explicit_config_roots.push(explicit);
        let found = discover_hatari_profiles(&r);
        assert_eq!(found.profiles.len(), 3);
        assert!(
            found
                .profiles
                .iter()
                .any(|p| p.installation_type == HatariInstallationType::FlatpakUser)
        );
    }
    #[test]
    fn portable_and_custom_executable_version_are_safe_metadata() {
        let t = TempDir::new().unwrap();
        let mut r = roots(&t);
        let exe = t.path().join("Hatari.AppImage");
        write(&exe, b"not executed");
        r.explicit_executables.push(exe.clone());
        r.known_version_outputs
            .insert(exe, "Hatari v2.5.1".to_string());
        r.portable_config_roots.push(t.path().join("portable"));
        write(
            &t.path().join("portable/hatari.cfg"),
            b"[System]\nnModelType=3\n",
        );
        let p = discover_hatari_profiles(&r);
        assert_eq!(
            p.profiles[0].executable_candidates[0].version.as_deref(),
            Some("2.5.1")
        );
        assert_eq!(parse_hatari_version("strange"), None);
    }
    #[test]
    fn malformed_config_is_partial_and_fail_soft() {
        let t = TempDir::new().unwrap();
        let cfg = t.path().join("hatari.cfg");
        write(&cfg, b"bad\n[System]\nnModelType=1\n");
        let inspected = inspect_hatari_game(
            &profile(cfg),
            &request("AtariST", HatariIdentityState::Verified),
            &[],
        );
        assert!(inspected.config.malformed);
        assert_eq!(inspected.config.machine.model, HatariMachineModel::Ste);
    }
    #[test]
    fn tos_filename_never_verifies_but_trusted_hash_does() {
        let t = TempDir::new().unwrap();
        let rom = t.path().join("tos104.img");
        write(&rom, b"synthetic tos");
        let cfg = t.path().join("hatari.cfg");
        write(
            &cfg,
            format!("[ROM]\nszTosImageFileName={}\n", rom.display()).as_bytes(),
        );
        let p = profile(cfg);
        let unverified =
            inspect_hatari_game(&p, &request("AtariST", HatariIdentityState::Verified), &[]);
        assert_eq!(
            unverified.health.tos.health,
            HatariTosHealth::PresentUnverified
        );
        let hash = unverified.health.tos.sha256.clone().unwrap();
        let verified = inspect_hatari_game(
            &p,
            &request("AtariST", HatariIdentityState::Verified),
            &[HatariTosReference {
                sha256: hash,
                version: Some("1.04".to_string()),
                region: Some("US".to_string()),
            }],
        );
        assert_eq!(verified.health.tos.health, HatariTosHealth::Verified);
        assert_eq!(verified.health.tos.version.as_deref(), Some("1.04"));
    }
    #[test]
    fn missing_tos_and_machine_models_remain_distinct() {
        let t = TempDir::new().unwrap();
        for (raw, expected) in [
            ("0", HatariMachineModel::St),
            ("1", HatariMachineModel::Ste),
            ("2", HatariMachineModel::Tt),
            ("3", HatariMachineModel::Falcon),
        ] {
            let cfg = t.path().join(format!("{raw}.cfg"));
            write(
                &cfg,
                format!("[System]\nnModelType={raw}\n[ROM]\nszTosImageFileName=missing.img\n")
                    .as_bytes(),
            );
            let i = inspect_hatari_game(
                &profile(cfg),
                &request("AtariST", HatariIdentityState::Verified),
                &[],
            );
            assert_eq!(i.config.machine.model, expected);
            assert_eq!(i.health.tos.health, HatariTosHealth::Missing);
        }
    }
    #[test]
    fn storage_input_audio_video_and_representations_are_context_only() {
        let t = TempDir::new().unwrap();
        let gem = t.path().join("gem");
        fs::create_dir(&gem).unwrap();
        let stx = t.path().join("Disk A.stx");
        write(&stx, b"RSY\0\0\0\0\0\0\0\0\0\0\0\0\0");
        let cfg = t.path().join("hatari.cfg");
        write(&cfg,format!("[System]\nnModelType=3\nnCpuLevel=3\n[Floppy]\nszDiskAFileName={}\nszDiskBFileName=Disk B.msa\n[HardDisk]\nbUseHardDiskDirectory=true\nszHardDiskDirectory={}\nbUseHardDiskImage=true\nszHardDiskImage=hdd.img\nbBootFromHardDisk=true\n[ROM]\nszCartridgeImageFileName=cart.img\n[Joystick0]\nnJoystickMode=1\n[Midi]\nbEnableMidi=true\nsMidiInFileName=/dev/does-not-exist\n[Sound]\nbEnableSound=true\nnPlaybackFreq=44100\n[Screen]\nbFullScreen=false\nnMonitorType=2\n",stx.display(),gem.display()).as_bytes());
        let i = inspect_hatari_game(
            &profile(cfg),
            &request("AtariST", HatariIdentityState::Verified),
            &[],
        );
        assert_eq!(
            i.config.floppies[0].representation,
            HatariFloppyRepresentation::Stx
        );
        assert_eq!(
            i.config.floppies[1].representation,
            HatariFloppyRepresentation::Msa
        );
        assert_eq!(i.config.storage[0].state, HatariPathState::Present);
        assert_eq!(i.config.storage[1].state, HatariPathState::Missing);
        assert_eq!(i.config.machine.model, HatariMachineModel::Falcon);
        assert_eq!(i.config.machine.cpu_family.as_deref(), Some("68030"));
        assert_eq!(i.config.video.monitor.as_deref(), Some("VGA"));
        assert_eq!(i.config.input.midi_enabled, Some(true));
    }
    #[test]
    fn core_identity_is_the_only_association_authority() {
        let t = TempDir::new().unwrap();
        let cfg = t.path().join("hatari.cfg");
        write(
            &cfg,
            b"[System]\nnModelType=1\n[Floppy]\nszDiskAFileName=Game (Atari ST).st\n",
        );
        let p = profile(cfg);
        let verified =
            inspect_hatari_game(&p, &request("AtariST", HatariIdentityState::Verified), &[]);
        assert_eq!(
            verified.selected_game.identity,
            HatariIdentityAssociation::CoreVerifiedAtari
        );
        assert_eq!(verified.config.machine.model, HatariMachineModel::Ste);
        assert_eq!(
            inspect_hatari_game(
                &p,
                &request("AtariST", HatariIdentityState::Unresolved),
                &[]
            )
            .selected_game
            .identity,
            HatariIdentityAssociation::Unresolved
        );
        assert_eq!(
            inspect_hatari_game(&p, &request("Wii", HatariIdentityState::Verified), &[])
                .selected_game
                .identity,
            HatariIdentityAssociation::NonAtari
        );
        assert_eq!(
            inspect_hatari_game(&p, &request("AtariST", HatariIdentityState::Conflict), &[])
                .selected_game
                .identity,
            HatariIdentityAssociation::Conflict
        );
    }
    #[test]
    fn bounded_oversized_config_is_not_read() {
        let t = TempDir::new().unwrap();
        let cfg = t.path().join("hatari.cfg");
        write(&cfg, &vec![b'x'; HATARI_MAX_CONFIG_BYTES as usize + 1]);
        let i = inspect_hatari_game(
            &profile(cfg),
            &request("AtariST", HatariIdentityState::Verified),
            &[],
        );
        assert!(!i.config.readable);
    }

    #[test]
    fn floppy_representations_and_multidisk_stay_per_disk_context() {
        let t = TempDir::new().unwrap();
        let cfg = t.path().join("hatari.cfg");
        write(
            &cfg,
            b"[Floppy]\nszDiskAFileName=Title Disk A.ipf\nszDiskBFileName=Title Disk B.msa\n",
        );
        let i = inspect_hatari_game(
            &profile(cfg),
            &request("AtariST", HatariIdentityState::Verified),
            &[],
        );
        assert_eq!(i.config.floppies.len(), 2);
        assert_eq!(i.config.floppies[0].drive, 'A');
        assert_eq!(
            i.config.floppies[0].representation,
            HatariFloppyRepresentation::Ipf
        );
        assert_eq!(i.config.floppies[1].drive, 'B');
        assert_eq!(
            i.config.floppies[1].representation,
            HatariFloppyRepresentation::Msa
        );
        assert!(
            i.config
                .floppies
                .iter()
                .all(|disk| disk.structural_format.is_none())
        );
    }

    #[test]
    fn raw_st_uses_the_shared_bounded_structural_inspector() {
        let t = TempDir::new().unwrap();
        let disk = t.path().join("disk.st");
        let mut bytes = vec![0_u8; 720 * 1024];
        bytes[0x0b..0x0d].copy_from_slice(&512_u16.to_le_bytes());
        bytes[0x0d] = 2;
        bytes[0x0e..0x10].copy_from_slice(&1_u16.to_le_bytes());
        bytes[0x10] = 2;
        bytes[0x11..0x13].copy_from_slice(&112_u16.to_le_bytes());
        bytes[0x13..0x15].copy_from_slice(&1440_u16.to_le_bytes());
        bytes[0x16..0x18].copy_from_slice(&5_u16.to_le_bytes());
        bytes[0x18..0x1a].copy_from_slice(&9_u16.to_le_bytes());
        bytes[0x1a..0x1c].copy_from_slice(&2_u16.to_le_bytes());
        write(&disk, &bytes);
        let cfg = t.path().join("hatari.cfg");
        write(
            &cfg,
            format!("[Floppy]\nszDiskAFileName={}\n", disk.display()).as_bytes(),
        );
        let i = inspect_hatari_game(
            &profile(cfg),
            &request("AtariST", HatariIdentityState::Verified),
            &[],
        );
        assert_eq!(
            i.config.floppies[0].representation,
            HatariFloppyRepresentation::St
        );
        assert!(
            i.config.floppies[0]
                .structural_format
                .as_ref()
                .unwrap()
                .is_recognised()
        );
    }

    #[test]
    fn configured_snapshot_is_only_a_candidate_and_is_bounded() {
        let t = TempDir::new().unwrap();
        let states = t.path().join("states");
        fs::create_dir(&states).unwrap();
        for number in 0..3 {
            write(&states.join(format!("{number}.sav")), b"state");
        }
        let configured = states.join("selected.sav");
        write(&configured, b"configured state");
        let cfg = t.path().join("hatari.cfg");
        write(
            &cfg,
            format!(
                "[Memory]\nszMemoryCaptureFileName={}\n",
                configured.display()
            )
            .as_bytes(),
        );
        let i = inspect_hatari_game(
            &profile(cfg),
            &request("AtariST", HatariIdentityState::Verified),
            &[],
        );
        assert_eq!(
            i.selected_game.save_states.configured_state,
            HatariPathState::Present
        );
        assert_eq!(i.selected_game.save_states.candidates.len(), 4);
        assert!(!i.selected_game.per_game_profile_available);
    }

    #[test]
    fn missing_executable_and_missing_explicit_config_are_neutral() {
        let t = TempDir::new().unwrap();
        let mut r = roots(&t);
        r.explicit_config_roots.push(t.path().join("missing"));
        r.explicit_executables.push(t.path().join("no-hatari"));
        let discovered = discover_hatari_profiles(&r);
        assert_eq!(discovered.profiles.len(), 1);
        assert!(!discovered.profiles[0].eligible);
        assert!(discovered.profiles[0].executable_candidates.is_empty());
    }
}
