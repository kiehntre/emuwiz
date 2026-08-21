//! Bounded, read-only inspection of local Amiberry and FS-UAE installations.
//!
//! This adapter consumes already-verified Amiga/WHDLoad context.  Emulator
//! configuration, filenames, and directory names are display context only;
//! they cannot create or replace preservation identity.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::amiga_disk::{
    AmigaDisk, FileSystem, HdfSlaveDiscovery, PartitionTraversalLimits, discover_whdload_slaves,
    inspect_hdf,
};
use crate::identity_source::whdload::SlaveArtifact;

use super::destination_safety::{
    DestinationRootState, DestinationSafetyFailureReason, validate_destination_root,
};

pub const AMIGA_MAX_PROFILES: usize = 24;
pub const AMIGA_MAX_CONFIG_BYTES: u64 = 256 * 1024;
pub const AMIGA_MAX_PROFILE_FILES: usize = 128;
pub const AMIGA_MAX_SAVE_STATE_CANDIDATES: usize = 128;
const MAX_CONFIG_LINES: usize = 8_192;
const MAX_CONFIG_LINE_BYTES: usize = 8 * 1024;
const MAX_UNKNOWN_SETTINGS: usize = 256;
const AMIBERRY_FLATPAK_ID: &str = "com.blitterstudio.amiberry";
const FS_UAE_FLATPAK_ID: &str = "net.fsuae.FS-UAE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaEmulatorKind {
    Amiberry,
    FsUae,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaProfileScope {
    User,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaWarningKind {
    UnsafePath,
    UnreadablePath,
    SymlinkSkipped,
    SpecialFileSkipped,
    FileTooLarge,
    InvalidUtf8,
    MalformedConfig,
    LineCountLimitReached,
    LineTooLong,
    EntryLimitReached,
    UnsupportedFilesystem,
    HdfRejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaWarning {
    pub kind: AmigaWarningKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaExecutable {
    pub path: PathBuf,
    pub installation_type: AmigaInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaProfile {
    pub profile_id: String,
    pub emulator: AmigaEmulatorKind,
    pub installation_type: AmigaInstallationType,
    pub scope: AmigaProfileScope,
    pub configuration_root: PathBuf,
    pub global_config_path: Option<PathBuf>,
    pub profile_paths: Vec<PathBuf>,
    pub executable_candidates: Vec<AmigaExecutable>,
    pub eligible: bool,
    pub warnings: Vec<AmigaWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaProfileDiscovery {
    pub profiles: Vec<AmigaProfile>,
    pub warnings: Vec<AmigaWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub explicit_configuration_roots: Vec<(AmigaEmulatorKind, PathBuf)>,
    pub portable_configuration_roots: Vec<(AmigaEmulatorKind, PathBuf)>,
    pub explicit_executables: Vec<(AmigaEmulatorKind, PathBuf)>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}

impl AmigaProfileDiscoveryRoots {
    pub fn from_environment() -> Option<Self> {
        let home = env::var_os("HOME").map(PathBuf::from)?;
        let xdg_config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let xdg_data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Some(Self {
            home,
            xdg_config_home,
            xdg_data_home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmigaMachineProfile {
    pub machine_model: Option<String>,
    pub chipset: Option<String>,
    pub cpu: Option<String>,
    pub fpu: Option<String>,
    pub chip_ram: Option<String>,
    pub fast_ram: Option<String>,
    pub z3_ram: Option<String>,
    pub video_standard: Option<String>,
    pub kickstart_path: Option<PathBuf>,
    pub floppy_mounts: Vec<PathBuf>,
    pub hdf_mounts: Vec<PathBuf>,
    pub controller_present: bool,
    pub audio: Option<String>,
    pub graphics: Option<String>,
    pub whdload_boot: Option<bool>,
    pub unknown: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaConfig {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub machine: AmigaMachineProfile,
    pub warnings: Vec<AmigaWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaKickstartState {
    NotConfigured,
    Missing,
    Unreadable,
    PresentUnverified,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaKickstart {
    pub path: Option<PathBuf>,
    pub state: AmigaKickstartState,
    pub hash_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaSlaveCandidate {
    pub provenance_path: Option<String>,
    pub artifact: SlaveArtifact,
    pub from_hdf: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaHdfPartitionInspection {
    pub filesystem: FileSystem,
    pub discovered: Option<HdfSlaveDiscovery>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaHdfInspection {
    pub path: PathBuf,
    pub disk: Option<AmigaDisk>,
    pub partitions: Vec<AmigaHdfPartitionInspection>,
    pub warnings: Vec<AmigaWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmigaGameRequest {
    /// Only core may supply this already-verified identity.  No emulator path
    /// or slave filename can populate it.
    pub verified_amiga_identity: Option<String>,
    /// Optional emulator-observed context, retained without authority.
    pub emulator_metadata: Option<String>,
    pub bare_slaves: Vec<SlaveArtifact>,
    pub hdf_inspections: Vec<AmigaHdfInspection>,
    /// An externally selected profile can be inspected, but its filename is
    /// never used to map it to identity.
    pub selected_profile_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaGameMapping {
    VerifiedIdentity,
    EmulatorMetadataOnly,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaHealth {
    pub detected: bool,
    pub config_readable: bool,
    pub kickstart: AmigaKickstart,
    pub whdload_support_present: bool,
    pub controller_configured: bool,
    pub game_mapping: AmigaGameMapping,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaGameInspection {
    pub game_mapping: AmigaGameMapping,
    pub verified_identity: Option<String>,
    pub emulator_metadata: Option<String>,
    pub config: AmigaConfig,
    pub per_game_config: Option<AmigaConfig>,
    /// Kept all together; this adapter has no best-slave heuristic.
    pub slave_candidates: Vec<AmigaSlaveCandidate>,
    pub hdf_inspections: Vec<AmigaHdfInspection>,
    pub save_state_candidates: Vec<PathBuf>,
    pub save_state_complete: bool,
    pub health: AmigaHealth,
}

#[derive(Clone)]
struct Candidate {
    emulator: AmigaEmulatorKind,
    installation_type: AmigaInstallationType,
    scope: AmigaProfileScope,
    root: PathBuf,
}

/// Finds only documented XDG/Flatpak locations and exact caller-provided
/// roots. It never recursively searches the home directory or launches a binary.
pub fn discover_amiga_profiles(roots: &AmigaProfileDiscoveryRoots) -> AmigaProfileDiscovery {
    let mut candidates = vec![
        Candidate {
            emulator: AmigaEmulatorKind::Amiberry,
            installation_type: AmigaInstallationType::Native,
            scope: AmigaProfileScope::User,
            root: roots.xdg_config_home.join("amiberry"),
        },
        Candidate {
            emulator: AmigaEmulatorKind::Amiberry,
            installation_type: AmigaInstallationType::Native,
            scope: AmigaProfileScope::User,
            root: roots.xdg_data_home.join("amiberry"),
        },
        Candidate {
            emulator: AmigaEmulatorKind::FsUae,
            installation_type: AmigaInstallationType::Native,
            scope: AmigaProfileScope::User,
            root: roots.xdg_config_home.join("fs-uae"),
        },
        Candidate {
            emulator: AmigaEmulatorKind::FsUae,
            installation_type: AmigaInstallationType::Native,
            scope: AmigaProfileScope::User,
            root: roots.xdg_data_home.join("fs-uae"),
        },
        Candidate {
            emulator: AmigaEmulatorKind::Amiberry,
            installation_type: AmigaInstallationType::FlatpakUser,
            scope: AmigaProfileScope::User,
            root: roots
                .home
                .join(".var/app")
                .join(AMIBERRY_FLATPAK_ID)
                .join("config/amiberry"),
        },
        Candidate {
            emulator: AmigaEmulatorKind::FsUae,
            installation_type: AmigaInstallationType::FlatpakUser,
            scope: AmigaProfileScope::User,
            root: roots
                .home
                .join(".var/app")
                .join(FS_UAE_FLATPAK_ID)
                .join("config/fs-uae"),
        },
    ];
    candidates.extend(roots.portable_configuration_roots.iter().cloned().map(
        |(emulator, root)| Candidate {
            emulator,
            installation_type: AmigaInstallationType::Portable,
            scope: AmigaProfileScope::Explicit,
            root,
        },
    ));
    candidates.extend(roots.explicit_configuration_roots.iter().cloned().map(
        |(emulator, root)| Candidate {
            emulator,
            installation_type: AmigaInstallationType::Explicit,
            scope: AmigaProfileScope::Explicit,
            root,
        },
    ));
    candidates
        .sort_by(|a, b| (kind_key(a.emulator), &a.root).cmp(&(kind_key(b.emulator), &b.root)));
    candidates.dedup_by(|a, b| a.emulator == b.emulator && a.root == b.root);
    let executables = discover_executables(roots);
    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for candidate in candidates {
        if profiles.len() >= AMIGA_MAX_PROFILES {
            warnings.push(warning(
                AmigaWarningKind::EntryLimitReached,
                &candidate.root,
                "profile limit reached",
            ));
            break;
        }
        if !candidate.root.exists() && candidate.scope == AmigaProfileScope::User {
            continue;
        }
        profiles.push(validate_profile(candidate, &executables));
    }
    AmigaProfileDiscovery { profiles, warnings }
}

/// Parses caller-supplied version text; discovery itself never executes an emulator.
pub fn parse_amiga_version(output: &str) -> Option<String> {
    let value = output.trim();
    let token = value.split_whitespace().find(|token| {
        token
            .trim_start_matches('v')
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })?;
    let version: String = token
        .trim_start_matches('v')
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (version.contains('.') && version.len() <= 32).then_some(version)
}

/// Uses the existing bounded HDF/RDB and OFS/FFS APIs. PFS/SFS/MuFS stay
/// detection-only; no alternate traversal implementation exists here.
pub fn inspect_amiga_hdf(
    path: &Path,
    limits: &PartitionTraversalLimits,
    cancel: Option<&AtomicBool>,
) -> AmigaHdfInspection {
    let mut warnings = Vec::new();
    if !safe_regular(path, &mut warnings) {
        return AmigaHdfInspection {
            path: path.into(),
            disk: None,
            partitions: Vec::new(),
            warnings,
        };
    }
    let Ok(disk) = inspect_hdf(path) else {
        warnings.push(warning(
            AmigaWarningKind::HdfRejected,
            path,
            "HDF/RDB inspection rejected image",
        ));
        return AmigaHdfInspection {
            path: path.into(),
            disk: None,
            partitions: Vec::new(),
            warnings,
        };
    };
    let mut partitions = Vec::new();
    for partition in &disk.rdb.partitions {
        match &partition.filesystem {
            FileSystem::Dos(_) => match discover_whdload_slaves(&disk, partition, limits, cancel) {
                Ok(discovered) => partitions.push(AmigaHdfPartitionInspection {
                    filesystem: partition.filesystem.clone(),
                    discovered: Some(discovered),
                    warning: None,
                }),
                Err(error) => partitions.push(AmigaHdfPartitionInspection {
                    filesystem: partition.filesystem.clone(),
                    discovered: None,
                    warning: Some(error.to_string()),
                }),
            },
            other => partitions.push(AmigaHdfPartitionInspection {
                filesystem: other.clone(),
                discovered: None,
                warning: Some("filesystem is detection-only; traversal was not attempted".into()),
            }),
        }
    }
    AmigaHdfInspection {
        path: path.into(),
        disk: Some(disk),
        partitions,
        warnings,
    }
}

/// Inspects a selected profile and supplied preservation context. It never
/// generates evidence, chooses a slave, or associates a profile by its name.
pub fn inspect_amiga_whdload_game(
    profile: &AmigaProfile,
    request: &AmigaGameRequest,
) -> AmigaGameInspection {
    let config = profile.global_config_path.as_ref().map_or_else(
        || absent_config(profile.configuration_root.join("<no-global-config>")),
        |path| inspect_config(path),
    );
    let per_game_config = request
        .selected_profile_path
        .as_ref()
        .filter(|path| profile.profile_paths.contains(path))
        .map(|path| inspect_config(path));
    let kickstart = inspect_kickstart(config.machine.kickstart_path.as_deref());
    let mut slaves: Vec<AmigaSlaveCandidate> = request
        .bare_slaves
        .iter()
        .cloned()
        .map(|artifact| AmigaSlaveCandidate {
            provenance_path: None,
            artifact,
            from_hdf: false,
        })
        .collect();
    for hdf in &request.hdf_inspections {
        for partition in &hdf.partitions {
            if let Some(discovered) = &partition.discovered {
                slaves.extend(discovered.candidates.iter().cloned().map(|candidate| {
                    AmigaSlaveCandidate {
                        provenance_path: Some(candidate.in_image_path),
                        artifact: candidate.artifact,
                        from_hdf: true,
                    }
                }));
            }
        }
    }
    let mut unique = BTreeSet::new();
    slaves.retain(|candidate| {
        unique.insert((
            candidate.artifact.hashes.sha256.clone(),
            candidate.provenance_path.clone(),
        ))
    });
    let game_mapping = if request
        .verified_amiga_identity
        .as_deref()
        .is_some_and(nonempty)
    {
        AmigaGameMapping::VerifiedIdentity
    } else if request.emulator_metadata.as_deref().is_some_and(nonempty) {
        AmigaGameMapping::EmulatorMetadataOnly
    } else {
        AmigaGameMapping::Unavailable
    };
    let (save_state_candidates, save_state_complete) =
        inspect_save_states(&profile.configuration_root);
    let mut health_warnings: Vec<String> = profile
        .warnings
        .iter()
        .chain(config.warnings.iter())
        .map(|warning| warning.detail.clone())
        .collect();
    if slaves.len() > 1 {
        health_warnings
            .push("multiple valid WHDLoad slaves retained; no slave was selected".into());
    }
    let whdload_support_present = [
        profile.configuration_root.join("WHDLoad"),
        profile.configuration_root.join("whdload"),
    ]
    .into_iter()
    .any(|path| real_dir(&path));
    let health = AmigaHealth {
        detected: profile.eligible || !profile.executable_candidates.is_empty(),
        config_readable: config.readable,
        kickstart,
        whdload_support_present,
        controller_configured: config.machine.controller_present,
        game_mapping,
        warnings: health_warnings,
    };
    AmigaGameInspection {
        game_mapping,
        verified_identity: request.verified_amiga_identity.clone(),
        emulator_metadata: request.emulator_metadata.clone(),
        config,
        per_game_config,
        slave_candidates: slaves,
        hdf_inspections: request.hdf_inspections.clone(),
        save_state_candidates,
        save_state_complete,
        health,
    }
}

fn validate_profile(candidate: Candidate, executables: &[AmigaExecutable]) -> AmigaProfile {
    let mut warnings = Vec::new();
    let eligible = if !candidate.root.is_absolute() {
        warnings.push(warning(
            AmigaWarningKind::UnsafePath,
            &candidate.root,
            "configuration path is not absolute",
        ));
        false
    } else if candidate.root.parent().is_none() {
        warnings.push(warning(
            AmigaWarningKind::UnsafePath,
            &candidate.root,
            "filesystem root cannot be an emulator profile",
        ));
        false
    } else {
        match validate_destination_root(&candidate.root) {
            Ok(value) if value.state() == DestinationRootState::Absent => {
                warnings.push(warning(
                    AmigaWarningKind::UnreadablePath,
                    &candidate.root,
                    "configuration root does not exist",
                ));
                false
            }
            Ok(_) => true,
            Err(error) => {
                let kind = match error.reason {
                    DestinationSafetyFailureReason::InspectionFailed => {
                        AmigaWarningKind::UnreadablePath
                    }
                    _ => AmigaWarningKind::UnsafePath,
                };
                warnings.push(warning(
                    kind,
                    &candidate.root,
                    "configuration root was rejected",
                ));
                false
            }
        }
    };
    let global_config_path = global_config(&candidate.root, candidate.emulator);
    let profile_paths = find_profile_paths(&candidate.root, candidate.emulator, &mut warnings);
    AmigaProfile {
        profile_id: format!("{:?}:{}", candidate.emulator, candidate.root.display()),
        emulator: candidate.emulator,
        installation_type: candidate.installation_type,
        scope: candidate.scope,
        configuration_root: candidate.root,
        global_config_path,
        profile_paths,
        executable_candidates: executables
            .iter()
            .filter(|executable| executable_kind(executable) == candidate.emulator)
            .cloned()
            .collect(),
        eligible,
        warnings,
    }
}

fn kind_key(kind: AmigaEmulatorKind) -> u8 {
    match kind {
        AmigaEmulatorKind::Amiberry => 0,
        AmigaEmulatorKind::FsUae => 1,
    }
}
fn executable_kind(executable: &AmigaExecutable) -> AmigaEmulatorKind {
    executable
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().contains("amiberry"))
        .then_some(AmigaEmulatorKind::Amiberry)
        .unwrap_or(AmigaEmulatorKind::FsUae)
}
fn global_config(root: &Path, kind: AmigaEmulatorKind) -> Option<PathBuf> {
    let names: &[&str] = match kind {
        AmigaEmulatorKind::Amiberry => &["amiberry.conf", "amiberry.uae"],
        AmigaEmulatorKind::FsUae => &["Launcher.ini", "fs-uae.conf"],
    };
    names
        .iter()
        .map(|name| root.join(name))
        .find(|path| regular(path))
}
fn find_profile_paths(
    root: &Path,
    kind: AmigaEmulatorKind,
    warnings: &mut Vec<AmigaWarning>,
) -> Vec<PathBuf> {
    let directory = match kind {
        AmigaEmulatorKind::Amiberry => root.join("configs"),
        AmigaEmulatorKind::FsUae => root.join("Configurations"),
    };
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut output = Vec::new();
    for entry in entries.flatten() {
        if output.len() >= AMIGA_MAX_PROFILE_FILES {
            warnings.push(warning(
                AmigaWarningKind::EntryLimitReached,
                &directory,
                "per-game profile limit reached",
            ));
            break;
        }
        let path = entry.path();
        if regular(&path) {
            output.push(path);
        }
    }
    output.sort();
    output
}
fn discover_executables(roots: &AmigaProfileDiscoveryRoots) -> Vec<AmigaExecutable> {
    let mut values = roots.explicit_executables.clone();
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path).take(128) {
            values.extend([
                (AmigaEmulatorKind::Amiberry, directory.join("amiberry")),
                (AmigaEmulatorKind::FsUae, directory.join("fs-uae")),
            ]);
        }
    }
    values.sort_by(|a, b| a.1.cmp(&b.1));
    values.dedup_by(|a, b| a.1 == b.1);
    values
        .into_iter()
        .filter(|(_, path)| regular(path))
        .map(|(_kind, path)| AmigaExecutable {
            installation_type: if roots
                .explicit_executables
                .iter()
                .any(|(_, candidate)| candidate == &path)
            {
                AmigaInstallationType::Explicit
            } else {
                AmigaInstallationType::Native
            },
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|value| parse_amiga_version(value)),
            path,
        })
        .collect()
}

fn inspect_config(path: &Path) -> AmigaConfig {
    let exists = path.exists();
    let mut warnings = Vec::new();
    let Some(text) = read_text(path, &mut warnings) else {
        return AmigaConfig {
            path: path.into(),
            exists,
            readable: false,
            machine: AmigaMachineProfile::default(),
            warnings,
        };
    };
    let mut machine = parse_config(&text, path, &mut warnings);
    if let Some(kickstart) = &machine.kickstart_path
        && kickstart.is_relative()
    {
        machine.kickstart_path = path.parent().map(|parent| parent.join(kickstart));
    }
    AmigaConfig {
        path: path.into(),
        exists,
        readable: true,
        machine,
        warnings,
    }
}
fn absent_config(path: PathBuf) -> AmigaConfig {
    AmigaConfig {
        path,
        exists: false,
        readable: false,
        machine: AmigaMachineProfile::default(),
        warnings: Vec::new(),
    }
}
fn parse_config(text: &str, path: &Path, warnings: &mut Vec<AmigaWarning>) -> AmigaMachineProfile {
    let mut result = AmigaMachineProfile::default();
    for (index, raw) in text.lines().enumerate() {
        if index >= MAX_CONFIG_LINES {
            warnings.push(warning(
                AmigaWarningKind::LineCountLimitReached,
                path,
                "config line limit reached",
            ));
            break;
        }
        if raw.len() > MAX_CONFIG_LINE_BYTES {
            warnings.push(warning(
                AmigaWarningKind::LineTooLong,
                path,
                "config line too long",
            ));
            continue;
        }
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with(';')
            || line.starts_with('[')
        {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(warning(
                AmigaWarningKind::MalformedConfig,
                path,
                "config line has no '='",
            ));
            continue;
        };
        apply_setting(&mut result, key.trim(), value.trim());
    }
    result
}
fn apply_setting(machine: &mut AmigaMachineProfile, key: &str, value: &str) {
    let lower = key.to_ascii_lowercase();
    let path = || (!value.is_empty()).then(|| PathBuf::from(value));
    match lower.as_str() {
        "amiga_model" | "model" | "config_hardware_model" => {
            machine.machine_model = Some(value.into())
        }
        "chipset" | "chipset_type" => machine.chipset = Some(value.into()),
        "cpu_model" | "cpu" => machine.cpu = Some(value.into()),
        "fpu_model" | "fpu" => machine.fpu = Some(value.into()),
        "chipmem_size" | "chip_ram" => machine.chip_ram = Some(value.into()),
        "fastmem_size" | "fast_ram" => machine.fast_ram = Some(value.into()),
        "z3mem_size" | "z3_ram" => machine.z3_ram = Some(value.into()),
        "ntsc" | "video_standard" => machine.video_standard = Some(value.into()),
        "kickstart_rom_file" | "kickstart_rom" | "kickstart_file" => {
            machine.kickstart_path = path()
        }
        "audio" | "audio_backend" => machine.audio = Some(value.into()),
        "graphics" | "gfx_api" | "gfx_filter" => machine.graphics = Some(value.into()),
        "whdload" | "whdload_boot" => machine.whdload_boot = boolean(value),
        _ if lower.contains("floppy") || lower.contains("diskimage") => {
            if let Some(path) = path() {
                machine.floppy_mounts.push(path);
            }
        }
        _ if lower.contains("hardfile")
            || lower.contains("hdf")
            || lower.contains("filesystem") =>
        {
            if let Some(path) = path() {
                machine.hdf_mounts.push(path);
            }
        }
        _ if lower.contains("joyport")
            || lower.contains("joystick")
            || lower.contains("controller")
            || lower.contains("input") =>
        {
            machine.controller_present = true
        }
        _ if machine.unknown.len() < MAX_UNKNOWN_SETTINGS => {
            machine.unknown.insert(key.into(), value.into());
        }
        _ => {}
    }
}
fn inspect_kickstart(path: Option<&Path>) -> AmigaKickstart {
    let Some(path) = path else {
        return AmigaKickstart {
            path: None,
            state: AmigaKickstartState::NotConfigured,
            hash_verified: false,
        };
    };
    let state = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => AmigaKickstartState::Missing,
        Err(_) => AmigaKickstartState::Unreadable,
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            AmigaKickstartState::Unreadable
        }
        Ok(_) => File::open(path)
            .map(|_| AmigaKickstartState::PresentUnverified)
            .unwrap_or(AmigaKickstartState::Unreadable),
    };
    AmigaKickstart {
        path: Some(path.into()),
        state,
        hash_verified: false,
    }
}
fn inspect_save_states(root: &Path) -> (Vec<PathBuf>, bool) {
    let mut output = Vec::new();
    for directory in [
        root.join("savestates"),
        root.join("save-states"),
        root.join("states"),
    ] {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if output.len() >= AMIGA_MAX_SAVE_STATE_CANDIDATES {
                return (output, false);
            }
            let path = entry.path();
            if regular(&path) {
                output.push(path);
            }
        }
    }
    output.sort();
    output.dedup();
    (output, true)
}
fn read_text(path: &Path, warnings: &mut Vec<AmigaWarning>) -> Option<String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(warning(
                AmigaWarningKind::UnreadablePath,
                path,
                &error.to_string(),
            ));
            return None;
        }
    };
    if metadata.file_type().is_symlink() {
        warnings.push(warning(
            AmigaWarningKind::SymlinkSkipped,
            path,
            "symlink skipped",
        ));
        return None;
    }
    if !metadata.is_file() {
        warnings.push(warning(
            AmigaWarningKind::SpecialFileSkipped,
            path,
            "not a regular file",
        ));
        return None;
    }
    if metadata.len() > AMIGA_MAX_CONFIG_BYTES {
        warnings.push(warning(
            AmigaWarningKind::FileTooLarge,
            path,
            "config exceeds byte limit",
        ));
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) => {
            warnings.push(warning(
                AmigaWarningKind::UnreadablePath,
                path,
                &error.to_string(),
            ));
            return None;
        }
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file.read_to_end(&mut bytes).is_err() {
        warnings.push(warning(
            AmigaWarningKind::UnreadablePath,
            path,
            "could not read config",
        ));
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(value) => Some(value),
        Err(_) => {
            warnings.push(warning(
                AmigaWarningKind::InvalidUtf8,
                path,
                "config is not UTF-8",
            ));
            None
        }
    }
}
fn safe_regular(path: &Path, warnings: &mut Vec<AmigaWarning>) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            warnings.push(warning(
                AmigaWarningKind::SymlinkSkipped,
                path,
                "symlink skipped",
            ));
            false
        }
        Ok(metadata) if !metadata.is_file() => {
            warnings.push(warning(
                AmigaWarningKind::SpecialFileSkipped,
                path,
                "not a regular file",
            ));
            false
        }
        Ok(_) => true,
        Err(error) => {
            warnings.push(warning(
                AmigaWarningKind::UnreadablePath,
                path,
                &error.to_string(),
            ));
            false
        }
    }
}
fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}
fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}
fn nonempty(value: &str) -> bool {
    !value.trim().is_empty()
}
fn boolean(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}
fn warning(kind: AmigaWarningKind, path: &Path, detail: impl Into<String>) -> AmigaWarning {
    AmigaWarning {
        kind,
        path: path.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::whdload::{ParsedWHDLoadSlave, SlaveHashes};
    use tempfile::tempdir;

    fn roots(temp: &tempfile::TempDir) -> AmigaProfileDiscoveryRoots {
        let home = temp.path().join("home");
        AmigaProfileDiscoveryRoots {
            xdg_config_home: home.join(".config"),
            xdg_data_home: home.join(".local/share"),
            home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
        }
    }
    fn slave(name: &str, hash: &str) -> SlaveArtifact {
        SlaveArtifact {
            path: PathBuf::from(name),
            name: name.into(),
            size_bytes: 4,
            parsed: ParsedWHDLoadSlave {
                runtime_version: 17,
                struct_size: 52,
                flags: 0,
                base_mem_size: 512 * 1024,
                exec_install: 0,
                game_loader: 0,
                current_dir: None,
                dont_cache: None,
                key_debug: None,
                key_exit: None,
                exp_mem: None,
                name: Some("Display title only".into()),
                copyright: None,
                info: None,
                kick_name: None,
                kick_size: None,
                kick_crc: None,
                config: Some("trainer=0".into()),
                extension_bytes: Vec::new(),
            },
            hashes: SlaveHashes {
                sha1: hash.into(),
                sha256: format!("sha256-{hash}"),
            },
        }
    }
    fn profile(roots: &AmigaProfileDiscoveryRoots, emulator: AmigaEmulatorKind) -> AmigaProfile {
        discover_amiga_profiles(roots)
            .profiles
            .into_iter()
            .find(|profile| profile.emulator == emulator && profile.eligible)
            .unwrap()
    }

    #[test]
    fn discovers_native_flatpak_and_portable_profiles() {
        let temp = tempdir().unwrap();
        let mut roots = roots(&temp);
        fs::create_dir_all(roots.xdg_config_home.join("amiberry")).unwrap();
        fs::write(
            roots.xdg_config_home.join("amiberry/amiberry.conf"),
            "amiga_model=A1200",
        )
        .unwrap();
        fs::create_dir_all(
            roots
                .home
                .join(".var/app")
                .join(FS_UAE_FLATPAK_ID)
                .join("config/fs-uae"),
        )
        .unwrap();
        fs::write(
            roots
                .home
                .join(".var/app")
                .join(FS_UAE_FLATPAK_ID)
                .join("config/fs-uae/Launcher.ini"),
            "amiga_model = CD32",
        )
        .unwrap();
        let portable = temp.path().join("portable");
        fs::create_dir_all(&portable).unwrap();
        fs::write(portable.join("amiberry.conf"), "").unwrap();
        roots
            .portable_configuration_roots
            .push((AmigaEmulatorKind::Amiberry, portable));
        let found = discover_amiga_profiles(&roots);
        assert!(
            found
                .profiles
                .iter()
                .any(|profile| profile.emulator == AmigaEmulatorKind::Amiberry
                    && profile.installation_type == AmigaInstallationType::Native)
        );
        assert!(
            found
                .profiles
                .iter()
                .any(|profile| profile.emulator == AmigaEmulatorKind::FsUae
                    && profile.installation_type == AmigaInstallationType::FlatpakUser)
        );
        assert!(
            found
                .profiles
                .iter()
                .any(|profile| profile.installation_type == AmigaInstallationType::Portable)
        );
    }

    #[test]
    fn reads_machine_kickstart_and_controller_without_filename_authority() {
        let temp = tempdir().unwrap();
        let roots = roots(&temp);
        let root = roots.xdg_config_home.join("amiberry");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("amiberry.conf"), "amiga_model=CD32\nchipset=AGA\nkickstart_rom_file=kick31.rom\njoyport0=mouse\nwhdload_boot=true\n").unwrap();
        fs::write(root.join("kick31.rom"), b"not a verified ROM").unwrap();
        let inspection = inspect_amiga_whdload_game(
            &profile(&roots, AmigaEmulatorKind::Amiberry),
            &AmigaGameRequest::default(),
        );
        assert_eq!(
            inspection.config.machine.machine_model.as_deref(),
            Some("CD32")
        );
        assert!(inspection.config.machine.controller_present);
        assert_eq!(
            inspection.health.kickstart.state,
            AmigaKickstartState::PresentUnverified
        );
        assert!(!inspection.health.kickstart.hash_verified);
        fs::remove_file(root.join("kick31.rom")).unwrap();
        let missing = inspect_amiga_whdload_game(
            &profile(&roots, AmigaEmulatorKind::Amiberry),
            &AmigaGameRequest::default(),
        );
        assert_eq!(missing.health.kickstart.state, AmigaKickstartState::Missing);
    }

    #[test]
    fn malformed_and_oversized_config_fail_soft() {
        let temp = tempdir().unwrap();
        let roots = roots(&temp);
        let root = roots.xdg_config_home.join("fs-uae");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Launcher.ini"), "broken line").unwrap();
        let bad = inspect_amiga_whdload_game(
            &profile(&roots, AmigaEmulatorKind::FsUae),
            &AmigaGameRequest::default(),
        );
        assert!(
            bad.config
                .warnings
                .iter()
                .any(|warning| warning.kind == AmigaWarningKind::MalformedConfig)
        );
        fs::write(
            root.join("Launcher.ini"),
            vec![b'x'; AMIGA_MAX_CONFIG_BYTES as usize + 1],
        )
        .unwrap();
        let large = inspect_amiga_whdload_game(
            &profile(&roots, AmigaEmulatorKind::FsUae),
            &AmigaGameRequest::default(),
        );
        assert!(
            large
                .config
                .warnings
                .iter()
                .any(|warning| warning.kind == AmigaWarningKind::FileTooLarge)
        );
    }

    #[test]
    fn preservation_identity_wins_and_multiple_slaves_are_not_selected() {
        let temp = tempdir().unwrap();
        let roots = roots(&temp);
        let root = roots.xdg_config_home.join("amiberry");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("amiberry.conf"), "").unwrap();
        let request = AmigaGameRequest {
            verified_amiga_identity: Some("exact preservation identity".into()),
            emulator_metadata: Some("conflicting emulator title".into()),
            bare_slaves: vec![
                slave("name-is-not-authority.slave", "one"),
                slave("other.slave", "two"),
            ],
            ..Default::default()
        };
        let inspection =
            inspect_amiga_whdload_game(&profile(&roots, AmigaEmulatorKind::Amiberry), &request);
        assert_eq!(inspection.game_mapping, AmigaGameMapping::VerifiedIdentity);
        assert_eq!(inspection.slave_candidates.len(), 2);
        assert!(
            inspection
                .health
                .warnings
                .iter()
                .any(|warning| warning.contains("no slave was selected"))
        );
    }

    #[test]
    fn emulator_metadata_and_profile_filename_never_verify_identity() {
        let temp = tempdir().unwrap();
        let mut roots = roots(&temp);
        let root = temp.path().join("custom");
        fs::create_dir_all(root.join("configs")).unwrap();
        fs::write(root.join("amiberry.conf"), "").unwrap();
        fs::write(
            root.join("configs/Definitely A Game.uae"),
            "amiga_model=A500",
        )
        .unwrap();
        roots
            .explicit_configuration_roots
            .push((AmigaEmulatorKind::Amiberry, root));
        let profile = profile(&roots, AmigaEmulatorKind::Amiberry);
        let request = AmigaGameRequest {
            emulator_metadata: Some("title in emulator config".into()),
            selected_profile_path: profile.profile_paths.first().cloned(),
            ..Default::default()
        };
        let inspection = inspect_amiga_whdload_game(&profile, &request);
        assert_eq!(
            inspection.game_mapping,
            AmigaGameMapping::EmulatorMetadataOnly
        );
        assert!(inspection.verified_identity.is_none());
        assert!(inspection.per_game_config.is_some());
    }

    #[test]
    fn pfs_stays_detection_only_and_version_is_fail_soft() {
        assert_eq!(parse_amiga_version("Amiberry v6.0.2"), Some("6.0.2".into()));
        assert_eq!(parse_amiga_version("unknown build"), None);
        let partition = AmigaHdfPartitionInspection {
            filesystem: FileSystem::Pfs,
            discovered: None,
            warning: Some("filesystem is detection-only; traversal was not attempted".into()),
        };
        assert!(partition.discovered.is_none());
        assert!(partition.warning.unwrap().contains("detection-only"));
    }
}
