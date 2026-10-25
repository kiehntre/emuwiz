//! Read-only discovery for explicit XRoar Dragon/CoCo machine profiles.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const XROAR_EXECUTABLE_NAME: &str = "xroar";
pub const XROAR_MACHINE_ENV: &str = "EMUWIZ_XROAR_MACHINE";
pub const XROAR_FIRMWARE_DIRECTORY_ENV: &str = "EMUWIZ_XROAR_FIRMWARE_DIRECTORY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum XRoarMachine {
    Dragon32,
    Dragon64,
    CoCo2,
    CoCo3,
}

impl XRoarMachine {
    pub const ALL: [Self; 4] = [Self::Dragon32, Self::Dragon64, Self::CoCo2, Self::CoCo3];

    pub const fn flag(self) -> &'static str {
        match self {
            Self::Dragon32 => "dragon32",
            Self::Dragon64 => "dragon64",
            Self::CoCo2 => "coco2",
            Self::CoCo3 => "coco3",
        }
    }

    pub const fn platform_id(self) -> &'static str {
        "Dragon / Tandy CoCo"
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "dragon32" | "dragon-32" => Some(Self::Dragon32),
            "dragon64" | "dragon-64" => Some(Self::Dragon64),
            "coco2" | "coco-2" => Some(Self::CoCo2),
            "coco3" | "coco-3" => Some(Self::CoCo3),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XRoarFirmwareState {
    Verified,
    PresentUnverified,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum XRoarInstallationType {
    Native,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarExecutable {
    pub path: PathBuf,
    pub installation_type: XRoarInstallationType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarProfile {
    pub profile_id: String,
    pub machine: XRoarMachine,
    pub installation_type: XRoarInstallationType,
    pub executable: XRoarExecutable,
    pub firmware: XRoarFirmwareState,
    pub eligible: bool,
    pub blocker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarProfileDiscovery {
    pub profiles: Vec<XRoarProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarExplicitProfile {
    pub executable: PathBuf,
    pub machine: XRoarMachine,
    pub firmware: XRoarFirmwareState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarProfileDiscoveryRoots {
    pub explicit_profiles: Vec<XRoarExplicitProfile>,
    pub path_env: Option<std::ffi::OsString>,
    pub selected_machine: Option<XRoarMachine>,
    pub firmware: XRoarFirmwareState,
    /// Explicit directory used for machine-specific ROM checks and command binding.
    pub firmware_directory: Option<PathBuf>,
}

impl Default for XRoarProfileDiscoveryRoots {
    fn default() -> Self {
        Self {
            explicit_profiles: Vec::new(),
            path_env: None,
            selected_machine: None,
            firmware_directory: None,
            firmware: XRoarFirmwareState::Unknown,
        }
    }
}

impl XRoarProfileDiscoveryRoots {
    pub fn from_environment() -> Self {
        let selected_machine = env::var(XROAR_MACHINE_ENV)
            .ok()
            .and_then(|value| XRoarMachine::parse(&value));
        let firmware_directory = env::var_os(XROAR_FIRMWARE_DIRECTORY_ENV)
            .map(PathBuf::from)
            .and_then(|path| fs::canonicalize(path).ok());
        let firmware = match (selected_machine, firmware_directory.as_deref()) {
            (Some(machine), Some(path)) => inspect_xroar_firmware(path, machine),
            (_, _) if env::var_os(XROAR_FIRMWARE_DIRECTORY_ENV).is_some() => {
                XRoarFirmwareState::Missing
            }
            _ => XRoarFirmwareState::Unknown,
        };
        Self {
            path_env: env::var_os("PATH"),
            selected_machine,
            firmware,
            firmware_directory,
            ..Self::default()
        }
    }
}

/// Canonical ROM names/sizes from the XRoar manual. This checks presence and
/// readability only; it never claims a verified BIOS hash.
/// https://www.6809.org.uk/xroar/doc/xroar.shtml#Prerequisites
impl XRoarMachine {
    pub(crate) fn cassette_roms(self) -> &'static [(&'static str, &'static str, u64)] {
        match self {
            XRoarMachine::Dragon32 => &[("-extbas", "d32.rom", 16384)],
            XRoarMachine::Dragon64 => &[
                ("-extbas", "d64_1.rom", 16384),
                ("-altbas", "d64_2.rom", 16384),
            ],
            XRoarMachine::CoCo2 => &[
                ("-bas", "bas13.rom", 8192),
                ("-extbas", "extbas11.rom", 8192),
            ],
            XRoarMachine::CoCo3 => &[("-extbas", "coco3.rom", 32768)],
        }
    }
}

fn inspect_xroar_firmware(directory: &Path, machine: XRoarMachine) -> XRoarFirmwareState {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let present = machine.cassette_roms().iter().all(|(_, name, size)| {
        let Ok(mut file) = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join(name))
        else {
            return false;
        };
        file.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.len() == *size)
            && file.read_exact(&mut [0u8; 1]).is_ok()
    });
    if present {
        XRoarFirmwareState::PresentUnverified
    } else {
        XRoarFirmwareState::Missing
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

pub fn discover_xroar_profiles(roots: &XRoarProfileDiscoveryRoots) -> XRoarProfileDiscovery {
    let mut executables = roots
        .explicit_profiles
        .iter()
        .map(|profile| (profile.executable.clone(), XRoarInstallationType::Explicit))
        .collect::<Vec<_>>();
    if let Some(path_env) = &roots.path_env {
        executables.extend(env::split_paths(path_env).map(|dir| {
            (
                dir.join(XROAR_EXECUTABLE_NAME),
                XRoarInstallationType::Native,
            )
        }));
    }
    executables.sort();
    executables.dedup();

    let mut profiles = Vec::new();
    for (executable_path, installation_type) in executables {
        if !executable(&executable_path) {
            continue;
        }
        let machine_profiles = roots
            .explicit_profiles
            .iter()
            .filter(|profile| profile.executable == executable_path)
            .map(|profile| (profile.machine, profile.firmware))
            .collect::<Vec<_>>();
        let machine_profiles = if machine_profiles.is_empty() {
            roots
                .selected_machine
                .map(|machine| vec![(machine, roots.firmware)])
                .unwrap_or_else(|| {
                    XRoarMachine::ALL
                        .into_iter()
                        .map(|machine| (machine, roots.firmware))
                        .collect()
                })
        } else {
            machine_profiles
        };
        for (machine, firmware) in machine_profiles {
            let firmware = roots
                .firmware_directory
                .as_deref()
                .map_or(firmware, |directory| {
                    inspect_xroar_firmware(directory, machine)
                });
            let selected = roots.selected_machine;
            let blocker = if selected.is_none() {
                Some("an explicit Dragon/CoCo machine must be selected".to_string())
            } else if selected != Some(machine) {
                Some("profile machine does not match the selected machine".to_string())
            } else {
                None
            };
            profiles.push(XRoarProfile {
                profile_id: format!("xroar:{}:{}", executable_path.display(), machine.flag()),
                machine,
                installation_type,
                executable: XRoarExecutable {
                    path: executable_path.clone(),
                    installation_type,
                },
                firmware,
                eligible: blocker.is_none(),
                blocker,
            });
        }
    }
    XRoarProfileDiscovery {
        profiles,
        complete: true,
    }
}

pub fn resolve_xroar_native_launch_binding(
    profile: &XRoarProfile,
) -> Result<XRoarExecutable, String> {
    if !profile.eligible {
        return Err(profile
            .blocker
            .clone()
            .unwrap_or_else(|| "XRoar profile is not eligible".into()));
    }
    if !executable(&profile.executable.path) {
        return Err("XRoar executable is missing or not executable".into());
    }
    Ok(profile.executable.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn cassette_bios_must_be_readable_regular_and_machine_specific() {
        use std::os::unix::fs::symlink;
        let directory = tempdir().unwrap();
        for machine in XRoarMachine::ALL {
            let root = directory.path().join(machine.flag());
            fs::create_dir(&root).unwrap();
            assert_eq!(
                inspect_xroar_firmware(&root, machine),
                XRoarFirmwareState::Missing
            );
            for (_, name, size) in machine.cassette_roms() {
                fs::write(root.join(name), vec![0u8; *size as usize]).unwrap();
            }
            assert_eq!(
                inspect_xroar_firmware(&root, machine),
                XRoarFirmwareState::PresentUnverified
            );
            let (_, name, _) = machine.cassette_roms()[0];
            let original = root.join(name);
            let moved = root.join("saved.rom");
            fs::rename(&original, &moved).unwrap();
            symlink(&moved, &original).unwrap();
            assert_eq!(
                inspect_xroar_firmware(&root, machine),
                XRoarFirmwareState::Missing
            );
            fs::remove_file(&original).unwrap();
            fs::write(&original, b"truncated").unwrap();
            assert_eq!(
                inspect_xroar_firmware(&root, machine),
                XRoarFirmwareState::Missing
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn discovers_selected_machine_from_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let executable = dir.path().join(XROAR_EXECUTABLE_NAME);
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let discovery = discover_xroar_profiles(&XRoarProfileDiscoveryRoots {
            path_env: Some(std::env::join_paths([dir.path()]).unwrap()),
            selected_machine: Some(XRoarMachine::CoCo3),
            firmware: XRoarFirmwareState::PresentUnverified,
            ..Default::default()
        });
        assert_eq!(discovery.profiles.len(), 1);
        assert!(discovery.profiles.iter().all(|profile| profile.eligible));
        assert!(
            discovery
                .profiles
                .iter()
                .all(|profile| profile.machine == XRoarMachine::CoCo3)
        );
    }

    #[cfg(unix)]
    #[test]
    fn no_machine_selection_is_visible_but_blocked() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let executable = dir.path().join(XROAR_EXECUTABLE_NAME);
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let discovery = discover_xroar_profiles(&XRoarProfileDiscoveryRoots {
            explicit_profiles: vec![XRoarExplicitProfile {
                executable,
                machine: XRoarMachine::Dragon32,
                firmware: XRoarFirmwareState::Unknown,
            }],
            ..Default::default()
        });
        assert_eq!(discovery.profiles.len(), 1);
        assert!(!discovery.profiles[0].eligible);
    }
}
