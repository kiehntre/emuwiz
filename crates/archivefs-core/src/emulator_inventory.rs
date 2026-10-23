//! Bounded, read-only inventory of installed emulator executables.
//!
//! This is deliberately an inventory projection, not an installer or a second
//! profile/discovery system.  Callers may provide proven candidates (for
//! example from Doctor/profile evidence); the convenience scanner only checks
//! a bounded set of executable names on `PATH` and uses each emulator's
//! bounded version-reporting argument.

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
    Mame,
    Dolphin,
    Rpcs3,
    Pcsx2,
    Ppsspp,
    DuckStation,
    Xemu,
    ShadPs4,
    Flycast,
}

impl InventoryEmulator {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mame => "MAME",
            Self::Dolphin => "Dolphin",
            Self::Rpcs3 => "RPCS3",
            Self::Pcsx2 => "PCSX2",
            Self::Ppsspp => "PPSSPP",
            Self::DuckStation => "DuckStation",
            Self::Xemu => "xemu",
            Self::ShadPs4 => "shadPS4",
            Self::Flycast => "Flycast",
        }
    }

    fn executable_names(self) -> &'static [&'static str] {
        match self {
            Self::Mame => &["mame", "mame64"],
            // Bare `dolphin` is KDE's file manager on Linux, not the emulator.
            Self::Dolphin => &["dolphin-emu"],
            Self::Rpcs3 => &["rpcs3"],
            Self::Pcsx2 => &["pcsx2"],
            Self::Ppsspp => &["ppsspp"],
            Self::DuckStation => &["duckstation", "duckstation-qt"],
            Self::Xemu => &["xemu"],
            Self::ShadPs4 => &["shadps4", "shadPS4"],
            Self::Flycast => &["flycast"],
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
    output
        .lines()
        .flat_map(str::split_whitespace)
        .find_map(|raw| {
            let token = raw.trim_matches(|c: char| {
                !c.is_ascii_alphanumeric() && !matches!(c, '.' | '-' | '+' | '_')
            });
            let normalized = token.strip_prefix(['v', 'V']).unwrap_or(token);
            if token.len() > 128
                || normalized.is_empty()
                || !normalized.chars().any(|c| c.is_ascii_digit())
                || !normalized
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            {
                return None;
            }
            let valid = normalized
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'));
            valid.then(|| normalized.to_string())
        })
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

/// KDE's Dolphin file manager shares the emulator's name. Its `--version`
/// prints lowercase `dolphin <gear version>` (e.g. `dolphin 23.08.5`) and
/// mentions KDE, whereas the emulator prints `Dolphin <build>`. Evidence from
/// the executable itself, not only its name, so a renamed or wrapped file
/// manager is still refused.
fn is_kde_file_manager(version_output: Option<&str>) -> bool {
    if let Some(output) = version_output {
        let lower = output.to_ascii_lowercase();
        if lower.contains("kde gear") || lower.contains("org.kde.dolphin") {
            return true;
        }
        let first = output.trim_start().split_whitespace().next();
        let second = output.trim_start().split_whitespace().nth(1);
        if first == Some("dolphin")
            && second.is_some_and(|v| {
                v.split('.').count() == 3 && v.chars().next().is_some_and(|c| c.is_ascii_digit())
            })
        {
            return true;
        }
    }
    false
}

fn installation_root(path: &Path) -> PathBuf {
    path.parent().unwrap_or(path).to_path_buf()
}

fn version_probe_argument(emulator: InventoryEmulator) -> &'static str {
    match emulator {
        // MAME follows its long-standing single-dash CLI convention and
        // rejects `--version` as an unknown option.
        InventoryEmulator::Mame => "-version",
        _ => "--version",
    }
}

fn probe_version(path: &Path, emulator: InventoryEmulator) -> Option<String> {
    // shadPS4 has no read-only version probe: user paths are initialized
    // before argument parsing. Its adapter uses static recognition instead.
    // Flycast has no version CLI; its cheat installer uses pinned artifact hashes.
    if matches!(
        emulator,
        InventoryEmulator::ShadPs4 | InventoryEmulator::Flycast
    ) {
        return None;
    }
    let mut child = Command::new(path)
        .arg(version_probe_argument(emulator))
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

/// Reuse the same bounded emulator-specific version probe used by PATH
/// inventory callers.
/// Callers that need provenance can retain this raw output alongside the
/// resulting inventory projection.
pub fn probe_version_output(path: &Path, emulator: InventoryEmulator) -> Option<String> {
    probe_version(path, emulator)
}

/// Scan only executable names in the current `PATH`; no home-directory crawl.
pub fn discover_installed_emulators() -> EmulatorInventory {
    discover_installed_emulators_mode(true)
}

/// Inspects the same bounded PATH candidates and managed manifests without
/// executing any emulator, provider command, version probe or network request.
/// Versions absent from local manifests stay unknown. This does not inspect
/// Flatpak, AppImage locations or configured selections; callers must disclose
/// that scope rather than claim an emulator is absent from the whole machine.
pub fn discover_installed_emulators_read_only() -> EmulatorInventory {
    discover_installed_emulators_mode(false)
}

fn discover_installed_emulators_mode(probe_versions: bool) -> EmulatorInventory {
    let path_entries: Vec<PathBuf> = env::var_os("PATH")
        .into_iter()
        .flat_map(|p| env::split_paths(&p).collect::<Vec<_>>())
        .take(MAX_PATH_ENTRIES + 1)
        .collect();
    discover_in_path_entries(path_entries, probe_versions)
}

fn discover_in_path_entries(path_entries: Vec<PathBuf>, probe_versions: bool) -> EmulatorInventory {
    let mut candidates = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut warnings = Vec::new();
    if !probe_versions && (path_entries.is_empty() || path_entries.len() > MAX_PATH_ENTRIES) {
        warnings.push(InventoryWarning { message: "PATH is unavailable/empty or exceeds the 64-directory inspection limit; discovery is incomplete.".into() });
    }
    for directory in path_entries.into_iter().take(MAX_PATH_ENTRIES) {
        if !probe_versions && !directory.is_absolute() {
            warnings.push(InventoryWarning {
                message: format!(
                    "Relative PATH directory was not inspected: {}",
                    directory.display()
                ),
            });
            continue;
        }
        for emulator in [
            InventoryEmulator::Mame,
            InventoryEmulator::Dolphin,
            InventoryEmulator::Rpcs3,
            InventoryEmulator::Pcsx2,
            InventoryEmulator::Ppsspp,
            InventoryEmulator::DuckStation,
            InventoryEmulator::Xemu,
        ] {
            for name in emulator.executable_names() {
                let path = directory.join(name);
                if !probe_versions {
                    match fs::symlink_metadata(&path) {
                        Err(error) if error.kind() != std::io::ErrorKind::NotFound => warnings
                            .push(InventoryWarning {
                                message: format!("Cannot inspect {}: {error}", path.display()),
                            }),
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            warnings.push(InventoryWarning {
                                message: format!(
                                    "Redirected executable was not inspected: {}",
                                    path.display()
                                ),
                            })
                        }
                        _ => (),
                    }
                }
                if safe_executable(&path) && seen.insert(path.clone()) {
                    let version_output = if probe_versions {
                        probe_version(&path, emulator)
                    } else {
                        None
                    };
                    if emulator == InventoryEmulator::Dolphin
                        && is_kde_file_manager(version_output.as_deref())
                    {
                        continue;
                    }
                    candidates.push(InventoryCandidate {
                        emulator,
                        installation_root: installation_root(&path),
                        executable_path: path.clone(),
                        version_output,
                        installation_type: InstallationType::Unknown,
                        update_capability: UpdateCapability::ManualUnknown,
                        preferred: None,
                    });
                }
            }
        }
    }
    let mut inventory = inventory_from_candidates(candidates);
    inventory.warnings.extend(warnings);
    match crate::app_dirs::data_dir() {
        Ok(data_root) if !probe_versions && !data_root.is_absolute() => {
            inventory.warnings.push(InventoryWarning {
                message: format!(
                    "Managed emulator data location is unavailable: relative root {}",
                    data_root.display()
                ),
            });
        }
        Ok(data_root) => {
            let managed = enumerate_managed_installations(&data_root);
            inventory.managed_installations = managed.entries;
            inventory.warnings.extend(
                managed
                    .warnings
                    .into_iter()
                    .map(|message| InventoryWarning { message }),
            );
        }
        Err(error) if !probe_versions => inventory.warnings.push(InventoryWarning {
            message: format!("Managed emulator data location is unavailable: {error}"),
        }),
        Err(_) => (),
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
        assert_eq!(
            parse_version_output("RetroArch v1.20.4"),
            Some("1.20.4".into())
        );
        assert_eq!(
            parse_version_output("build 0.0.34-17000"),
            Some("0.0.34-17000".into())
        );
        assert_eq!(parse_version_output("nightly abc123"), None);
        assert_eq!(parse_version_output(""), None);
    }

    #[test]
    fn dolphin_does_not_claim_the_kde_file_manager() {
        assert!(
            !InventoryEmulator::Dolphin
                .executable_names()
                .contains(&"dolphin")
        );
    }

    #[test]
    fn kde_file_manager_is_recognised_by_evidence_not_name() {
        assert!(is_kde_file_manager(Some("dolphin 23.08.5\n")));
        assert!(is_kde_file_manager(Some("Dolphin from KDE Gear")));
        assert!(!is_kde_file_manager(Some("Dolphin 2509-1")));
        assert!(!is_kde_file_manager(Some("Dolphin 5.0-21460")));
        assert!(!is_kde_file_manager(Some("")));
        assert!(!is_kde_file_manager(Some(
            "qt.qpa: KDE platform theme loaded\nDolphin 2509-1"
        )));
        assert!(!is_kde_file_manager(None));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_skips_kde_dolphin_keeps_real_emulator_and_handles_empty_path() {
        use std::os::unix::fs::PermissionsExt;
        let make = |dir: &Path, name: &str, out: &str| {
            let p = dir.join(name);
            fs::write(&p, format!("#!/bin/sh\necho '{out}'\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        };
        let kde = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        make(kde.path(), "dolphin", "dolphin 23.08.5");
        make(kde.path(), "dolphin-emu", "dolphin 23.08.5");
        make(real.path(), "dolphin-emu", "Dolphin 2509-1");
        let run = |dirs: Vec<PathBuf>| discover_in_path_entries(dirs, true);
        let only_kde = run(vec![kde.path().into()]);
        assert!(
            only_kde
                .installations
                .iter()
                .all(|i| i.emulator != InventoryEmulator::Dolphin)
        );
        let both = run(vec![kde.path().into(), real.path().into()]);
        let dolphins: Vec<_> = both
            .installations
            .iter()
            .filter(|i| i.emulator == InventoryEmulator::Dolphin)
            .collect();
        assert_eq!(dolphins.len(), 1);
        assert!(dolphins[0].executable_path.starts_with(real.path()));
        let none = run(Vec::new());
        assert!(
            none.installations
                .iter()
                .all(|i| i.emulator != InventoryEmulator::Dolphin)
        );
    }

    #[test]
    fn mame_uses_its_supported_single_dash_version_flag() {
        assert_eq!(version_probe_argument(InventoryEmulator::Mame), "-version");
        assert_eq!(
            version_probe_argument(InventoryEmulator::Dolphin),
            "--version"
        );
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
