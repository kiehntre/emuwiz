//! Fresh, no-shell DeSmuME launch preflight.

use crate::launch::desmume_command::{DESMUME_SUPPORTED_PLATFORM_ID, direct_desmume_extension};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::{
    DesmumeProfileDiscoveryRoots, discover_desmume_profiles, resolve_desmume_native_launch_binding,
};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesmumeLaunchPreflightErrorKind {
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
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeLaunchPreflightError {
    pub kind: DesmumeLaunchPreflightErrorKind,
    pub detail: String,
}
fn fail(
    kind: DesmumeLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> DesmumeLaunchPreflightError {
    DesmumeLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}
fn file(path: &Path) -> Result<CapturedFileIdentity, DesmumeLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        fail(
            DesmumeLaunchPreflightErrorKind::ContentNotFound,
            error.to_string(),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::ContentIsSymlink,
            "path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::ContentNotRegularFile,
            "path is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}
pub fn preflight_desmume_launch(
    request: &DesmumeLaunchRequest,
    roots: &DesmumeProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, DesmumeLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content must be absolute",
        ));
    }
    if !direct_desmume_extension(&request.selected_content_path)
        || crate::archive_kind(&request.selected_content_path)
            .is_some_and(|kind| kind.is_mount_input())
    {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::ContentFormatUnsupported,
            "not a direct .nds DeSmuME content file",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed since authorization",
        ));
    }
    let CanonicalIdentityStatus::Resolved(identity) = identity else {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::IdentityUnresolved,
            "identity is unresolved or conflicting",
        ));
    };
    if identity.platform_id != DESMUME_SUPPORTED_PLATFORM_ID
        || identity.platform_id != request.expected_platform_id
        || identity.game_key != request.expected_game_key
    {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::IdentityMismatch,
            "identity no longer matches authorized Nintendo DS content",
        ));
    }
    let profile = discover_desmume_profiles(roots)
        .profiles
        .into_iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            fail(
                DesmumeLaunchPreflightErrorKind::ProfileNotFound,
                "authorized DeSmuME profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "DeSmuME profile is ineligible".into()),
        ));
    }
    let binding = resolve_desmume_native_launch_binding(&profile).map_err(|error| {
        fail(
            DesmumeLaunchPreflightErrorKind::BindingUnavailable,
            error.detail,
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::BindingDrift,
            "DeSmuME executable binding changed since authorization",
        ));
    }
    let executable_identity = file(&binding.executable).map_err(|error| {
        fail(
            DesmumeLaunchPreflightErrorKind::BindingUnavailable,
            error.detail,
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&binding.executable)
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 == 0)
        {
            return Err(fail(
                DesmumeLaunchPreflightErrorKind::BindingUnavailable,
                "DeSmuME executable no longer has an execute bit",
            ));
        }
    }
    if executable_identity != request.executable_identity {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::BindingDrift,
            "DeSmuME executable changed since authorization",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(fail(
            DesmumeLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments: vec![request.selected_content_path.clone().into_os_string()],
        working_directory: None,
    })
}
pub fn spawn_desmume(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}
