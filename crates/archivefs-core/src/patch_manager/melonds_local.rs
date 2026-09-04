//! Bounded, read-only melonDS discovery and configuration inspection.
//!
//! This adapter intentionally models only the standalone Nintendo DS target.
//! It does not turn `.nds` into identity evidence and it never writes the
//! emulator configuration or firmware.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const MELONDS_MAX_PROFILES: usize = 16;
pub const MELONDS_MAX_CONFIG_BYTES: u64 = 256 * 1024;
const FLATPAK_APP_ID: &str = "net.kuribo64.melonDS";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MelonDsInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MelonDsFirmwareMode {
    DirectBoot,
    ExternalFirmwareBoot,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MelonDsFirmwareState {
    NotRequiredForDirectBoot,
    PresentUnverified,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsFirmwareEvidence {
    pub mode: MelonDsFirmwareMode,
    pub bios7: MelonDsFirmwareState,
    pub bios9: MelonDsFirmwareState,
    pub firmware: MelonDsFirmwareState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsExecutable {
    pub path: PathBuf,
    pub installation_type: MelonDsInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsConfigInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub direct_boot: Option<bool>,
    pub external_bios_enabled: Option<bool>,
    pub bios7_path: Option<PathBuf>,
    pub bios9_path: Option<PathBuf>,
    pub firmware_path: Option<PathBuf>,
    pub dsi_settings_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsProfile {
    pub profile_id: String,
    pub installation_type: MelonDsInstallationType,
    pub configuration_path: PathBuf,
    pub config_path: Option<PathBuf>,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<MelonDsExecutable>,
    pub firmware: MelonDsFirmwareEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsProfileDiscovery {
    pub profiles: Vec<MelonDsProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
}

impl MelonDsProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, MelonDsDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(MelonDsDiscoveryError::HomeUnavailable)?;
        let xdg_config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let appimage_directory =
            env::var_os("APPIMAGE").and_then(|p| PathBuf::from(p).parent().map(Path::to_path_buf));
        Ok(Self {
            home,
            xdg_config_home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MelonDsDiscoveryError {
    HomeUnavailable,
}

impl std::fmt::Display for MelonDsDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set")
    }
}
impl std::error::Error for MelonDsDiscoveryError {}

fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}

fn executable_candidates(roots: &MelonDsProfileDiscoveryRoots) -> Vec<MelonDsExecutable> {
    let mut paths: Vec<(PathBuf, MelonDsInstallationType)> = roots
        .explicit_executables
        .iter()
        .cloned()
        .map(|p| (p, MelonDsInstallationType::Explicit))
        .collect();
    if let Some(dir) = &roots.appimage_directory {
        for name in ["melonDS.AppImage", "melonds.AppImage"] {
            paths.push((dir.join(name), MelonDsInstallationType::Portable));
        }
    }
    if let Some(path_env) = env::var_os("PATH") {
        for dir in env::split_paths(&path_env) {
            for name in ["melonDS", "melonds"] {
                paths.push((dir.join(name), MelonDsInstallationType::Native));
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|(p, _)| regular(p))
        .map(|(path, installation_type)| {
            let version = roots
                .known_version_outputs
                .get(&path)
                .and_then(|o| parse_melonds_version(o));
            MelonDsExecutable {
                path,
                installation_type,
                version,
            }
        })
        .collect()
}

fn config_path(root: &Path) -> Option<PathBuf> {
    [root.join("melonDS.toml"), root.join("melonDS.ini")]
        .into_iter()
        .find(|p| regular(p))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn parse_config(path: &Path) -> MelonDsConfigInspection {
    let bytes = fs::read(path).ok();
    let readable = bytes
        .as_ref()
        .is_some_and(|b| b.len() as u64 <= MELONDS_MAX_CONFIG_BYTES);
    let text = bytes
        .filter(|b| b.len() as u64 <= MELONDS_MAX_CONFIG_BYTES)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let mut direct_boot = None;
    let mut external_bios_enabled = None;
    let mut bios7_path = None;
    let mut bios9_path = None;
    let mut firmware_path = None;
    let mut dsi_settings_present = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.ends_with("DirectBoot") {
            direct_boot = parse_bool(value);
        } else if key.ends_with("ExternalBIOSEnable") {
            external_bios_enabled = parse_bool(value);
        } else if key.ends_with("BIOS7Path") {
            bios7_path = Some(PathBuf::from(value.trim_matches('"')));
        } else if key.ends_with("BIOS9Path") {
            bios9_path = Some(PathBuf::from(value.trim_matches('"')));
        } else if key.ends_with("FirmwarePath") {
            firmware_path = Some(PathBuf::from(value.trim_matches('"')));
        } else if key.starts_with("DSi.") || key.starts_with("DSi/") {
            dsi_settings_present = true;
        }
    }
    MelonDsConfigInspection {
        path: path.to_path_buf(),
        exists: true,
        readable,
        direct_boot,
        external_bios_enabled,
        bios7_path,
        bios9_path,
        firmware_path,
        dsi_settings_present,
    }
}

fn resolved_path(config_dir: &Path, configured: Option<&PathBuf>) -> Option<PathBuf> {
    configured.map(|p| {
        if p.is_absolute() {
            p.clone()
        } else {
            config_dir.join(p)
        }
    })
}

fn firmware(config_dir: &Path, config: Option<&PathBuf>) -> MelonDsFirmwareState {
    resolved_path(config_dir, config).map_or(MelonDsFirmwareState::Missing, |p| {
        if regular(&p) {
            MelonDsFirmwareState::PresentUnverified
        } else {
            MelonDsFirmwareState::Missing
        }
    })
}

fn profile(
    root: PathBuf,
    installation_type: MelonDsInstallationType,
    executables: &[MelonDsExecutable],
) -> MelonDsProfile {
    let config = config_path(&root);
    let inspection = config.as_ref().map(|p| parse_config(p));
    let mode = match inspection.as_ref().and_then(|c| c.direct_boot) {
        Some(true) => MelonDsFirmwareMode::DirectBoot,
        Some(false) => MelonDsFirmwareMode::ExternalFirmwareBoot,
        None => MelonDsFirmwareMode::Unknown,
    };
    let evidence = match mode {
        MelonDsFirmwareMode::DirectBoot => MelonDsFirmwareEvidence {
            mode,
            bios7: MelonDsFirmwareState::NotRequiredForDirectBoot,
            bios9: MelonDsFirmwareState::NotRequiredForDirectBoot,
            firmware: MelonDsFirmwareState::NotRequiredForDirectBoot,
        },
        MelonDsFirmwareMode::ExternalFirmwareBoot => {
            let c = inspection.as_ref();
            MelonDsFirmwareEvidence {
                mode,
                bios7: firmware(&root, c.and_then(|x| x.bios7_path.as_ref())),
                bios9: firmware(&root, c.and_then(|x| x.bios9_path.as_ref())),
                firmware: firmware(&root, c.and_then(|x| x.firmware_path.as_ref())),
            }
        }
        MelonDsFirmwareMode::Unknown => MelonDsFirmwareEvidence {
            mode,
            bios7: MelonDsFirmwareState::Unknown,
            bios9: MelonDsFirmwareState::Unknown,
            firmware: MelonDsFirmwareState::Unknown,
        },
    };
    let matching: Vec<MelonDsExecutable> = executables
        .iter()
        .filter(|e| {
            e.installation_type == installation_type
                || installation_type == MelonDsInstallationType::Explicit
        })
        .cloned()
        .collect();
    let eligible =
        config.is_some() && !matching.is_empty() && inspection.as_ref().is_some_and(|c| c.readable);
    let blocker = (!eligible).then(|| {
        if config.is_none() {
            "no melonDS.toml or melonDS.ini was found".to_string()
        } else if matching.is_empty() {
            "no safe melonDS executable was discovered".to_string()
        } else {
            "melonDS configuration is unreadable or oversized".to_string()
        }
    });
    MelonDsProfile {
        profile_id: format!("melonds:{}", root.display()),
        installation_type,
        configuration_path: root,
        config_path: config,
        eligible,
        blocker,
        executable_candidates: matching,
        firmware: evidence,
    }
}

pub fn discover_melonds_profiles(roots: &MelonDsProfileDiscoveryRoots) -> MelonDsProfileDiscovery {
    let mut candidates = vec![
        (
            roots.xdg_config_home.join("melonDS"),
            MelonDsInstallationType::Native,
        ),
        (
            roots
                .home
                .join(".var/app")
                .join(FLATPAK_APP_ID)
                .join("config/melonDS"),
            MelonDsInstallationType::FlatpakUser,
        ),
    ];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, MelonDsInstallationType::Portable)),
    );
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, MelonDsInstallationType::Explicit)),
    );
    if let Some(dir) = &roots.appimage_directory {
        candidates.push((dir.join("melonDS"), MelonDsInstallationType::Portable));
    }
    candidates.sort();
    candidates.dedup_by(|a, b| a.0 == b.0);
    let executables = executable_candidates(roots);
    let profiles = candidates
        .into_iter()
        .filter(|(p, kind)| {
            p.is_dir()
                || matches!(
                    kind,
                    MelonDsInstallationType::Explicit | MelonDsInstallationType::Portable
                )
        })
        .take(MELONDS_MAX_PROFILES)
        .map(|(p, k)| profile(p, k, &executables))
        .collect();
    MelonDsProfileDiscovery {
        profiles,
        complete: true,
    }
}

pub fn parse_melonds_version(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let i = lower.find("melonds")?;
    let tail = output[i + 7..].trim_start().trim_start_matches('v');
    let value: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!value.is_empty() && value.chars().next().is_some_and(|c| c.is_ascii_digit())).then_some(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MelonDsLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    AmbiguousExecutable,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsLaunchBlocker {
    pub kind: MelonDsLaunchBlockerKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsNativeLaunchBinding {
    pub executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MelonDsGameRequest {
    pub verified_game_key: Option<String>,
}

pub fn resolve_melonds_native_launch_binding(
    profile: &MelonDsProfile,
) -> Result<MelonDsNativeLaunchBinding, MelonDsLaunchBlocker> {
    if !profile.eligible {
        return Err(MelonDsLaunchBlocker {
            kind: MelonDsLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "profile is not eligible".into()),
        });
    }
    let matching: Vec<_> = profile
        .executable_candidates
        .iter()
        .filter(|e| {
            e.installation_type == profile.installation_type
                || profile.installation_type == MelonDsInstallationType::Explicit
        })
        .collect();
    let valid: Vec<_> = matching
        .into_iter()
        .filter(|e| {
            fs::symlink_metadata(&e.path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
                && is_executable(&e.path)
        })
        .collect();
    match valid.as_slice() {
        [] => Err(MelonDsLaunchBlocker {
            kind: MelonDsLaunchBlockerKind::ExecutableMissing,
            detail: "no safe executable matches this profile".into(),
        }),
        [one] => Ok(MelonDsNativeLaunchBinding {
            executable: one.path.clone(),
        }),
        _ => Err(MelonDsLaunchBlocker {
            kind: MelonDsLaunchBlockerKind::AmbiguousExecutable,
            detail: "more than one safe executable matches this profile".into(),
        }),
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    regular(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn melonds_version_is_bounded_and_optional() {
        assert_eq!(parse_melonds_version("melonDS 1.1"), Some("1.1".into()));
        assert_eq!(parse_melonds_version("unknown"), None);
    }

    #[test]
    fn melonds_direct_boot_does_not_require_external_firmware() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("melonDS.toml");
        fs::write(&config, "Emu.DirectBoot = true\n").unwrap();
        let inspection = parse_config(&config);
        assert_eq!(inspection.direct_boot, Some(true));
        let profile = profile(
            dir.path().to_path_buf(),
            MelonDsInstallationType::Explicit,
            &[],
        );
        assert_eq!(profile.firmware.mode, MelonDsFirmwareMode::DirectBoot);
        assert_eq!(
            profile.firmware.firmware,
            MelonDsFirmwareState::NotRequiredForDirectBoot
        );
    }

    #[test]
    fn melonds_external_firmware_is_presence_only_evidence() {
        let dir = tempdir().unwrap();
        let bios7 = dir.path().join("bios7.bin");
        let bios9 = dir.path().join("bios9.bin");
        let firmware = dir.path().join("firmware.bin");
        for path in [&bios7, &bios9, &firmware] {
            fs::write(path, b"fixture").unwrap();
        }
        let config = dir.path().join("melonDS.ini");
        fs::write(
            &config,
            format!(
                "Emu.DirectBoot = false\nDS.BIOS7Path = \"{}\"\nDS.BIOS9Path = \"{}\"\nDS.FirmwarePath = \"{}\"\n",
                bios7.display(),
                bios9.display(),
                firmware.display()
            ),
        )
        .unwrap();
        let profile = profile(
            dir.path().to_path_buf(),
            MelonDsInstallationType::Explicit,
            &[],
        );
        assert_eq!(
            profile.firmware.mode,
            MelonDsFirmwareMode::ExternalFirmwareBoot
        );
        assert_eq!(
            profile.firmware.bios7,
            MelonDsFirmwareState::PresentUnverified
        );
    }
}
