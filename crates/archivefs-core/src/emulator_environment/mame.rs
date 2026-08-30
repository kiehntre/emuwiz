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

use super::{ExecutableProbe, ReadOnlyHostFilesystem};

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
}
