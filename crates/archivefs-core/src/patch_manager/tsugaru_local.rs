//! Read-only discovery for the standalone Tsugaru FM Towns emulator.
//!
//! Tsugaru's CUI requires an explicit ROM directory as its first argument.
//! PATH discovery therefore creates a visible-but-ineligible profile until
//! that directory is supplied through the explicit discovery roots (or the
//! `EMUWIZ_TSUGARU_ROM_DIRECTORY` environment setting used by the GUI).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const TSUGARU_EXECUTABLE_NAME: &str = "Tsugaru_CUI";
pub const TSUGARU_ROM_DIRECTORY_ENV: &str = "EMUWIZ_TSUGARU_ROM_DIRECTORY";
pub const TSUGARU_FIRMWARE_DIRECTORY_ENV: &str = "EMUWIZ_TSUGARU_FIRMWARE_DIRECTORY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsugaruInstallationType {
    Native,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsugaruFirmwareState {
    Verified,
    PresentUnverified,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsugaruExecutable {
    pub path: PathBuf,
    pub installation_type: TsugaruInstallationType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsugaruProfile {
    pub profile_id: String,
    pub installation_type: TsugaruInstallationType,
    pub executable: TsugaruExecutable,
    pub rom_directory: Option<PathBuf>,
    pub firmware: TsugaruFirmwareState,
    pub eligible: bool,
    pub blocker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsugaruProfileDiscovery {
    pub profiles: Vec<TsugaruProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsugaruProfileDiscoveryRoots {
    pub explicit_profiles: Vec<(PathBuf, PathBuf, TsugaruFirmwareState)>,
    pub path_env: Option<std::ffi::OsString>,
    pub rom_directory: Option<PathBuf>,
    pub firmware: TsugaruFirmwareState,
}

impl Default for TsugaruProfileDiscoveryRoots {
    fn default() -> Self {
        Self {
            explicit_profiles: Vec::new(),
            path_env: None,
            rom_directory: None,
            firmware: TsugaruFirmwareState::Unknown,
        }
    }
}

impl TsugaruProfileDiscoveryRoots {
    pub fn from_environment() -> Self {
        let rom_directory = env::var_os(TSUGARU_ROM_DIRECTORY_ENV).map(PathBuf::from);
        let firmware = match env::var_os(TSUGARU_FIRMWARE_DIRECTORY_ENV) {
            Some(path) if Path::new(&path).is_dir() => TsugaruFirmwareState::PresentUnverified,
            Some(_) => TsugaruFirmwareState::Missing,
            None => TsugaruFirmwareState::Unknown,
        };
        Self {
            explicit_profiles: Vec::new(),
            path_env: env::var_os("PATH"),
            rom_directory,
            firmware,
        }
    }
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| !meta.file_type().is_symlink() && meta.is_dir())
}

pub fn discover_tsugaru_profiles(roots: &TsugaruProfileDiscoveryRoots) -> TsugaruProfileDiscovery {
    let mut candidates = roots
        .explicit_profiles
        .iter()
        .map(|(executable, rom_directory, firmware)| {
            (
                executable.clone(),
                Some(rom_directory.clone()),
                TsugaruInstallationType::Explicit,
                *firmware,
            )
        })
        .collect::<Vec<_>>();
    if let Some(path_env) = &roots.path_env {
        candidates.extend(env::split_paths(path_env).map(|dir| {
            (
                dir.join(TSUGARU_EXECUTABLE_NAME),
                roots.rom_directory.clone(),
                TsugaruInstallationType::Native,
                roots.firmware,
            )
        }));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates.dedup();
    let profiles = candidates
        .into_iter()
        .filter(|(path, _, _, _)| executable(path))
        .map(|(path, rom_directory, installation_type, firmware)| {
            let blocker = if rom_directory.as_deref().is_none_or(|p| !directory(p)) {
                Some("an explicit Tsugaru ROM directory is required".to_string())
            } else {
                None
            };
            TsugaruProfile {
                profile_id: format!("tsugaru:{}", path.display()),
                installation_type,
                executable: TsugaruExecutable {
                    path,
                    installation_type,
                },
                rom_directory,
                firmware,
                eligible: blocker.is_none(),
                blocker,
            }
        })
        .collect();
    TsugaruProfileDiscovery {
        profiles,
        complete: true,
    }
}

pub fn resolve_tsugaru_native_launch_binding(
    profile: &TsugaruProfile,
) -> Result<TsugaruExecutable, String> {
    if !profile.eligible {
        return Err(profile
            .blocker
            .clone()
            .unwrap_or_else(|| "Tsugaru profile is not eligible".into()));
    }
    if !executable(&profile.executable.path) {
        return Err("Tsugaru executable is missing or not executable".into());
    }
    if profile
        .rom_directory
        .as_deref()
        .is_none_or(|path| !directory(path))
    {
        return Err("Tsugaru ROM directory is missing or unsafe".into());
    }
    Ok(profile.executable.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn discovers_tsugaru_only_with_explicit_rom_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let executable = dir.path().join(TSUGARU_EXECUTABLE_NAME);
        let roms = dir.path().join("roms");
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir(&roms).unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        let discovery = discover_tsugaru_profiles(&TsugaruProfileDiscoveryRoots {
            path_env: Some(path),
            rom_directory: Some(roms.clone()),
            ..Default::default()
        });
        assert_eq!(discovery.profiles.len(), 1);
        assert!(discovery.profiles[0].eligible);
        assert_eq!(
            discovery.profiles[0].rom_directory.as_deref(),
            Some(roms.as_path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_rom_directory_is_visible_but_blocked() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let executable = dir.path().join(TSUGARU_EXECUTABLE_NAME);
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let discovery = discover_tsugaru_profiles(&TsugaruProfileDiscoveryRoots {
            explicit_profiles: vec![(
                executable,
                dir.path().join("missing-roms"),
                TsugaruFirmwareState::Unknown,
            )],
            ..Default::default()
        });
        assert_eq!(discovery.profiles.len(), 1);
        assert!(!discovery.profiles[0].eligible);
    }
}
