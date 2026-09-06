//! Bounded, read-only native DeSmuME discovery.
//!
//! Current upstream command-line parsing accepts one positional NDS file. The
//! normal Linux GUI executable is `desmume`; V1 deliberately does not treat
//! the separate CLI frontend, Flatpak, or an AppImage as interchangeable.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const DESMUME_MAX_EXPLICIT_EXECUTABLES: usize = 16;
const NATIVE_EXECUTABLE: &str = "desmume";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DesmumeInstallationType {
    Native,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeExecutable {
    pub path: PathBuf,
    pub installation_type: DesmumeInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeProfile {
    pub profile_id: String,
    pub installation_type: DesmumeInstallationType,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable: Option<DesmumeExecutable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeProfileDiscovery {
    pub profiles: Vec<DesmumeProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesmumeProfileDiscoveryRoots {
    pub explicit_executables: Vec<PathBuf>,
    pub path_env: Option<std::ffi::OsString>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}
impl DesmumeProfileDiscoveryRoots {
    pub fn from_environment() -> Self {
        Self {
            path_env: env::var_os("PATH"),
            ..Self::default()
        }
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

pub fn parse_desmume_version(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let start = lower.find("desmume")? + "desmume".len();
    let version: String = output[start..]
        .trim_start()
        .trim_start_matches(['-', 'v', 'V'])
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!version.is_empty()).then_some(version)
}

fn profile(
    path: PathBuf,
    installation_type: DesmumeInstallationType,
    versions: &BTreeMap<PathBuf, String>,
) -> DesmumeProfile {
    let safe = path.file_name().and_then(|name| name.to_str()) == Some(NATIVE_EXECUTABLE)
        && executable(&path);
    let executable = safe.then(|| DesmumeExecutable {
        version: versions
            .get(&path)
            .and_then(|value| parse_desmume_version(value)),
        path: path.clone(),
        installation_type,
    });
    DesmumeProfile {
        profile_id: format!(
            "desmume:{}:{}",
            match installation_type {
                DesmumeInstallationType::Native => "native",
                DesmumeInstallationType::Explicit => "explicit",
            },
            path.display()
        ),
        installation_type,
        eligible: executable.is_some(),
        executable,
        blocker: (!safe).then(|| {
            "DeSmuME requires an exact regular executable named desmume with an execute bit".into()
        }),
    }
}

pub fn discover_desmume_profiles(roots: &DesmumeProfileDiscoveryRoots) -> DesmumeProfileDiscovery {
    let mut paths: Vec<(PathBuf, DesmumeInstallationType)> = roots
        .explicit_executables
        .iter()
        .take(DESMUME_MAX_EXPLICIT_EXECUTABLES)
        .cloned()
        .map(|p| (p, DesmumeInstallationType::Explicit))
        .collect();
    if let Some(path_env) = &roots.path_env {
        paths.extend(env::split_paths(path_env).map(|directory| {
            (
                directory.join(NATIVE_EXECUTABLE),
                DesmumeInstallationType::Native,
            )
        }));
    }
    paths.sort();
    paths.dedup();
    DesmumeProfileDiscovery {
        profiles: paths
            .into_iter()
            .filter(|(path, _)| path.exists())
            .map(|(path, kind)| profile(path, kind, &roots.known_version_outputs))
            .collect(),
        complete: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesmumeLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeLaunchBlocker {
    pub kind: DesmumeLaunchBlockerKind,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeNativeLaunchBinding {
    pub executable: PathBuf,
}

pub fn resolve_desmume_native_launch_binding(
    profile: &DesmumeProfile,
) -> Result<DesmumeNativeLaunchBinding, DesmumeLaunchBlocker> {
    if !profile.eligible {
        return Err(DesmumeLaunchBlocker {
            kind: DesmumeLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "DeSmuME profile is ineligible".into()),
        });
    }
    let Some(executable) = profile
        .executable
        .as_ref()
        .filter(|exe| executable(&exe.path))
    else {
        return Err(DesmumeLaunchBlocker {
            kind: DesmumeLaunchBlockerKind::ExecutableMissing,
            detail: "the exact discovered DeSmuME executable is no longer safe".into(),
        });
    };
    Ok(DesmumeNativeLaunchBinding {
        executable: executable.path.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[cfg(unix)]
    fn mark_exec(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
    #[test]
    fn native_and_explicit_exact_desmume_are_discovered_without_substitution() {
        let dir = tempdir().unwrap();
        let executable = dir.path().join("desmume");
        fs::write(&executable, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&executable);
        let found = discover_desmume_profiles(&DesmumeProfileDiscoveryRoots {
            path_env: Some(dir.path().as_os_str().to_owned()),
            explicit_executables: vec![executable],
            ..Default::default()
        });
        assert_eq!(
            found
                .profiles
                .iter()
                .filter(|profile| profile.eligible)
                .count(),
            2
        );
    }
    #[test]
    fn wrong_or_non_executable_explicit_path_is_refused() {
        let dir = tempdir().unwrap();
        let executable = dir.path().join("desmume-cli");
        fs::write(&executable, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&executable);
        assert!(
            !discover_desmume_profiles(&DesmumeProfileDiscoveryRoots {
                explicit_executables: vec![executable],
                ..Default::default()
            })
            .profiles[0]
                .eligible
        );
    }
}
