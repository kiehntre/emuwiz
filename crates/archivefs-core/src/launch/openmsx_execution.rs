//! Fresh preflight and watched execution for the existing openMSX cartridge
//! adapter. This module never invokes a shell or writes openMSX configuration.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::patch_manager::{OpenMsxProfileDiscoveryRoots, discover_openmsx_profiles};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMsxLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub expected_machine: String,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMsxLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    ContentNotRegularFile,
    ContentFormatUnsupported,
    ContentChangedBeforeSpawn,
    IdentityUnresolved,
    IdentityMismatch,
    ProfileNotFound,
    ProfileIneligible,
    BindingUnavailable,
    BindingDrift,
    MachineMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMsxLaunchPreflightError {
    pub kind: OpenMsxLaunchPreflightErrorKind,
    pub detail: String,
}

#[derive(Debug)]
pub enum OpenMsxLaunchExecutionError {
    Preflight(OpenMsxLaunchPreflightError),
    Spawn(std::io::Error),
}

fn error(
    kind: OpenMsxLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> OpenMsxLaunchPreflightError {
    OpenMsxLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn machine_for_platform(platform: &str) -> Option<&'static str> {
    match platform {
        "MSX" => Some("C-BIOS_MSX1"),
        "MSX2" => Some("C-BIOS_MSX2"),
        _ => None,
    }
}

fn capture_content(path: &Path) -> Result<CapturedFileIdentity, OpenMsxLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| {
        error(
            OpenMsxLaunchPreflightErrorKind::ContentNotFound,
            source.to_string(),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::ContentIsSymlink,
            "selected content is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::ContentNotRegularFile,
            "selected content is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

pub fn preflight_openmsx_launch(
    request: &OpenMsxLaunchRequest,
    roots: &OpenMsxProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, OpenMsxLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content path must be absolute",
        ));
    }
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(identity) => identity,
        CanonicalIdentityStatus::Unknown => {
            return Err(error(
                OpenMsxLaunchPreflightErrorKind::IdentityUnresolved,
                "fresh openMSX identity is unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(error(
                OpenMsxLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity evidence conflicts",
            ));
        }
    };
    let expected_machine = machine_for_platform(&resolved.platform_id).ok_or_else(|| {
        error(
            OpenMsxLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity is not an MSX or MSX2 title",
        )
    })?;
    let expected_extension = if resolved.platform_id == "MSX" {
        "mx1"
    } else {
        "mx2"
    };
    if resolved.platform_id != request.expected_platform_id
        || resolved.game_key != request.expected_game_key
    {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity no longer matches the authorized openMSX content",
        ));
    }
    if request.expected_machine != expected_machine {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::MachineMismatch,
            "authorized openMSX machine does not match the fresh MSX identity",
        ));
    }
    if !request
        .selected_content_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected_extension))
    {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::ContentFormatUnsupported,
            format!("only direct .{expected_extension} cartridges are supported"),
        ));
    }
    if crate::archive_kind(&request.selected_content_path).is_some_and(|kind| kind.is_mount_input())
    {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::ContentFormatUnsupported,
            "content is an archive or mount-input path, not direct content",
        ));
    }
    if capture_content(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed since authorization",
        ));
    }
    let profile = discover_openmsx_profiles(roots)
        .profiles
        .into_iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                OpenMsxLaunchPreflightErrorKind::ProfileNotFound,
                "authorized openMSX profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "openMSX profile is not eligible".into()),
        ));
    }
    if profile.executable.path != request.expected_executable {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::BindingDrift,
            "openMSX executable binding changed since authorization",
        ));
    }
    let metadata = fs::symlink_metadata(&profile.executable.path).map_err(|source| {
        error(
            OpenMsxLaunchPreflightErrorKind::BindingUnavailable,
            source.to_string(),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::BindingUnavailable,
            "openMSX executable is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(error(
                OpenMsxLaunchPreflightErrorKind::BindingUnavailable,
                "openMSX executable has no execute bit set",
            ));
        }
    }
    if CapturedFileIdentity::capture(&metadata) != request.executable_identity {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::BindingDrift,
            "openMSX executable changed since authorization",
        ));
    }
    if capture_content(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            OpenMsxLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: profile.executable.path,
        arguments: vec![
            "-machine".into(),
            request.expected_machine.clone().into(),
            "-carta".into(),
            request.selected_content_path.clone().into_os_string(),
        ],
        working_directory: None,
    })
}

pub fn spawn_openmsx(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}

/// Performs the final identity/profile/content preflight and then starts the
/// already-authorized openMSX command under the shared watched-process
/// lifecycle. No shell or configuration mutation is involved.
pub fn preflight_and_launch_openmsx(
    request: &OpenMsxLaunchRequest,
    roots: &OpenMsxProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<WatchedProcess, OpenMsxLaunchExecutionError> {
    let command = preflight_openmsx_launch(request, roots, identity)
        .map_err(OpenMsxLaunchExecutionError::Preflight)?;
    spawn_openmsx(&command).map_err(OpenMsxLaunchExecutionError::Spawn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
    use std::ffi::OsString;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn identity() -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "MSX".into(),
            game_key: "verified-msx-game".into(),
        })
    }

    fn fixture() -> (
        tempfile::TempDir,
        OpenMsxLaunchRequest,
        OpenMsxProfileDiscoveryRoots,
    ) {
        let directory = tempdir().unwrap();
        let executable_path = directory.path().join("openmsx");
        let content_path = directory.path().join("game.mx1");
        fs::write(&executable_path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::write(&content_path, b"mx1-content").unwrap();
        #[cfg(unix)]
        executable(&executable_path);
        let roots = OpenMsxProfileDiscoveryRoots {
            explicit_executables: vec![executable_path.clone()],
            path_env: None,
        };
        let request = OpenMsxLaunchRequest {
            selected_content_path: content_path.clone(),
            expected_platform_id: "MSX".into(),
            expected_game_key: "verified-msx-game".into(),
            profile_id: format!("openmsx:{}", executable_path.display()),
            expected_executable: executable_path.clone(),
            expected_machine: "C-BIOS_MSX1".into(),
            content_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&content_path).unwrap(),
            ),
            executable_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&executable_path).unwrap(),
            ),
        };
        (directory, request, roots)
    }

    #[test]
    fn fresh_preflight_preserves_typed_argv() {
        let (_directory, request, roots) = fixture();
        let command = preflight_openmsx_launch(&request, &roots, &identity()).unwrap();
        assert_eq!(command.executable, request.expected_executable);
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("-machine"),
                OsString::from("C-BIOS_MSX1"),
                OsString::from("-carta"),
                request.selected_content_path.clone().into_os_string(),
            ]
        );
    }

    #[test]
    fn content_drift_is_refused() {
        let (_directory, request, roots) = fixture();
        fs::write(&request.selected_content_path, b"changed").unwrap();
        let error = preflight_openmsx_launch(&request, &roots, &identity()).unwrap_err();
        assert_eq!(
            error.kind,
            OpenMsxLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );
    }

    #[test]
    fn wrong_machine_or_platform_is_refused() {
        let (_directory, mut request, roots) = fixture();
        request.expected_machine = "C-BIOS_MSX2".into();
        let error = preflight_openmsx_launch(&request, &roots, &identity()).unwrap_err();
        assert_eq!(error.kind, OpenMsxLaunchPreflightErrorKind::MachineMismatch);
    }

    #[cfg(unix)]
    #[test]
    fn watched_spawn_reports_exit_without_a_shell() {
        let (_directory, request, roots) = fixture();
        let command = preflight_openmsx_launch(&request, &roots, &identity()).unwrap();
        let mut process = spawn_openmsx(&command).unwrap();
        loop {
            if let Some(report) = process.poll() {
                assert!(report.status.as_ref().unwrap().success());
                break;
            }
            std::thread::yield_now();
        }
    }
}
