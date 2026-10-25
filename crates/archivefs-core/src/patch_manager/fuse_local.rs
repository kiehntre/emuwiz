//! Bounded, read-only discovery for the native Fuse ZX Spectrum emulator.
//!
//! This module only discovers an exact `fuse` executable (or an explicit path)
//! and never starts it or writes Fuse configuration.  Fuse has no separate
//! BIOS requirement for normal tape/snapshot loading.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const FUSE_MAX_PROFILES: usize = 16;
pub const FUSE_NATIVE_BINARY_NAMES: &[&str] = &["fuse"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FuseInstallationType {
    Native,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuseExecutable {
    pub path: PathBuf,
    pub installation_type: FuseInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuseProfile {
    pub profile_id: String,
    pub installation_type: FuseInstallationType,
    pub executable: PathBuf,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuseProfileDiscovery {
    pub profiles: Vec<FuseProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FuseProfileDiscoveryRoots {
    pub explicit_executables: Vec<PathBuf>,
    pub path_override: Option<std::ffi::OsString>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}

impl FuseProfileDiscoveryRoots {
    pub fn from_environment() -> Self {
        Self::default()
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

fn parse_fuse_version(raw: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        let marker = lower.find("fuse")?;
        let tail = line[marker..]
            .split_whitespace()
            .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
        Some(
            tail.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-')
                .to_string(),
        )
    })
}

fn candidates(roots: &FuseProfileDiscoveryRoots) -> Vec<(PathBuf, FuseInstallationType)> {
    let mut out = roots
        .explicit_executables
        .iter()
        .take(FUSE_MAX_PROFILES)
        .cloned()
        .map(|p| (p, FuseInstallationType::Explicit))
        .collect::<Vec<_>>();
    if let Some(path) = roots.path_override.clone().or_else(|| env::var_os("PATH")) {
        for dir in env::split_paths(&path) {
            for name in FUSE_NATIVE_BINARY_NAMES {
                out.push((dir.join(name), FuseInstallationType::Native));
            }
        }
    }
    out
}

pub fn discover_fuse_profiles(roots: &FuseProfileDiscoveryRoots) -> FuseProfileDiscovery {
    let mut seen = std::collections::BTreeSet::new();
    let mut profiles = Vec::new();
    for (path, installation_type) in candidates(roots) {
        if !seen.insert(path.clone()) {
            continue;
        }
        if installation_type == FuseInstallationType::Native && fs::symlink_metadata(&path).is_err()
        {
            continue;
        }
        let eligible = executable(&path);
        let blocker = (!eligible).then(|| {
            if regular(&path) {
                "Fuse executable is not marked executable".to_string()
            } else {
                "Fuse path is not a regular file".to_string()
            }
        });
        profiles.push(FuseProfile {
            profile_id: format!("fuse:{}", path.display()),
            installation_type,
            executable: path.clone(),
            eligible,
            blocker,
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|v| parse_fuse_version(v)),
        });
        if profiles.len() >= FUSE_MAX_PROFILES {
            break;
        }
    }
    FuseProfileDiscovery {
        profiles,
        complete: true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuseLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuseLaunchBlocker {
    pub kind: FuseLaunchBlockerKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuseNativeLaunchBinding {
    pub executable: PathBuf,
}

pub fn resolve_fuse_native_launch_binding(
    profile: &FuseProfile,
) -> Result<FuseNativeLaunchBinding, FuseLaunchBlocker> {
    if !profile.eligible {
        return Err(FuseLaunchBlocker {
            kind: FuseLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "Fuse profile is not eligible".into()),
        });
    }
    if !regular(&profile.executable) {
        return Err(FuseLaunchBlocker {
            kind: FuseLaunchBlockerKind::ExecutableMissing,
            detail: "Fuse executable is missing or not a regular file".into(),
        });
    }
    #[cfg(unix)]
    if !executable(&profile.executable) {
        return Err(FuseLaunchBlocker {
            kind: FuseLaunchBlockerKind::ExecutableNotExecutable,
            detail: "Fuse executable is not executable".into(),
        });
    }
    Ok(FuseNativeLaunchBinding {
        executable: profile.executable.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn explicit_executable_is_discovered_and_version_is_parsed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("fuse");
        fs::write(&path, b"#! /bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let mut roots = FuseProfileDiscoveryRoots::default();
        roots.explicit_executables.push(path.clone());
        roots
            .known_version_outputs
            .insert(path.clone(), "Fuse 1.6.0\n".into());
        let d = discover_fuse_profiles(&roots);
        assert_eq!(d.profiles.len(), 1);
        assert!(d.profiles[0].eligible);
        assert_eq!(d.profiles[0].version.as_deref(), Some("1.6.0"));
        assert!(resolve_fuse_native_launch_binding(&d.profiles[0]).is_ok());
    }
}
