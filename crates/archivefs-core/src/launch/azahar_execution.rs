//! Fresh, read-only Azahar preflight and shared process execution.
use crate::launch::azahar_command::{AzaharLaunchRequest, build_azahar_command_plan};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::readiness::LaunchBlocker;
use crate::patch_manager::{AzaharContentForm, AzaharEvidenceState, inspect_azahar_3dsx};
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AzaharPreflightErrorKind {
    ExecutableUnavailable,
    ExecutableDrift,
    ContentUnavailable,
    ContentDrift,
    CommandBlocked,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzaharPreflightError {
    pub kind: AzaharPreflightErrorKind,
    pub detail: String,
}
fn error(kind: AzaharPreflightErrorKind, detail: impl Into<String>) -> AzaharPreflightError {
    AzaharPreflightError {
        kind,
        detail: detail.into(),
    }
}
fn check(
    path: &std::path::Path,
    expected: CapturedFileIdentity,
    kind: AzaharPreflightErrorKind,
    label: &str,
) -> Result<(), AzaharPreflightError> {
    let m =
        fs::symlink_metadata(path).map_err(|e| error(kind, format!("{label} unavailable: {e}")))?;
    if m.file_type().is_symlink() || !m.is_file() || CapturedFileIdentity::capture(&m) != expected {
        return Err(error(kind, format!("{label} changed or is unsafe")));
    }
    Ok(())
}
fn check_executable(path: &std::path::Path) -> Result<(), AzaharPreflightError> {
    check(
        path,
        // The caller's identity check below supplies the actual expected
        // identity; this helper only verifies the executable contract.
        CapturedFileIdentity::capture(&fs::symlink_metadata(path).map_err(|e| {
            error(
                AzaharPreflightErrorKind::ExecutableUnavailable,
                e.to_string(),
            )
        })?),
        AzaharPreflightErrorKind::ExecutableUnavailable,
        "Azahar executable",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::symlink_metadata(path).map_err(|e| {
            error(
                AzaharPreflightErrorKind::ExecutableUnavailable,
                e.to_string(),
            )
        })?;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(error(
                AzaharPreflightErrorKind::ExecutableUnavailable,
                "Azahar executable is not executable",
            ));
        }
    }
    Ok(())
}
pub fn preflight_azahar_launch(
    request: &AzaharLaunchRequest,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, AzaharPreflightError> {
    check_executable(&request.executable)?;
    check(
        &request.executable,
        request.executable_identity,
        AzaharPreflightErrorKind::ExecutableDrift,
        "Azahar executable",
    )?;
    check(
        &request.content,
        request.captured_identity,
        AzaharPreflightErrorKind::ContentDrift,
        "selected .3dsx",
    )?;
    if AzaharContentForm::from_path(&request.content) != AzaharContentForm::ThreeDsx {
        return Err(error(
            AzaharPreflightErrorKind::ContentDrift,
            "selected content is no longer a loose .3dsx file",
        ));
    }
    if let Some(profile) = &request.profile {
        if matches!(
            request.profile_state,
            AzaharEvidenceState::Unreadable | AzaharEvidenceState::Oversized
        ) {
            return Err(error(
                AzaharPreflightErrorKind::CommandBlocked,
                "selected Azahar profile is unreadable or oversized",
            ));
        }
        if let Some(identity) = request.profile_identity {
            check(
                profile,
                identity,
                AzaharPreflightErrorKind::CommandBlocked,
                "Azahar profile",
            )?;
        }
    }
    // Re-inspect bounded embedded metadata immediately before spawn. Missing
    // or malformed SMDH is a warning, not a Phase 1 blocker; the call ensures
    // cached title metadata is never used as launch authorization.
    inspect_azahar_3dsx(&request.content).map_err(|e| {
        error(
            AzaharPreflightErrorKind::ContentUnavailable,
            format!("selected .3dsx unavailable: {e}"),
        )
    })?;
    let plan = build_azahar_command_plan(identity, request);
    if let Some(blocker) = plan.blockers.into_iter().next() {
        return Err(error(
            AzaharPreflightErrorKind::CommandBlocked,
            format_blocker(blocker),
        ));
    }
    let command = plan.command.ok_or_else(|| {
        error(
            AzaharPreflightErrorKind::CommandBlocked,
            "Azahar command missing",
        )
    })?;
    Ok(PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: request.content.parent().map(|p| p.to_path_buf()),
    })
}
fn format_blocker(blocker: LaunchBlocker) -> String {
    format!("Azahar launch blocked: {blocker:?}")
}
pub struct LaunchedAzaharProcess {
    pub pid: u32,
    watched: WatchedProcess,
}
impl LaunchedAzaharProcess {
    pub fn poll(&mut self) -> Option<&process_spawn::ProcessExitReport> {
        self.watched.poll()
    }
    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}
pub fn spawn_azahar(
    command: PreparedProcessCommand,
) -> Result<LaunchedAzaharProcess, std::io::Error> {
    let watched = process_spawn::spawn_watched_process(&command)?;
    Ok(LaunchedAzaharProcess {
        pid: watched.pid,
        watched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_content_fails_closed_without_spawn() {
        let request = AzaharLaunchRequest {
            executable: "/missing/azahar".into(),
            platform: "Nintendo 3DS".into(),
            content: "/missing/game.3dsx".into(),
            content_form: crate::patch_manager::AzaharContentForm::ThreeDsx,
            profile: None,
            profile_state: crate::patch_manager::AzaharEvidenceState::Absent,
            profile_identity: None,
            title_identity: None,
            keys_state: crate::patch_manager::AzaharEvidenceState::Absent,
            system_data_state: crate::patch_manager::AzaharEvidenceState::Unknown,
            captured_identity: CapturedFileIdentity {
                device: 0,
                inode: 0,
                size: 0,
                modified: None,
            },
            executable_identity: CapturedFileIdentity {
                device: 0,
                inode: 0,
                size: 0,
                modified: None,
            },
        };
        let result = preflight_azahar_launch(&request, &CanonicalIdentityStatus::Unknown);
        assert!(result.is_err());
    }
}
