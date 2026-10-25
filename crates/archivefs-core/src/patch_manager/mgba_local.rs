//! Bounded, read-only discovery and profile inspection for native mGBA.
//!
//! mGBA can boot Game Boy, Game Boy Color, and Game Boy Advance content without
//! an external BIOS.  A configured BIOS is therefore optional evidence, never
//! a readiness prerequisite.  This module never writes mGBA configuration and
//! never executes a discovered binary.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const MGBA_MAX_PROFILES: usize = 16;
pub const MGBA_MAX_CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MgbaInstallationType {
    Native,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgbaBiosState {
    NotConfigured,
    PresentUnverified,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaConfigInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub bios_path: Option<PathBuf>,
    pub bios: MgbaBiosState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaExecutable {
    pub path: PathBuf,
    pub installation_type: MgbaInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaProfile {
    pub profile_id: String,
    pub installation_type: MgbaInstallationType,
    pub configuration_path: PathBuf,
    pub config_path: Option<PathBuf>,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<MgbaExecutable>,
    pub config: Option<MgbaConfigInspection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaProfileDiscovery {
    pub profiles: Vec<MgbaProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}

impl MgbaProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, MgbaDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(MgbaDiscoveryError::HomeUnavailable)?;
        let xdg_config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Ok(Self {
            home,
            xdg_config_home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgbaDiscoveryError {
    HomeUnavailable,
}
impl std::fmt::Display for MgbaDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set")
    }
}
impl std::error::Error for MgbaDiscoveryError {}

fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    regular(path) && fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    regular(path)
}

fn executables(roots: &MgbaProfileDiscoveryRoots) -> Vec<MgbaExecutable> {
    let mut paths: Vec<(PathBuf, MgbaInstallationType)> = roots
        .explicit_executables
        .iter()
        .cloned()
        .map(|p| (p, MgbaInstallationType::Explicit))
        .collect();
    if let Some(path_env) = env::var_os("PATH") {
        for dir in env::split_paths(&path_env) {
            paths.push((dir.join("mgba"), MgbaInstallationType::Native));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|(p, _)| executable(p))
        .map(|(path, installation_type)| MgbaExecutable {
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|s| parse_mgba_version(s)),
            path,
            installation_type,
        })
        .collect()
}

fn config_path(root: &Path) -> Option<PathBuf> {
    [root.join("config.ini"), root.join("config.ini.bak")]
        .into_iter()
        .find(|p| regular(p))
}

fn inspect_config(path: &Path) -> MgbaConfigInspection {
    let bytes = fs::read(path).ok();
    let readable = bytes
        .as_ref()
        .is_some_and(|b| b.len() as u64 <= MGBA_MAX_CONFIG_BYTES);
    let text = bytes
        .filter(|b| b.len() as u64 <= MGBA_MAX_CONFIG_BYTES)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let bios_path = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("bios"))
        .map(|(_, value)| PathBuf::from(value.trim().trim_matches('"')));
    let bios = match &bios_path {
        None => MgbaBiosState::NotConfigured,
        Some(path) if regular(path) => MgbaBiosState::PresentUnverified,
        Some(_) => MgbaBiosState::Missing,
    };
    MgbaConfigInspection {
        path: path.to_path_buf(),
        exists: true,
        readable,
        bios_path,
        bios,
    }
}

fn profile(
    root: PathBuf,
    installation_type: MgbaInstallationType,
    all: &[MgbaExecutable],
) -> MgbaProfile {
    let config_path = config_path(&root);
    let config = config_path.as_ref().map(|p| inspect_config(p));
    let matching: Vec<_> = all
        .iter()
        .filter(|e| {
            e.installation_type == installation_type
                || installation_type == MgbaInstallationType::Explicit
        })
        .cloned()
        .collect();
    let eligible = !matching.is_empty() && config.as_ref().is_none_or(|c| c.readable);
    let blocker = (!eligible).then(|| {
        if matching.is_empty() {
            "no safe mGBA executable was discovered".into()
        } else {
            "mGBA configuration is unreadable or oversized".into()
        }
    });
    MgbaProfile {
        profile_id: format!("mgba:{}", root.display()),
        installation_type,
        configuration_path: root,
        config_path,
        eligible,
        blocker,
        executable_candidates: matching,
        config,
    }
}

pub fn discover_mgba_profiles(roots: &MgbaProfileDiscoveryRoots) -> MgbaProfileDiscovery {
    let mut candidates = vec![(
        roots.xdg_config_home.join("mgba"),
        MgbaInstallationType::Native,
    )];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, MgbaInstallationType::Portable)),
    );
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, MgbaInstallationType::Explicit)),
    );
    candidates.sort();
    candidates.dedup_by(|a, b| a.0 == b.0);
    let all = executables(roots);
    let profiles = candidates
        .into_iter()
        .filter(|(p, k)| {
            p.is_dir()
                || matches!(
                    k,
                    MgbaInstallationType::Explicit | MgbaInstallationType::Portable
                )
        })
        .take(MGBA_MAX_PROFILES)
        .map(|(p, k)| profile(p, k, &all))
        .collect();
    MgbaProfileDiscovery {
        profiles,
        complete: true,
    }
}

/// Parses bounded version output without executing a process.
pub fn parse_mgba_version(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let start = lower.find("mgba")? + 4;
    let tail = output[start..].trim_start().trim_start_matches(['v', 'V']);
    let value: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!value.is_empty() && value.starts_with(|c: char| c.is_ascii_digit())).then_some(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgbaLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    AmbiguousExecutable,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaLaunchBlocker {
    pub kind: MgbaLaunchBlockerKind,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MgbaGameRequest {
    pub verified_game_key: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaNativeLaunchBinding {
    pub executable: PathBuf,
}

pub fn resolve_mgba_native_launch_binding(
    profile: &MgbaProfile,
) -> Result<MgbaNativeLaunchBinding, MgbaLaunchBlocker> {
    if !profile.eligible {
        return Err(MgbaLaunchBlocker {
            kind: MgbaLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "profile is not eligible".into()),
        });
    }
    let valid: Vec<_> = profile
        .executable_candidates
        .iter()
        .filter(|e| {
            e.installation_type == profile.installation_type
                || profile.installation_type == MgbaInstallationType::Explicit
        })
        .filter(|e| executable(&e.path))
        .collect();
    match valid.as_slice() {
        [one] => Ok(MgbaNativeLaunchBinding {
            executable: one.path.clone(),
        }),
        [] => Err(MgbaLaunchBlocker {
            kind: MgbaLaunchBlockerKind::ExecutableMissing,
            detail: "no safe executable matches this profile".into(),
        }),
        _ => Err(MgbaLaunchBlocker {
            kind: MgbaLaunchBlockerKind::AmbiguousExecutable,
            detail: "more than one safe executable matches this profile".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    #[cfg(unix)]
    fn mark_exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut m = fs::metadata(p).unwrap().permissions();
        m.set_mode(0o755);
        fs::set_permissions(p, m).unwrap();
    }
    #[test]
    fn version_is_bounded_and_optional() {
        assert_eq!(parse_mgba_version("mGBA 0.10.3"), Some("0.10.3".into()));
        assert_eq!(parse_mgba_version("unknown"), None);
    }
    #[test]
    fn discovers_explicit_executable_and_optional_bios() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir_all(&root).unwrap();
        let exe = d.path().join("mgba");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let bios = d.path().join("gba.bin");
        fs::write(&bios, b"bios").unwrap();
        fs::write(
            root.join("config.ini"),
            format!("bios = {}\n", bios.display()),
        )
        .unwrap();
        let roots = MgbaProfileDiscoveryRoots {
            home: d.path().into(),
            xdg_config_home: d.path().join("none"),
            explicit_configuration_roots: vec![root],
            portable_configuration_roots: vec![],
            explicit_executables: vec![exe],
            known_version_outputs: BTreeMap::new(),
        };
        let p = &discover_mgba_profiles(&roots).profiles[0];
        assert!(p.eligible);
        assert_eq!(
            p.config.as_ref().unwrap().bios,
            MgbaBiosState::PresentUnverified
        );
        assert!(resolve_mgba_native_launch_binding(p).is_ok());
    }
}
