//! Native FinalBurn Neo execution with live content and executable preflight.

use std::fs;
use std::path::PathBuf;

use crate::launch::fbneo_command::{
    FbneoCommand, FbneoCommandPlan, FbneoSetEvidence, build_fbneo_command_plan,
};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    CapturedFileIdentity, PreparedProcessCommand, WatchedProcess, spawn_watched_process,
};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbneoLaunchRequest {
    pub identity: CanonicalIdentityStatus,
    pub set: FbneoSetEvidence,
    pub expected_executable: PathBuf,
    pub selected_content: PathBuf,
    pub expected_content_identity: Option<CapturedFileIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbneoLaunchPreflightError {
    pub blockers: Vec<LaunchBlocker>,
}

#[derive(Debug)]
pub enum FbneoLaunchExecutionError {
    Preflight(FbneoLaunchPreflightError),
    Spawn(std::io::Error),
}

pub fn preflight_fbneo_launch(
    request: &FbneoLaunchRequest,
) -> Result<FbneoCommand, FbneoLaunchPreflightError> {
    let metadata = fs::symlink_metadata(&request.expected_executable).map_err(|_| {
        FbneoLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                LaunchBlockerKind::FbneoEmulatorUnavailable,
                "the authorized FBNeo executable is no longer available",
            )],
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(FbneoLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                LaunchBlockerKind::FbneoEmulatorUnavailable,
                "the authorized FBNeo executable is not a regular file",
            )],
        });
    }
    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 == 0 {
        return Err(FbneoLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                LaunchBlockerKind::FbneoEmulatorUnavailable,
                "the authorized FBNeo executable is not executable",
            )],
        });
    }
    let content_metadata =
        fs::symlink_metadata(&request.selected_content).map_err(|_| FbneoLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                LaunchBlockerKind::FbneoContentUnavailable,
                "the selected FBNeo archive/content is no longer available",
            )],
        })?;
    if content_metadata.file_type().is_symlink()
        || (!content_metadata.is_file() && !content_metadata.is_dir())
    {
        return Err(FbneoLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                LaunchBlockerKind::FbneoContentUnavailable,
                "the selected FBNeo archive/content is not a regular file or directory",
            )],
        });
    }
    if let Some(expected) = request.expected_content_identity
        && (!content_metadata.is_file()
            || CapturedFileIdentity::capture(&content_metadata) != expected)
    {
        return Err(FbneoLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                LaunchBlockerKind::FbneoContentUnavailable,
                "the selected FBNeo archive/content changed since it was authorized",
            )],
        });
    }
    if request.set.resolution.archive_path != request.selected_content {
        return Err(FbneoLaunchPreflightError {
            blockers: vec![LaunchBlocker::new(
                LaunchBlockerKind::FbneoContentUnavailable,
                "selected FBNeo content no longer matches the authorized set evidence",
            )],
        });
    }
    let FbneoCommandPlan { command, blockers } = build_fbneo_command_plan(
        &request.identity,
        &request.set,
        Some(&request.expected_executable),
    );
    match (command, blockers.is_empty()) {
        (Some(command), true) => Ok(command),
        (_, _) => Err(FbneoLaunchPreflightError { blockers }),
    }
}

pub fn spawn_fbneo(command: &FbneoCommand) -> std::io::Result<WatchedProcess> {
    spawn_watched_process(&PreparedProcessCommand {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
    })
}

pub fn preflight_and_launch_fbneo(
    request: &FbneoLaunchRequest,
) -> Result<WatchedProcess, FbneoLaunchExecutionError> {
    let command = preflight_fbneo_launch(request).map_err(FbneoLaunchExecutionError::Preflight)?;
    spawn_fbneo(&command).map_err(FbneoLaunchExecutionError::Spawn)
}

#[cfg(test)]
mod tests;
