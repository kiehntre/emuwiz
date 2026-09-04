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
use crate::launch::process_spawn::{
    CapturedFileIdentity, PreparedProcessCommand, WatchedProcess, spawn_watched_process,
};
use crate::launch::readiness::LaunchBlocker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameLaunchRequest {
    pub identity: CanonicalIdentityStatus,
    pub set_resolutions: Vec<SetResolution>,
    pub expected_executable: PathBuf,
    /// Exact archive/ROM content selected by the user. MAME usually receives
    /// the machine shortname instead of this path in argv.
    pub selected_content: PathBuf,
    /// Optional point-in-time identity captured with the selection. When
    /// present, preflight rejects a replacement at the same path.
    pub expected_content_identity: Option<CapturedFileIdentity>,
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
    let content_metadata =
        fs::symlink_metadata(&request.selected_content).map_err(|_| MameLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                crate::launch::readiness::LaunchBlockerKind::MameSetVerdictUnavailable,
                "the selected MAME archive/content is no longer available",
            )],
        })?;
    if content_metadata.file_type().is_symlink()
        || (!content_metadata.is_file() && !content_metadata.is_dir())
    {
        return Err(MameLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                crate::launch::readiness::LaunchBlockerKind::MameSetVerdictUnavailable,
                "the selected MAME archive/content is not a regular file or directory",
            )],
        });
    }
    if let Some(expected) = request.expected_content_identity
        && (!content_metadata.is_file()
            || CapturedFileIdentity::capture(&content_metadata) != expected)
    {
        return Err(MameLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                crate::launch::readiness::LaunchBlockerKind::MameSetVerdictUnavailable,
                "the selected MAME archive/content changed since it was authorized",
            )],
        });
    }
    let Some(resolution) = request.set_resolutions.first() else {
        return Err(MameLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                crate::launch::readiness::LaunchBlockerKind::MameSetVerdictUnavailable,
                "no current DAT-backed MAME set verdict is available",
            )],
        });
    };
    if request.set_resolutions.len() == 1 && resolution.archive_path != request.selected_content {
        return Err(MameLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                crate::launch::readiness::LaunchBlockerKind::MameSetIdentityUnavailable,
                "selected MAME content no longer matches the authorized set evidence",
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
        let selected_content = executable.with_extension("zip");
        std::fs::write(&selected_content, b"verified set placeholder").unwrap();
        let expected_content_identity = Some(CapturedFileIdentity::capture(
            &std::fs::symlink_metadata(&selected_content).unwrap(),
        ));
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
                archive_path: selected_content.clone(),
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
            selected_content,
            expected_content_identity,
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

    #[test]
    fn missing_selected_content_is_blocked_before_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fake-mame");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut request = request(executable);
        request.selected_content = dir.path().join("gone.zip");
        let error = preflight_mame_launch(&request).unwrap_err();
        assert!(error.blockers.iter().any(|blocker| {
            blocker.kind == crate::launch::readiness::LaunchBlockerKind::MameSetVerdictUnavailable
        }));
    }

    #[test]
    fn selected_content_drift_is_blocked_without_changing_machine_target() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fake-mame");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut request = request(executable);
        request.selected_content = dir.path().join("different.zip");
        request.expected_content_identity = None;
        std::fs::write(&request.selected_content, b"different").unwrap();
        let error = preflight_mame_launch(&request).unwrap_err();
        assert!(error.blockers.iter().any(|blocker| {
            blocker.kind == crate::launch::readiness::LaunchBlockerKind::MameSetIdentityUnavailable
        }));
    }

    #[test]
    fn selected_content_replacement_at_the_same_path_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fake-mame");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let request = request(executable);
        std::fs::write(
            &request.selected_content,
            b"replacement with a different size",
        )
        .unwrap();
        let error = preflight_mame_launch(&request).unwrap_err();
        assert!(error.blockers.iter().any(|blocker| {
            blocker.kind == crate::launch::readiness::LaunchBlockerKind::MameSetVerdictUnavailable
        }));
    }
}
