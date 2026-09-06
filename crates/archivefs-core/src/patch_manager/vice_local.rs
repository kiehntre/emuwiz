//! Bounded, read-only VICE C64 executable discovery.
//!
//! VICE's current manual names `x64` as the fast C64 emulator and `x64sc` as
//! the accurate C64 emulator. Both are exposed as separate exact profiles:
//! discovery never silently exchanges one for the other. VICE obtains its C64
//! kernal/basic/chargen and drive ROMs through its normal installed system-file
//! search path, so this adapter neither copies nor models external firmware.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const VICE_MAX_EXPLICIT_EXECUTABLES: usize = 16;
const NATIVE_EXECUTABLES: &[&str] = &["x64sc", "x64"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ViceInstallationType {
    Native,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ViceC64ExecutableKind {
    X64sc,
    X64,
}

impl ViceC64ExecutableKind {
    fn from_path(path: &Path) -> Option<Self> {
        match path.file_name()?.to_str()? {
            "x64sc" => Some(Self::X64sc),
            "x64" => Some(Self::X64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceExecutable {
    pub path: PathBuf,
    pub installation_type: ViceInstallationType,
    pub kind: ViceC64ExecutableKind,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceProfile {
    pub profile_id: String,
    pub installation_type: ViceInstallationType,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable: Option<ViceExecutable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceProfileDiscovery {
    pub profiles: Vec<ViceProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViceProfileDiscoveryRoots {
    pub explicit_executables: Vec<PathBuf>,
    pub path_env: Option<std::ffi::OsString>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}

impl ViceProfileDiscoveryRoots {
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

pub fn parse_vice_version(output: &str) -> Option<String> {
    let start = output.to_ascii_lowercase().find("vice")? + 4;
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
    installation_type: ViceInstallationType,
    versions: &BTreeMap<PathBuf, String>,
) -> ViceProfile {
    let kind = ViceC64ExecutableKind::from_path(&path);
    let safe = kind.is_some() && executable(&path);
    let executable = safe.then(|| ViceExecutable {
        version: versions.get(&path).and_then(|v| parse_vice_version(v)),
        kind: kind.unwrap(),
        path: path.clone(),
        installation_type,
    });
    ViceProfile {
        profile_id: format!(
            "vice:{}:{}",
            match installation_type {
                ViceInstallationType::Native => "native",
                ViceInstallationType::Explicit => "explicit",
            },
            path.display()
        ),
        installation_type,
        eligible: executable.is_some(),
        executable,
        blocker: (!safe).then(|| {
            "VICE requires an exact regular executable named x64sc or x64 with an execute bit"
                .into()
        }),
    }
}

pub fn discover_vice_profiles(roots: &ViceProfileDiscoveryRoots) -> ViceProfileDiscovery {
    let mut paths: Vec<(PathBuf, ViceInstallationType)> = roots
        .explicit_executables
        .iter()
        .take(VICE_MAX_EXPLICIT_EXECUTABLES)
        .cloned()
        .map(|p| (p, ViceInstallationType::Explicit))
        .collect();
    if let Some(path_env) = &roots.path_env {
        for directory in env::split_paths(path_env) {
            for name in NATIVE_EXECUTABLES {
                paths.push((directory.join(name), ViceInstallationType::Native));
            }
        }
    }
    paths.sort();
    paths.dedup();
    ViceProfileDiscovery {
        profiles: paths
            .into_iter()
            .filter(|(p, _)| p.exists())
            .map(|(p, kind)| profile(p, kind, &roots.known_version_outputs))
            .collect(),
        complete: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViceLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    ExecutableUnsafe,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceLaunchBlocker {
    pub kind: ViceLaunchBlockerKind,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceNativeLaunchBinding {
    pub executable: PathBuf,
    pub kind: ViceC64ExecutableKind,
}

pub fn resolve_vice_native_launch_binding(
    profile: &ViceProfile,
) -> Result<ViceNativeLaunchBinding, ViceLaunchBlocker> {
    if !profile.eligible {
        return Err(ViceLaunchBlocker {
            kind: ViceLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "VICE profile is ineligible".into()),
        });
    }
    let Some(executable) = profile.executable.as_ref().filter(|x| executable(&x.path)) else {
        return Err(ViceLaunchBlocker {
            kind: ViceLaunchBlockerKind::ExecutableMissing,
            detail: "the exact discovered VICE executable is no longer safe".into(),
        });
    };
    Ok(ViceNativeLaunchBinding {
        executable: executable.path.clone(),
        kind: executable.kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[cfg(unix)]
    fn mark_exec(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(path).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(path, p).unwrap();
    }
    #[test]
    fn discovers_each_supported_c64_binary_without_substitution() {
        let d = tempdir().unwrap();
        let x64sc = d.path().join("x64sc");
        let x64 = d.path().join("x64");
        fs::write(&x64sc, b"x").unwrap();
        fs::write(&x64, b"x").unwrap();
        #[cfg(unix)]
        {
            mark_exec(&x64sc);
            mark_exec(&x64);
        }
        let found = discover_vice_profiles(&ViceProfileDiscoveryRoots {
            path_env: Some(d.path().as_os_str().to_owned()),
            ..Default::default()
        });
        assert_eq!(found.profiles.iter().filter(|p| p.eligible).count(), 2);
        assert_ne!(found.profiles[0].profile_id, found.profiles[1].profile_id);
    }
    #[test]
    fn explicit_safe_binary_is_accepted_and_non_executable_is_refused() {
        let d = tempdir().unwrap();
        let exe = d.path().join("x64sc");
        fs::write(&exe, b"x").unwrap();
        let roots = ViceProfileDiscoveryRoots {
            explicit_executables: vec![exe.clone()],
            ..Default::default()
        };
        assert!(!discover_vice_profiles(&roots).profiles[0].eligible);
        #[cfg(unix)]
        mark_exec(&exe);
        assert!(discover_vice_profiles(&roots).profiles[0].eligible);
    }
    #[test]
    fn unknown_explicit_name_is_never_accepted() {
        let d = tempdir().unwrap();
        let exe = d.path().join("vice");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        assert!(
            !discover_vice_profiles(&ViceProfileDiscoveryRoots {
                explicit_executables: vec![exe],
                ..Default::default()
            })
            .profiles[0]
                .eligible
        );
    }
}
