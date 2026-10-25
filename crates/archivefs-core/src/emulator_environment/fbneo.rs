//! Read-only standalone FinalBurn Neo executable binding.
//!
//! The repository has strong evidence for the FBNeo DAT ecosystem, but no
//! reviewed Linux packaging convention for a standalone binary. Therefore
//! V1 accepts only an explicit caller-provided executable path. This avoids
//! confusing a RetroArch core, MAME, or an unverified PATH binary with the
//! standalone adapter.

use std::path::{Path, PathBuf};

use super::{ExecutableProbe, ReadOnlyHostFilesystem};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FbneoDiscoveryInputs {
    pub explicit_executable: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbneoExecutableDiscovery {
    pub candidates: Vec<PathBuf>,
    pub selected: Option<PathBuf>,
    pub provenance: &'static str,
}

pub fn discover_fbneo_executable(
    filesystem: &dyn ReadOnlyHostFilesystem,
    inputs: &FbneoDiscoveryInputs,
) -> FbneoExecutableDiscovery {
    let candidates = inputs
        .explicit_executable
        .as_deref()
        .filter(|path| filesystem.probe_executable(path) == ExecutableProbe::RegularExecutable)
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    FbneoExecutableDiscovery {
        selected: candidates.first().cloned(),
        candidates,
        provenance: "explicit standalone FBNeo executable binding",
    }
}

/// No reliable standalone FBNeo version command is currently evidenced by
/// the project. Keep this explicit rather than running an emulator flag that
/// could launch a frontend or mutate its state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbneoVersion {
    UnknownVersion,
}

pub fn inspect_fbneo_version() -> FbneoVersion {
    FbneoVersion::UnknownVersion
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_environment::HostReadOnlyFilesystem;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn explicit_regular_executable_is_selected() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("FinalBurn Neo");
        std::fs::write(&executable, b"fake").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let found = discover_fbneo_executable(
            &HostReadOnlyFilesystem,
            &FbneoDiscoveryInputs {
                explicit_executable: Some(executable.clone()),
            },
        );
        assert_eq!(found.selected, Some(executable));
    }

    #[test]
    fn missing_or_non_executable_binding_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fbneo");
        std::fs::write(&executable, b"fake").unwrap();
        let found = discover_fbneo_executable(
            &HostReadOnlyFilesystem,
            &FbneoDiscoveryInputs {
                explicit_executable: Some(executable),
            },
        );
        assert!(found.selected.is_none());
        assert_eq!(inspect_fbneo_version(), FbneoVersion::UnknownVersion);
    }
}
