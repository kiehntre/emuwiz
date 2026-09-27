//! Post-install cheat loadability verification.
//!
//! A successful filesystem transaction proves only that bytes were written.
//! This module separately classifies whether the *selected* emulator is
//! expected to load them, using read-only evidence: the installed file's
//! bytes, the emulator's cheat-directory and filename convention, and its
//! cheat-enable setting. It never claims a cheat executed - EmuWiz has no
//! runtime evidence of that - only that the emulator is configured to load
//! it.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::cheat_route::{CheatApplySupport, CheatRoute, CheatRouteTarget};

/// Upper bound for re-reading an installed cheat file.
pub const MAX_LOADABILITY_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Upper bound for reading an emulator config to look up one setting.
pub const MAX_LOADABILITY_CONFIG_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatLoadabilityState {
    /// Path convention matches and the emulator's cheat setting is on.
    LoadableVerifiedByConfig,
    /// Path convention matches; the cheat setting could not be read.
    LoadableExpected,
    /// The emulator appears to be running and reads cheats only at
    /// game/content start.
    RestartRequired,
    /// The emulator's cheat setting is explicitly off.
    EmulatorCheatsDisabled,
    /// The file is not where (or not what) the selected emulator reads.
    PathNotObserved,
    /// EmuWiz cannot install cheats for the selected emulator.
    UnsupportedBySelectedEmulator,
    /// Which emulator/core reads this install is not known.
    AmbiguousProfile,
    Unknown,
}

impl CheatLoadabilityState {
    /// Whether the selected emulator is expected to load the cheat, as far as
    /// read-only evidence shows. `None` means it cannot be determined.
    pub fn will_load(self) -> Option<bool> {
        match self {
            Self::LoadableVerifiedByConfig | Self::LoadableExpected | Self::RestartRequired => {
                Some(true)
            }
            Self::EmulatorCheatsDisabled
            | Self::PathNotObserved
            | Self::UnsupportedBySelectedEmulator => Some(false),
            Self::AmbiguousProfile | Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatRestartRequirement {
    /// The emulator reads cheats when the game boots; a game that is
    /// already running must be restarted.
    RestartGame,
    /// RetroArch reads per-game cheats when content loads; running content
    /// must be reloaded (or the file loaded from Quick Menu -> Cheats).
    ReloadContent,
    Unknown,
}

pub fn cheat_restart_requirement(target: &CheatRouteTarget) -> CheatRestartRequirement {
    match target {
        CheatRouteTarget::RetroArch { .. } => CheatRestartRequirement::ReloadContent,
        CheatRouteTarget::Standalone { adapter_id } => match adapter_id.as_str() {
            "pcsx2" | "dolphin" | "xenia" | "duckstation" | "ppsspp" | "mgba" | "mame"
            | "flycast" | "rpcs3" => CheatRestartRequirement::RestartGame,
            _ => CheatRestartRequirement::Unknown,
        },
    }
}

/// Result of re-reading the installed destination after the transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum CheatInstalledFileCheck {
    Verified {
        sha256: String,
    },
    Missing,
    NotARegularFile,
    TooLarge,
    Unreadable {
        detail: String,
    },
    DigestMismatch {
        expected: String,
        observed: String,
    },
    /// No expected digest was available, so bytes could not be compared.
    NotChecked,
}

impl CheatInstalledFileCheck {
    pub fn verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

fn normalize_digest(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("sha256:")
        .trim_start_matches("sha256-")
        .to_ascii_lowercase()
}

/// Re-reads `path` (never following a final symlink) and compares its
/// SHA-256 with the digest the transaction recorded.
pub fn verify_installed_cheat_file(
    path: &Path,
    expected_sha256: Option<&str>,
) -> CheatInstalledFileCheck {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CheatInstalledFileCheck::Missing;
        }
        Err(error) => {
            return CheatInstalledFileCheck::Unreadable {
                detail: error.to_string(),
            };
        }
    };
    if !metadata.file_type().is_file() {
        return CheatInstalledFileCheck::NotARegularFile;
    }
    if metadata.len() > MAX_LOADABILITY_FILE_BYTES {
        return CheatInstalledFileCheck::TooLarge;
    }
    let Some(expected) = expected_sha256 else {
        return CheatInstalledFileCheck::NotChecked;
    };
    let mut bytes = Vec::new();
    match File::open(path).and_then(|file| {
        file.take(MAX_LOADABILITY_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
    }) {
        Ok(_) if bytes.len() as u64 > MAX_LOADABILITY_FILE_BYTES => {
            return CheatInstalledFileCheck::TooLarge;
        }
        Ok(_) => {}
        Err(error) => {
            return CheatInstalledFileCheck::Unreadable {
                detail: error.to_string(),
            };
        }
    }
    let observed = hex(&Sha256::digest(&bytes));
    let expected = normalize_digest(expected);
    if observed == expected {
        CheatInstalledFileCheck::Verified { sha256: observed }
    } else {
        CheatInstalledFileCheck::DigestMismatch { expected, observed }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Where the selected emulator reads cheats for this game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatLoadPathConvention {
    /// The directory the emulator reads this game's cheat file from.
    pub directory: PathBuf,
    /// The exact file name the emulator looks for, when known.
    pub file_name: Option<String>,
    /// A required lowercase file-name suffix (e.g. `.pnach`), when the exact
    /// name is not known.
    pub required_suffix: Option<String>,
    /// Stable identifier describing the convention (for technical details).
    pub convention: &'static str,
}

/// Whether and how the emulator's global cheat switch was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatEnablementEvidence {
    /// `None` when the setting is absent or the config could not be read.
    pub enabled: Option<bool>,
    /// The config key consulted, e.g. `[EmuCore] EnableCheats`.
    pub setting: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatIdentityStrength {
    /// Serial/CRC/Game ID/hash identity.
    Exact,
    /// Matched only by platform and title.
    TitleOnly,
    Unknown,
}

/// Whether the emulator process was observed running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorProcessObservation {
    Running,
    NotRunning,
    NotObserved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatLoadabilityInput {
    pub route: CheatRoute,
    /// The installed file (destination root joined with its relative path).
    pub installed_path: Option<PathBuf>,
    pub file_check: CheatInstalledFileCheck,
    pub expected_path: Option<CheatLoadPathConvention>,
    pub enablement: Option<CheatEnablementEvidence>,
    pub process: EmulatorProcessObservation,
    pub identity: CheatIdentityStrength,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "evidence", rename_all = "snake_case")]
pub enum CheatLoadabilityEvidence {
    SelectedEmulator { name: String },
    RetroArchCore { core: String },
    InstalledBytesVerified { sha256: String },
    DestinationMatchesConvention { convention: &'static str },
    CheatsEnabledInConfig { setting: &'static str },
    EmulatorRunning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "issue", rename_all = "snake_case")]
pub enum CheatLoadabilityIssue {
    ApplyUnsupportedForEmulator,
    InstalledFileNotVerified { detail: String },
    RetroArchCoreUnknown,
    DestinationOutsideEmulatorDirectory { expected_directory: PathBuf },
    FileNameNotReadByEmulator { expected: String },
    RetroArchManualLoadRequired,
    ExpectedPathUnknown,
    CheatsDisabledInConfig { setting: &'static str },
    CheatSettingUnreadable { setting: &'static str },
    GameRevisionNotVerified,
}

impl CheatLoadabilityIssue {
    pub fn message(&self) -> String {
        match self {
            Self::ApplyUnsupportedForEmulator => {
                "EmuWiz cannot install cheats for the selected emulator.".to_string()
            }
            Self::InstalledFileNotVerified { detail } => {
                format!("The installed file could not be verified: {detail}.")
            }
            Self::RetroArchCoreUnknown => {
                "EmuWiz cannot verify which RetroArch core this install belongs to.".to_string()
            }
            Self::DestinationOutsideEmulatorDirectory { expected_directory } => format!(
                "The file is not in the folder the selected emulator reads ({}).",
                expected_directory.display()
            ),
            Self::FileNameNotReadByEmulator { expected } => {
                format!("The selected emulator looks for a file named {expected}.")
            }
            Self::RetroArchManualLoadRequired => "RetroArch does not load this file automatically. Open Quick Menu → Cheats → Load Cheat File and choose it.".to_string(),
            Self::ExpectedPathUnknown => {
                "EmuWiz does not know where the selected emulator reads cheats from.".to_string()
            }
            Self::CheatsDisabledInConfig { setting } => {
                format!("Cheats are disabled in the selected emulator's settings ({setting}).")
            }
            Self::CheatSettingUnreadable { setting } => format!(
                "EmuWiz could not read the emulator's cheat setting ({setting}); check it is on."
            ),
            Self::GameRevisionNotVerified => {
                "Game revision not verified: this cheat was matched by title and platform only."
                    .to_string()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatLoadabilityReport {
    pub selected_emulator: String,
    pub retroarch_core: Option<String>,
    pub install_target: Option<PathBuf>,
    /// Whether the file itself is installed and its bytes verified. Kept
    /// separate from `state`, which is about the emulator loading it.
    pub file_installed: bool,
    pub file_check: CheatInstalledFileCheck,
    pub state: CheatLoadabilityState,
    pub restart: CheatRestartRequirement,
    pub evidence: Vec<CheatLoadabilityEvidence>,
    pub issues: Vec<CheatLoadabilityIssue>,
}

impl CheatLoadabilityReport {
    /// One plain-language sentence. Never claims the cheat executed.
    pub fn headline(&self) -> String {
        if !self.file_installed {
            return "The cheat file could not be verified after installing.".to_string();
        }
        match self.state {
            CheatLoadabilityState::LoadableVerifiedByConfig => {
                "Installed and ready.".to_string()
            }
            CheatLoadabilityState::LoadableExpected => {
                "Installed. The selected emulator should load it; EmuWiz could not confirm its cheat setting.".to_string()
            }
            CheatLoadabilityState::RestartRequired => {
                "Installed. Restart the emulator to load this cheat.".to_string()
            }
            CheatLoadabilityState::EmulatorCheatsDisabled => {
                "Installed, but cheats are disabled in the selected emulator.".to_string()
            }
            CheatLoadabilityState::PathNotObserved => {
                "Installed, but the selected emulator is not currently configured to load it."
                    .to_string()
            }
            CheatLoadabilityState::UnsupportedBySelectedEmulator => {
                "This cheat format is not supported by your selected emulator.".to_string()
            }
            CheatLoadabilityState::AmbiguousProfile => {
                "EmuWiz cannot verify which RetroArch core this install belongs to.".to_string()
            }
            CheatLoadabilityState::Unknown => {
                "Installed. EmuWiz cannot tell whether the selected emulator will load it."
                    .to_string()
            }
        }
    }

    /// Short restart guidance that applies whenever the file will load.
    pub fn restart_note(&self) -> Option<&'static str> {
        if self.state.will_load() != Some(true) {
            return None;
        }
        match self.restart {
            CheatRestartRequirement::RestartGame => {
                Some("If the game is already running, restart it to load this cheat.")
            }
            CheatRestartRequirement::ReloadContent => Some(
                "If the game is already running, close and reload the content to load this cheat.",
            ),
            CheatRestartRequirement::Unknown => None,
        }
    }
}

fn path_is_directly_inside(path: &Path, directory: &Path) -> bool {
    path.parent().is_some_and(|parent| parent == directory)
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}

/// Classifies loadability. Pure: all filesystem facts arrive in `input`.
pub fn assess_cheat_loadability(input: &CheatLoadabilityInput) -> CheatLoadabilityReport {
    let target = &input.route.target;
    let mut evidence = vec![CheatLoadabilityEvidence::SelectedEmulator {
        name: target.display_name(),
    }];
    let mut issues = Vec::new();
    let retroarch_core = target.retroarch_core().map(str::to_owned);
    if let Some(core) = &retroarch_core {
        evidence.push(CheatLoadabilityEvidence::RetroArchCore { core: core.clone() });
    }
    if let CheatInstalledFileCheck::Verified { sha256 } = &input.file_check {
        evidence.push(CheatLoadabilityEvidence::InstalledBytesVerified {
            sha256: sha256.clone(),
        });
    }
    if input.identity == CheatIdentityStrength::TitleOnly {
        issues.push(CheatLoadabilityIssue::GameRevisionNotVerified);
    }
    let restart = cheat_restart_requirement(target);
    let file_installed = input.file_check.verified();

    let state = (|| {
        if input.route.apply_support != CheatApplySupport::Supported {
            issues.push(CheatLoadabilityIssue::ApplyUnsupportedForEmulator);
            return CheatLoadabilityState::UnsupportedBySelectedEmulator;
        }
        if !file_installed {
            issues.push(CheatLoadabilityIssue::InstalledFileNotVerified {
                detail: file_check_detail(&input.file_check),
            });
            return CheatLoadabilityState::PathNotObserved;
        }
        if matches!(target, CheatRouteTarget::RetroArch { core: None }) {
            issues.push(CheatLoadabilityIssue::RetroArchCoreUnknown);
            return CheatLoadabilityState::AmbiguousProfile;
        }
        let (Some(expected), Some(installed)) = (&input.expected_path, &input.installed_path)
        else {
            issues.push(CheatLoadabilityIssue::ExpectedPathUnknown);
            return CheatLoadabilityState::Unknown;
        };
        if !path_is_directly_inside(installed, &expected.directory) {
            if matches!(target, CheatRouteTarget::RetroArch { .. }) {
                issues.push(CheatLoadabilityIssue::RetroArchManualLoadRequired);
            } else {
                issues.push(CheatLoadabilityIssue::DestinationOutsideEmulatorDirectory {
                    expected_directory: expected.directory.clone(),
                });
            }
            return CheatLoadabilityState::PathNotObserved;
        }
        let installed_name = installed
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(name) = &expected.file_name
            && &installed_name != name
        {
            if matches!(target, CheatRouteTarget::RetroArch { .. }) {
                issues.push(CheatLoadabilityIssue::RetroArchManualLoadRequired);
            }
            issues.push(CheatLoadabilityIssue::FileNameNotReadByEmulator {
                expected: name.clone(),
            });
            return CheatLoadabilityState::PathNotObserved;
        }
        if let Some(suffix) = &expected.required_suffix
            && !installed_name
                .to_ascii_lowercase()
                .ends_with(suffix.as_str())
        {
            issues.push(CheatLoadabilityIssue::FileNameNotReadByEmulator {
                expected: format!("*{suffix}"),
            });
            return CheatLoadabilityState::PathNotObserved;
        }
        evidence.push(CheatLoadabilityEvidence::DestinationMatchesConvention {
            convention: expected.convention,
        });
        let enabled = match &input.enablement {
            Some(CheatEnablementEvidence {
                enabled: Some(false),
                setting,
            }) => {
                issues.push(CheatLoadabilityIssue::CheatsDisabledInConfig { setting: *setting });
                return CheatLoadabilityState::EmulatorCheatsDisabled;
            }
            Some(CheatEnablementEvidence {
                enabled: Some(true),
                setting,
            }) => {
                evidence
                    .push(CheatLoadabilityEvidence::CheatsEnabledInConfig { setting: *setting });
                true
            }
            Some(CheatEnablementEvidence {
                enabled: None,
                setting,
            }) => {
                issues.push(CheatLoadabilityIssue::CheatSettingUnreadable { setting: *setting });
                false
            }
            None => false,
        };
        if input.process == EmulatorProcessObservation::Running {
            evidence.push(CheatLoadabilityEvidence::EmulatorRunning);
            return CheatLoadabilityState::RestartRequired;
        }
        if enabled {
            CheatLoadabilityState::LoadableVerifiedByConfig
        } else {
            CheatLoadabilityState::LoadableExpected
        }
    })();

    CheatLoadabilityReport {
        selected_emulator: target.display_name(),
        retroarch_core,
        install_target: input.installed_path.clone(),
        file_installed,
        file_check: input.file_check.clone(),
        state,
        restart,
        evidence,
        issues,
    }
}

fn file_check_detail(check: &CheatInstalledFileCheck) -> String {
    match check {
        CheatInstalledFileCheck::Verified { .. } => "verified".to_string(),
        CheatInstalledFileCheck::Missing => "the file is missing".to_string(),
        CheatInstalledFileCheck::NotARegularFile => "the path is not a regular file".to_string(),
        CheatInstalledFileCheck::TooLarge => "the file is larger than the check limit".to_string(),
        CheatInstalledFileCheck::Unreadable { detail } => format!("unreadable ({detail})"),
        CheatInstalledFileCheck::DigestMismatch { .. } => {
            "its contents differ from what was installed".to_string()
        }
        CheatInstalledFileCheck::NotChecked => "no recorded digest to compare".to_string(),
    }
}

/// Reads one boolean `key = value` setting from a small INI/CFG/TOML-style
/// config, optionally restricted to `[section]`. Read-only and bounded;
/// symlinked or oversized configs yield `None`. Accepts `true/false`,
/// `1/0`, `yes/no`, `on/off`, optionally quoted.
pub fn read_config_bool(path: &Path, section: Option<&str>, key: &str) -> Option<bool> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_LOADABILITY_CONFIG_BYTES {
        return None;
    }
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(MAX_LOADABILITY_CONFIG_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    parse_config_bool(&text, section, key)
}

pub fn parse_config_bool(text: &str, section: Option<&str>, key: &str) -> Option<bool> {
    let mut current_section: Option<String> = None;
    let mut found = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current_section = Some(line[1..line.len() - 1].trim().to_string());
            continue;
        }
        if let Some(section) = section
            && !current_section
                .as_deref()
                .is_some_and(|current| current.eq_ignore_ascii_case(section))
        {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case(key) {
            continue;
        }
        let value = value.split('#').next().unwrap_or_default().trim();
        let value = value.trim_matches('"').trim_matches('\'').trim();
        found = match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        };
    }
    found
}

/// Linux process-name fragments per emulator. Matching is on the kernel
/// `comm` name, which is truncated to 15 bytes.
fn process_name_fragments(target: &CheatRouteTarget) -> &'static [&'static str] {
    match target {
        CheatRouteTarget::RetroArch { .. } => &["retroarch"],
        CheatRouteTarget::Standalone { adapter_id } => match adapter_id.as_str() {
            "pcsx2" => &["pcsx2"],
            "dolphin" => &["dolphin-emu"],
            "xenia" => &["xenia"],
            "duckstation" => &["duckstation"],
            "ppsspp" => &["ppsspp"],
            "mgba" => &["mgba"],
            "mame" => &["mame"],
            "flycast" => &["flycast"],
            "rpcs3" => &["rpcs3"],
            "azahar" => &["azahar"],
            _ => &[],
        },
    }
}

/// Maximum `/proc` entries scanned when looking for a running emulator.
const MAX_PROC_ENTRIES: usize = 8_192;

/// Read-only check for a running emulator process (Linux `/proc` only).
/// Returns `NotObserved` when the check is unavailable or incomplete.
pub fn observe_emulator_process(target: &CheatRouteTarget) -> EmulatorProcessObservation {
    observe_emulator_process_in(Path::new("/proc"), target)
}

pub fn observe_emulator_process_in(
    proc_root: &Path,
    target: &CheatRouteTarget,
) -> EmulatorProcessObservation {
    let fragments = process_name_fragments(target);
    if fragments.is_empty() {
        return EmulatorProcessObservation::NotObserved;
    }
    let Ok(entries) = fs::read_dir(proc_root) else {
        return EmulatorProcessObservation::NotObserved;
    };
    let mut scanned = 0usize;
    for entry in entries {
        scanned += 1;
        if scanned > MAX_PROC_ENTRIES {
            return EmulatorProcessObservation::NotObserved;
        }
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        if !name
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let Ok(comm) = fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        let comm = comm.trim().to_ascii_lowercase();
        if fragments.iter().any(|fragment| comm.contains(fragment)) {
            return EmulatorProcessObservation::Running;
        }
    }
    EmulatorProcessObservation::NotRunning
}

#[cfg(test)]
mod tests;
