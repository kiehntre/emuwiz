//! Bounded, read-only inventory of installed emulator executables.
//!
//! This is deliberately an inventory projection, not an installer or a second
//! profile/discovery system.  Callers may provide proven candidates (for
//! example from Doctor/profile evidence); the convenience scanner only checks
//! a bounded set of executable names on `PATH` and probes `--version`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::managed_emulator_install::{
    ManagedInstallInventoryEntry, enumerate_managed_installations,
};

pub const MAX_PATH_ENTRIES: usize = 64;
pub const MAX_VERSION_OUTPUT_BYTES: usize = 16 * 1024;
pub const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, Ord, PartialOrd)]
pub enum InventoryEmulator {
    Dolphin,
    Rpcs3,
    Pcsx2,
    Ppsspp,
    DuckStation,
    Xemu,
}

impl InventoryEmulator {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dolphin => "Dolphin",
            Self::Rpcs3 => "RPCS3",
            Self::Pcsx2 => "PCSX2",
            Self::Ppsspp => "PPSSPP",
            Self::DuckStation => "DuckStation",
            Self::Xemu => "xemu",
        }
    }

    fn executable_names(self) -> &'static [&'static str] {
        match self {
            Self::Dolphin => &["dolphin-emu", "dolphin"],
            Self::Rpcs3 => &["rpcs3"],
            Self::Pcsx2 => &["pcsx2"],
            Self::Ppsspp => &["ppsspp"],
            Self::DuckStation => &["duckstation", "duckstation-qt"],
            Self::Xemu => &["xemu"],
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum VersionConfidence {
    VerifiedCommand,
    PackageMetadata,
    ProfileEvidence,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum VersionSource {
    VersionCommand,
    PackageMetadata,
    Profile,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum BuildChannel {
    Stable,
    Beta,
    Development,
    Nightly,
    Canary,
    Custom,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum InstallationType {
    SystemPackage,
    Flatpak,
    AppImage,
    Portable,
    Manual,
    Managed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateCapability {
    PackageManager,
    Flatpak,
    UpstreamRelease,
    PortableManaged,
    ManualUnknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum SaveStateRisk {
    VersionSensitive,
    Unknown,
    NotApplicable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InventoryWarning {
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmulatorInstallation {
    pub emulator: InventoryEmulator,
    pub executable_path: PathBuf,
    pub installation_root: PathBuf,
    pub version: Option<String>,
    pub version_confidence: VersionConfidence,
    pub version_source: VersionSource,
    pub channel: BuildChannel,
    pub installation_type: InstallationType,
    pub update_capability: UpdateCapability,
    pub preferred: Option<bool>,
    pub save_state_risk: SaveStateRisk,
    pub warnings: Vec<InventoryWarning>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmulatorInventory {
    pub installations: Vec<EmulatorInstallation>,
    #[serde(default)]
    pub managed_installations: Vec<ManagedInstallInventoryEntry>,
    pub warnings: Vec<InventoryWarning>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryCandidate {
    pub emulator: InventoryEmulator,
    pub executable_path: PathBuf,
    pub installation_root: PathBuf,
    pub version_output: Option<String>,
    pub installation_type: InstallationType,
    pub update_capability: UpdateCapability,
    pub preferred: Option<bool>,
}

pub fn parse_version_output(output: &str) -> Option<String> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let mut token = line
        .split_whitespace()
        .find(|token| {
            let token = token.trim_matches(|c: char| {
                !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '_'
            });
            token.chars().any(|c| c.is_ascii_digit()) && token.contains('.')
                || token.chars().filter(|c| *c == '.').count() >= 1
                    && token.chars().any(|c| c.is_ascii_digit())
        })?
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '_')
        .to_string();
    if token.len() > 128 || !token.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if token.starts_with("version") {
        token = token[7..]
            .trim_matches(|c: char| c == ':' || c == '=')
            .to_string();
    }
    (!token.is_empty()).then_some(token)
}

pub fn parse_channel(output: &str) -> BuildChannel {
    let lower = output.to_ascii_lowercase();
    if lower.contains("nightly") {
        BuildChannel::Nightly
    } else if lower.contains("canary") {
        BuildChannel::Canary
    } else if lower.contains("development") || lower.contains(" dev") || lower.contains("dev build")
    {
        BuildChannel::Development
    } else if lower.contains("beta") {
        BuildChannel::Beta
    } else if lower.contains("stable") {
        BuildChannel::Stable
    } else {
        BuildChannel::Unknown
    }
}

pub fn inventory_from_candidates(mut candidates: Vec<InventoryCandidate>) -> EmulatorInventory {
    candidates
        .sort_by(|a, b| (a.emulator, &a.executable_path).cmp(&(b.emulator, &b.executable_path)));
    let installations = candidates
        .into_iter()
        .map(|candidate| {
            let version = candidate
                .version_output
                .as_deref()
                .and_then(parse_version_output);
            let mut warnings = Vec::new();
            if version.is_none() {
                warnings.push(InventoryWarning {
                    message: "Installed executable found, but its version could not be verified."
                        .into(),
                });
            }
            let channel = candidate
                .version_output
                .as_deref()
                .map(parse_channel)
                .unwrap_or(BuildChannel::Unknown);
            EmulatorInstallation {
                emulator: candidate.emulator,
                executable_path: candidate.executable_path,
                installation_root: candidate.installation_root,
                version,
                version_confidence: if candidate.version_output.is_some() {
                    VersionConfidence::VerifiedCommand
                } else {
                    VersionConfidence::Unknown
                },
                version_source: if candidate.version_output.is_some() {
                    VersionSource::VersionCommand
                } else {
                    VersionSource::Unknown
                },
                channel,
                installation_type: candidate.installation_type,
                update_capability: candidate.update_capability,
                preferred: candidate.preferred,
                save_state_risk: if matches!(
                    candidate.emulator,
                    InventoryEmulator::Dolphin
                        | InventoryEmulator::Rpcs3
                        | InventoryEmulator::Pcsx2
                        | InventoryEmulator::Ppsspp
                        | InventoryEmulator::DuckStation
                        | InventoryEmulator::Xemu
                ) {
                    SaveStateRisk::VersionSensitive
                } else {
                    SaveStateRisk::Unknown
                },
                warnings,
            }
        })
        .collect();
    EmulatorInventory {
        installations,
        managed_installations: Vec::new(),
        warnings: Vec::new(),
    }
}

fn safe_executable(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.is_file() && !m.file_type().is_symlink())
        .unwrap_or(false)
}

fn installation_root(path: &Path) -> PathBuf {
    path.parent().unwrap_or(path).to_path_buf()
}

fn probe_version(path: &Path) -> Option<String> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if started.elapsed() >= VERSION_PROBE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().ok()?;
    let mut bytes = output.stdout;
    bytes.extend_from_slice(&output.stderr);
    bytes.truncate(MAX_VERSION_OUTPUT_BYTES);
    String::from_utf8(bytes).ok()
}

/// Scan only executable names in the current `PATH`; no home-directory crawl.
pub fn discover_installed_emulators() -> EmulatorInventory {
    let mut candidates = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let path_entries: Vec<PathBuf> = env::var_os("PATH")
        .into_iter()
        .flat_map(|p| env::split_paths(&p).collect::<Vec<_>>())
        .take(MAX_PATH_ENTRIES)
        .collect();
    for directory in path_entries {
        for emulator in [
            InventoryEmulator::Dolphin,
            InventoryEmulator::Rpcs3,
            InventoryEmulator::Pcsx2,
            InventoryEmulator::Ppsspp,
            InventoryEmulator::DuckStation,
            InventoryEmulator::Xemu,
        ] {
            for name in emulator.executable_names() {
                let path = directory.join(name);
                if safe_executable(&path) && seen.insert(path.clone()) {
                    candidates.push(InventoryCandidate {
                        emulator,
                        installation_root: installation_root(&path),
                        executable_path: path.clone(),
                        version_output: probe_version(&path),
                        installation_type: InstallationType::Unknown,
                        update_capability: UpdateCapability::ManualUnknown,
                        preferred: None,
                    });
                }
            }
        }
    }
    let mut inventory = inventory_from_candidates(candidates);
    if let Ok(data_root) = crate::app_dirs::data_dir() {
        let managed = enumerate_managed_installations(&data_root);
        inventory.managed_installations = managed.entries;
        inventory.warnings.extend(
            managed
                .warnings
                .into_iter()
                .map(|message| InventoryWarning { message }),
        );
    }
    inventory
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_parsing_fails_closed() {
        assert_eq!(
            parse_version_output("Dolphin 2509-1"),
            Some("2509-1".into())
        );
        assert_eq!(parse_version_output("not a version"), None);
    }
    #[test]
    fn channel_is_evidence_based() {
        assert_eq!(
            parse_channel("Dolphin development build"),
            BuildChannel::Development
        );
        assert_eq!(parse_channel("release"), BuildChannel::Unknown);
    }
    #[test]
    fn candidates_are_deterministic_and_keep_multiple_installs() {
        let make = |p: &str| InventoryCandidate {
            emulator: InventoryEmulator::Dolphin,
            executable_path: p.into(),
            installation_root: "/x".into(),
            version_output: Some("2509 stable".into()),
            installation_type: InstallationType::Manual,
            update_capability: UpdateCapability::ManualUnknown,
            preferred: None,
        };
        let inventory = inventory_from_candidates(vec![make("/b"), make("/a")]);
        assert_eq!(
            inventory.installations[0].executable_path,
            PathBuf::from("/a")
        );
        assert_eq!(inventory.installations.len(), 2);
    }
    #[test]
    fn real_smoke_is_bounded_and_read_only() {
        let _ = discover_installed_emulators();
    }
}
