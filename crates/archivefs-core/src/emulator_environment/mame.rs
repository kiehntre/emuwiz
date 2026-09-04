//! Bounded native MAME executable discovery.
//!
//! This is deliberately the same read-only `PATH` probing pattern used by
//! RetroArch. It discovers a candidate executable only; it does not run MAME,
//! inspect a ROM set, or create a configuration. Flatpak is not guessed here:
//! the caller must supply an exact executable binding if a future shared
//! profile discovers one.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use super::{ExecutableProbe, FsProbe, ReadOnlyHostFilesystem};

/// MAME's conventional configuration filename. Inspection is explicit and
/// read-only; this adapter never creates or rewrites it.
pub const MAME_CONFIG_FILE_NAME: &str = "mame.ini";
const MAX_MAME_CONFIG_BYTES: usize = 64 * 1024;

/// The path-valued portions of a MAME profile that EmuWiz can report without
/// claiming that the referenced ROMs or software are complete. Values are
/// retained as configured strings because MAME performs its own path
/// expansion and search-path resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameProfile {
    pub config_path: PathBuf,
    pub rompath: Vec<String>,
    pub cfg_directory: Option<String>,
    pub nvram_directory: Option<String>,
    pub snapshot_directory: Option<String>,
    pub hashpath: Option<String>,
}

impl MameProfile {
    /// A configured `rompath` is launch arrangement evidence, not a claim
    /// that every set/dependency is present.
    pub fn rom_search_path_configured(&self) -> bool {
        !self.rompath.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MameProfileInspection {
    Missing,
    Unreadable,
    Present(MameProfile),
}

/// Inspect one caller-authorized `mame.ini` without invoking MAME and without
/// mutating it. MAME ini files are simple `key value` records; unknown keys
/// are intentionally ignored so this remains forward compatible.
pub fn inspect_mame_profile(
    filesystem: &dyn ReadOnlyHostFilesystem,
    config_path: &std::path::Path,
) -> MameProfileInspection {
    match filesystem.probe(config_path) {
        FsProbe::Missing => return MameProfileInspection::Missing,
        FsProbe::PresentFile => {}
        _ => return MameProfileInspection::Unreadable,
    }
    let bytes = match filesystem.read_bounded(config_path, MAX_MAME_CONFIG_BYTES) {
        super::BoundedReadResult::Ok(bytes) => bytes,
        _ => return MameProfileInspection::Unreadable,
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return MameProfileInspection::Unreadable;
    };
    let mut rompath = Vec::new();
    let mut cfg_directory = None;
    let mut nvram_directory = None;
    let mut snapshot_directory = None;
    let mut hashpath = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "rompath" => rompath.extend(
                value
                    .split(';')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string),
            ),
            "cfg_directory" => cfg_directory = Some(value.to_string()),
            "nvram_directory" => nvram_directory = Some(value.to_string()),
            "snapshot_directory" => snapshot_directory = Some(value.to_string()),
            "hashpath" => hashpath = Some(value.to_string()),
            _ => {}
        }
    }
    MameProfileInspection::Present(MameProfile {
        config_path: config_path.to_path_buf(),
        rompath,
        cfg_directory,
        nvram_directory,
        snapshot_directory,
        hashpath,
    })
}

/// Exact caller-supplied or PATH-derived MAME executable candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameExecutableDiscovery {
    pub candidates: Vec<PathBuf>,
    pub selected: Option<PathBuf>,
    pub provenance: &'static str,
}

/// Inputs to the bounded MAME discovery pass. No global PATH lookup is
/// performed: the caller supplies the already-established environment value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MameDiscoveryInputs {
    pub explicit_executable: Option<PathBuf>,
    pub executable_search_path: Option<OsString>,
}

pub fn discover_mame_executable(
    filesystem: &dyn ReadOnlyHostFilesystem,
    inputs: &MameDiscoveryInputs,
) -> MameExecutableDiscovery {
    if let Some(path) = &inputs.explicit_executable {
        let candidates = (filesystem.probe_executable(path) == ExecutableProbe::RegularExecutable)
            .then(|| path.clone())
            .into_iter()
            .collect::<Vec<_>>();
        return MameExecutableDiscovery {
            selected: candidates.first().cloned(),
            candidates,
            provenance: "explicit MAME executable binding",
        };
    }

    let mut candidates = Vec::new();
    if let Some(path_value) = &inputs.executable_search_path {
        for directory_bytes in path_value.as_bytes().split(|byte| *byte == b':') {
            if directory_bytes.is_empty() {
                continue;
            }
            let directory = PathBuf::from(OsStr::from_bytes(directory_bytes));
            for name in ["mame", "mame64"] {
                let candidate = directory.join(name);
                if !candidates.contains(&candidate)
                    && filesystem.probe_executable(&candidate) == ExecutableProbe::RegularExecutable
                {
                    candidates.push(candidate);
                }
            }
        }
    }
    let selected = (candidates.len() == 1).then(|| candidates[0].clone());
    MameExecutableDiscovery {
        selected,
        candidates,
        provenance: "bounded MAME executable PATH discovery",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_environment::HostReadOnlyFilesystem;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn explicit_executable_is_selected_only_when_executable() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("mame");
        std::fs::write(&executable, b"fake").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let found = discover_mame_executable(
            &HostReadOnlyFilesystem,
            &MameDiscoveryInputs {
                explicit_executable: Some(executable.clone()),
                executable_search_path: None,
            },
        );
        assert_eq!(found.selected, Some(executable));
        assert_eq!(found.candidates.len(), 1);
    }

    #[test]
    fn path_discovery_requires_a_unique_mame_candidate() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("mame64");
        std::fs::write(&executable, b"fake").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let found = discover_mame_executable(
            &HostReadOnlyFilesystem,
            &MameDiscoveryInputs {
                explicit_executable: None,
                executable_search_path: Some(directory.path().as_os_str().to_os_string()),
            },
        );
        assert_eq!(found.selected, Some(executable));
    }

    #[test]
    fn two_candidates_are_not_silently_selected() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["mame", "mame64"] {
            let executable = directory.path().join(name);
            std::fs::write(&executable, b"fake").unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = discover_mame_executable(
            &HostReadOnlyFilesystem,
            &MameDiscoveryInputs {
                explicit_executable: None,
                executable_search_path: Some(directory.path().as_os_str().to_os_string()),
            },
        );
        assert_eq!(found.selected, None);
        assert_eq!(found.candidates.len(), 2);
    }

    #[test]
    fn mame_ini_is_inspected_read_only_and_rompath_is_not_completeness() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join(MAME_CONFIG_FILE_NAME);
        std::fs::write(
            &config,
            "rompath /roms/main;/roms/software\ncfg_directory cfg\nnvram_directory nvram\nsnapshot_directory snaps\nhashpath hash\n",
        )
        .unwrap();
        let inspection = inspect_mame_profile(&HostReadOnlyFilesystem, &config);
        let MameProfileInspection::Present(profile) = inspection else {
            panic!("profile should parse")
        };
        assert_eq!(profile.rompath, vec!["/roms/main", "/roms/software"]);
        assert!(profile.rom_search_path_configured());
        assert_eq!(
            std::fs::read_to_string(config).unwrap(),
            "rompath /roms/main;/roms/software\ncfg_directory cfg\nnvram_directory nvram\nsnapshot_directory snaps\nhashpath hash\n"
        );
    }

    #[test]
    fn missing_mame_ini_is_distinct_from_an_empty_rompath() {
        let directory = tempfile::tempdir().unwrap();
        let missing = inspect_mame_profile(
            &HostReadOnlyFilesystem,
            &directory.path().join(MAME_CONFIG_FILE_NAME),
        );
        assert_eq!(missing, MameProfileInspection::Missing);
        let config = directory.path().join(MAME_CONFIG_FILE_NAME);
        std::fs::write(&config, "verbose 0\n").unwrap();
        let MameProfileInspection::Present(profile) =
            inspect_mame_profile(&HostReadOnlyFilesystem, &config)
        else {
            panic!("profile should parse")
        };
        assert!(!profile.rom_search_path_configured());
    }
}
