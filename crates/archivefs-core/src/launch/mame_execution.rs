//! Native MAME execution with live executable/set-verdict preflight.
//!
//! This module only starts MAME with a minimal native argv. It does not claim
//! that a successful spawn means the game booted, and it never mounts,
//! extracts, rebuilds, or repacks ROM content.

use std::fs;
use std::path::PathBuf;

use crate::dat::set::SetResolution;
use crate::launch::mame_command::{MameCommand, build_mame_command_plan};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{PreparedProcessCommand, WatchedProcess, spawn_watched_process};
use crate::launch::readiness::LaunchBlocker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameLaunchRequest {
    pub identity: CanonicalIdentityStatus,
    pub set_resolutions: Vec<SetResolution>,
    pub expected_executable: PathBuf,
    pub rom_search_path_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameLaunchPreflightError {
    pub blockers: Vec<LaunchBlocker>,
}

#[derive(Debug)]
pub enum MameLaunchExecutionError {
    Preflight(MameLaunchPreflightError),
    Spawn(std::io::Error),
}

/// Revalidates the exact executable and current set evidence immediately
/// before producing the command. The caller supplies fresh set resolutions;
/// cached absence is never treated as a pass.
pub fn preflight_mame_launch(
    request: &MameLaunchRequest,
) -> Result<MameCommand, MameLaunchPreflightError> {
    let executable = &request.expected_executable;
    let metadata = fs::symlink_metadata(executable).map_err(|_| MameLaunchPreflightError {
        blockers: vec![LaunchBlocker::new(
            crate::launch::readiness::LaunchBlockerKind::MameEmulatorUnavailable,
            "the authorized MAME executable is no longer available",
        )],
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(MameLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                crate::launch::readiness::LaunchBlockerKind::MameEmulatorUnavailable,
                "the authorized MAME executable is not a regular file",
            )],
        });
    }
    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 == 0 {
        return Err(MameLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                crate::launch::readiness::LaunchBlockerKind::MameEmulatorUnavailable,
                "the authorized MAME executable is not executable",
            )],
        });
    }
    let plan = build_mame_command_plan(
        &request.identity,
        &request.set_resolutions,
        Some(executable),
        request.rom_search_path_configured,
    );
    match (plan.command, plan.blockers.is_empty()) {
        (Some(command), true) => Ok(command),
        (_, _) => Err(MameLaunchPreflightError {
            blockers: plan.blockers,
        }),
    }
}

pub fn spawn_mame(command: &MameCommand) -> std::io::Result<WatchedProcess> {
    spawn_watched_process(&PreparedProcessCommand {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
    })
}

pub fn preflight_and_launch_mame(
    request: &MameLaunchRequest,
) -> Result<WatchedProcess, MameLaunchExecutionError> {
    let command = preflight_mame_launch(request).map_err(MameLaunchExecutionError::Preflight)?;
    spawn_mame(&command).map_err(MameLaunchExecutionError::Spawn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::dependency::{DependencyState, SetDependencyReport};
    use crate::dat::set::{SetIdentity, SetResolution, SetState};
    use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
    use std::os::unix::fs::PermissionsExt;

    fn request(executable: PathBuf) -> MameLaunchRequest {
        MameLaunchRequest {
            identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "Arcade".into(),
                game_key: "test;set".into(),
            }),
            set_resolutions: vec![SetResolution {
                identity: SetIdentity {
                    source_id: "mame".into(),
                    game_name: "test;set".into(),
                },
                archive_path: "/library/test.zip".into(),
                state: SetState::Complete,
                members_required: Vec::new(),
                members_verified: Vec::new(),
                members_bad: Vec::new(),
                members_optional: Vec::new(),
                members_borrowed: Vec::new(),
                disks_required: Vec::new(),
                disks_verified: Vec::new(),
                disks_parent_required: Vec::new(),
                dependencies: SetDependencyReport {
                    state: DependencyState::NotApplicable,
                    requirements: Vec::new(),
                },
            }],
            expected_executable: executable,
            rom_search_path_configured: true,
        }
    }

    #[test]
    fn fake_executable_receives_exact_set_argument_and_reports_exit() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fake-mame");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s' \"$1\" > \"$0.argv\"\nexit 7\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let request = request(executable.clone());
        let command = preflight_mame_launch(&request).unwrap();
        assert_eq!(
            command.arguments,
            vec![std::ffi::OsString::from("test;set")]
        );
        let mut process = spawn_mame(&command).unwrap();
        while process.poll().is_none() {
            std::thread::yield_now();
        }
        assert_eq!(
            process.poll().unwrap().status.as_ref().unwrap().code(),
            Some(7)
        );
        assert_eq!(
            std::fs::read_to_string(executable.with_extension("argv")).unwrap(),
            "test;set"
        );
    }
}
