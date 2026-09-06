//! Bounded discovery for the reviewed openMSX cartridge profiles.
//!
//! V1 deliberately discovers only the exact `openmsx` executable from PATH or
//! an explicitly supplied path. Machine selection is bound by the launch
//! command (`C-BIOS_MSX1`/`C-BIOS_MSX2`); no openMSX configuration is read or
//! written here.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMsxInstallationType {
    Native,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMsxExecutable {
    pub path: PathBuf,
    pub installation_type: OpenMsxInstallationType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMsxProfile {
    pub profile_id: String,
    pub installation_type: OpenMsxInstallationType,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable: OpenMsxExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMsxProfileDiscovery {
    pub profiles: Vec<OpenMsxProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenMsxProfileDiscoveryRoots {
    pub explicit_executables: Vec<PathBuf>,
    pub path_env: Option<std::ffi::OsString>,
}

impl OpenMsxProfileDiscoveryRoots {
    pub fn from_environment() -> Self {
        Self {
            explicit_executables: Vec::new(),
            path_env: env::var_os("PATH"),
        }
    }
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn discover_openmsx_profiles(roots: &OpenMsxProfileDiscoveryRoots) -> OpenMsxProfileDiscovery {
    let mut paths = Vec::new();
    for path in &roots.explicit_executables {
        if executable(path) && !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    if let Some(path_env) = &roots.path_env {
        for dir in env::split_paths(path_env) {
            let path = dir.join("openmsx");
            if executable(&path) && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    let profiles = paths
        .into_iter()
        .map(|path| {
            let installation_type = if roots.explicit_executables.iter().any(|p| p == &path) {
                OpenMsxInstallationType::Explicit
            } else {
                OpenMsxInstallationType::Native
            };
            OpenMsxProfile {
                profile_id: format!("openmsx:{}", path.display()),
                installation_type,
                eligible: true,
                blocker: None,
                executable: OpenMsxExecutable {
                    path,
                    installation_type,
                },
            }
        })
        .collect();
    OpenMsxProfileDiscovery {
        profiles,
        complete: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn discovers_path_and_explicit_openmsx_without_config_access() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let executable = dir.path().join("openmsx");
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        let discovery = discover_openmsx_profiles(&OpenMsxProfileDiscoveryRoots {
            explicit_executables: vec![executable.clone()],
            path_env: Some(path),
        });
        assert_eq!(discovery.profiles.len(), 1);
        assert!(discovery.profiles[0].eligible);
        assert_eq!(discovery.profiles[0].executable.path, executable);
        assert_eq!(
            discovery.profiles[0].installation_type,
            OpenMsxInstallationType::Explicit
        );
    }

    #[test]
    fn missing_executable_is_not_discovered() {
        let discovery = discover_openmsx_profiles(&OpenMsxProfileDiscoveryRoots {
            explicit_executables: vec![PathBuf::from("/missing/openmsx")],
            path_env: None,
        });
        assert!(discovery.profiles.is_empty());
    }
}
