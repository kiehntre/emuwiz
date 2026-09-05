//! Bounded, read-only discovery for the upstream Mesen 2 Linux program.
//!
//! Evidence: SourMesen/Mesen2's current Linux workflow publishes `Mesen`,
//! its AppImage also embeds `Mesen`, and `CommandLineHelper` accepts an
//! existing file path as content.  The AppImage is intentionally not a
//! candidate here: this adapter never changes execute permission and this
//! repository has no Mesen-specific managed-AppImage trust path.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const MESEN_MAX_PROFILES: usize = 16;
pub const MESEN_MAX_CONFIG_BYTES: u64 = 256 * 1024;
const EXECUTABLE_NAMES: &[&str] = &["Mesen"];
const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MesenInstallationType {
    Native,
    Explicit,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenExecutable {
    pub path: PathBuf,
    pub installation_type: MesenInstallationType,
    pub version: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenConfigInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub oversized: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenProfile {
    pub profile_id: String,
    pub installation_type: MesenInstallationType,
    pub configuration_path: PathBuf,
    pub config_path: PathBuf,
    pub config: MesenConfigInspection,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<MesenExecutable>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenProfileDiscovery {
    pub profiles: Vec<MesenProfile>,
    pub complete: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MesenDiscoveryError {
    HomeUnavailable,
}
impl std::fmt::Display for MesenDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set")
    }
}
impl std::error::Error for MesenDiscoveryError {}
impl MesenProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, MesenDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(MesenDiscoveryError::HomeUnavailable)?;
        let xdg_config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Ok(Self {
            home,
            xdg_config_home,
            explicit_configuration_roots: vec![],
            explicit_executables: vec![],
            known_version_outputs: BTreeMap::new(),
        })
    }
}
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
fn inspect_config(path: PathBuf) -> MesenConfigInspection {
    let meta = fs::symlink_metadata(&path).ok();
    let exists = meta
        .as_ref()
        .is_some_and(|m| m.is_file() && !m.file_type().is_symlink());
    let oversized = meta
        .as_ref()
        .is_some_and(|m| m.len() > MESEN_MAX_CONFIG_BYTES);
    let readable = exists && !oversized && fs::read(&path).is_ok();
    MesenConfigInspection {
        path,
        exists,
        readable,
        oversized,
    }
}
fn executables(roots: &MesenProfileDiscoveryRoots) -> Vec<MesenExecutable> {
    let mut paths: Vec<(PathBuf, MesenInstallationType)> = roots
        .explicit_executables
        .iter()
        .cloned()
        .map(|p| (p, MesenInstallationType::Explicit))
        .collect();
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            for name in EXECUTABLE_NAMES {
                paths.push((dir.join(name), MesenInstallationType::Native));
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|(p, _)| executable(p))
        .map(|(path, installation_type)| MesenExecutable {
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|s| parse_mesen_version(s)),
            path,
            installation_type,
        })
        .collect()
}
pub fn parse_mesen_version(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let start = lower.find("mesen")? + 5;
    let value: String = output[start..]
        .trim_start()
        .trim_start_matches(['v', 'V', '2'])
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!value.is_empty()).then_some(value)
}
pub fn discover_mesen_profiles(roots: &MesenProfileDiscoveryRoots) -> MesenProfileDiscovery {
    let mut candidates = vec![(
        roots.xdg_config_home.join("Mesen2"),
        MesenInstallationType::Native,
    )];
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, MesenInstallationType::Explicit)),
    );
    candidates.sort();
    candidates.dedup_by(|a, b| a.0 == b.0);
    let all = executables(roots);
    let profiles = candidates.into_iter().filter(|(root, kind)| root.is_dir() || *kind == MesenInstallationType::Explicit || !all.is_empty()).take(MESEN_MAX_PROFILES).map(|(root, kind)| {
        let config_path = root.join(SETTINGS_FILE); let config = inspect_config(config_path.clone());
        let matching: Vec<_> = all.iter().filter(|e| e.installation_type == kind || kind == MesenInstallationType::Explicit).cloned().collect();
        let eligible = !matching.is_empty() && config.readable;
        let blocker = (!eligible).then(|| if matching.is_empty() { "no safe Mesen executable was discovered".into() } else { "Mesen settings.json is missing, unreadable, or oversized; launching would open its first-run configuration wizard".into() });
        MesenProfile { profile_id: format!("mesen:{}", root.display()), installation_type: kind, configuration_path: root, config_path, config, eligible, blocker, executable_candidates: matching }
    }).collect();
    MesenProfileDiscovery {
        profiles,
        complete: true,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MesenLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    AmbiguousExecutable,
    ExecutableUnsafe,
    ExecutableNotExecutable,
    UnsupportedInstallationType,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenLaunchBlocker {
    pub kind: MesenLaunchBlockerKind,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenNativeLaunchBinding {
    pub executable: PathBuf,
}
pub fn resolve_mesen_native_launch_binding(
    profile: &MesenProfile,
) -> Result<MesenNativeLaunchBinding, MesenLaunchBlocker> {
    if !profile.eligible {
        return Err(MesenLaunchBlocker {
            kind: MesenLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "profile is not eligible".into()),
        });
    }
    if !matches!(
        profile.installation_type,
        MesenInstallationType::Native | MesenInstallationType::Explicit
    ) {
        return Err(MesenLaunchBlocker {
            kind: MesenLaunchBlockerKind::UnsupportedInstallationType,
            detail: "unsupported Mesen installation type".into(),
        });
    }
    let valid: Vec<_> = profile
        .executable_candidates
        .iter()
        .filter(|e| {
            (e.installation_type == profile.installation_type
                || profile.installation_type == MesenInstallationType::Explicit)
                && executable(&e.path)
        })
        .collect();
    match valid.as_slice() {
        [one] => Ok(MesenNativeLaunchBinding {
            executable: one.path.clone(),
        }),
        [] => Err(MesenLaunchBlocker {
            kind: MesenLaunchBlockerKind::ExecutableMissing,
            detail: "no safe Mesen executable matches this profile".into(),
        }),
        _ => Err(MesenLaunchBlocker {
            kind: MesenLaunchBlockerKind::AmbiguousExecutable,
            detail: "more than one safe Mesen executable matches this profile".into(),
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[cfg(unix)]
    fn exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut x = fs::metadata(p).unwrap().permissions();
        x.set_mode(0o755);
        fs::set_permissions(p, x).unwrap();
    }
    fn roots(d: &Path, exe: Vec<PathBuf>) -> MesenProfileDiscoveryRoots {
        MesenProfileDiscoveryRoots {
            home: d.into(),
            xdg_config_home: d.join("config"),
            explicit_configuration_roots: vec![d.join("profile")],
            explicit_executables: exe,
            known_version_outputs: BTreeMap::new(),
        }
    }
    #[test]
    fn discovers_usable_native_mesen() {
        let d = tempdir().unwrap();
        let e = d.path().join("Mesen");
        fs::write(&e, b"x").unwrap();
        #[cfg(unix)]
        exec(&e);
        fs::create_dir_all(d.path().join("profile")).unwrap();
        fs::write(d.path().join("profile/settings.json"), b"{}").unwrap();
        let x = discover_mesen_profiles(&roots(d.path(), vec![e]));
        let usable = x
            .profiles
            .iter()
            .find(|p| p.eligible)
            .expect("a usable Mesen profile is discovered");
        assert!(resolve_mesen_native_launch_binding(usable).is_ok());
    }
    #[test]
    fn no_executable_is_found_but_unusable_profile_is_reported() {
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join("profile")).unwrap();
        fs::write(d.path().join("profile/settings.json"), b"{}").unwrap();
        let x = discover_mesen_profiles(&roots(d.path(), vec![]));
        assert!(!x.profiles[0].eligible);
        assert_eq!(
            resolve_mesen_native_launch_binding(&x.profiles[0])
                .unwrap_err()
                .kind,
            MesenLaunchBlockerKind::ProfileIneligible
        );
    }
    #[test]
    fn non_executable_is_refused() {
        let d = tempdir().unwrap();
        let e = d.path().join("Mesen");
        fs::write(&e, b"x").unwrap();
        fs::create_dir_all(d.path().join("profile")).unwrap();
        fs::write(d.path().join("profile/settings.json"), b"{}").unwrap();
        assert!(!discover_mesen_profiles(&roots(d.path(), vec![e])).profiles[0].eligible);
    }
}
