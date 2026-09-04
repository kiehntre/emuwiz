//! Fresh native mGBA launch preflight and process spawn.
//!
//! Only direct, loose `.gb`, `.gbc`, and `.gba` files are accepted.  The
//! canonical identity and platform are supplied by the authoritative identity
//! layer and are checked again immediately before spawn.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::mgba_command::{MGBA_SUPPORTED_PLATFORM_IDS, direct_mgba_extension};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::patch_manager::{
    MgbaProfileDiscoveryRoots, discover_mgba_profiles, resolve_mgba_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
    pub config_identity: Option<CapturedFileIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgbaLaunchPreflightErrorKind {
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
    ConfigurationChanged,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaLaunchPreflightError {
    pub kind: MgbaLaunchPreflightErrorKind,
    pub detail: String,
}
fn error(
    kind: MgbaLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> MgbaLaunchPreflightError {
    MgbaLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}
fn file(path: &Path) -> Result<CapturedFileIdentity, MgbaLaunchPreflightError> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| error(MgbaLaunchPreflightErrorKind::ContentNotFound, e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentIsSymlink,
            "selected content is a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentNotRegularFile,
            "selected content is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&meta))
}

pub fn preflight_mgba_launch(
    request: &MgbaLaunchRequest,
    roots: &MgbaProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, MgbaLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content path must be absolute",
        ));
    }
    if !MGBA_SUPPORTED_PLATFORM_IDS.contains(&request.expected_platform_id.as_str())
        || !direct_mgba_extension(
            &request.selected_content_path,
            &request.expected_platform_id,
        )
    {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentFormatUnsupported,
            "native mGBA accepts only direct .gb, .gbc, or .gba content matching the selected platform",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed since authorization",
        ));
    }
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => r,
        CanonicalIdentityStatus::Unknown => {
            return Err(error(
                MgbaLaunchPreflightErrorKind::IdentityUnresolved,
                "fresh mGBA identity is unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(error(
                MgbaLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity evidence conflicts",
            ));
        }
    };
    if resolved.platform_id != request.expected_platform_id
        || !MGBA_SUPPORTED_PLATFORM_IDS.contains(&resolved.platform_id.as_str())
        || resolved.game_key != request.expected_game_key
    {
        return Err(error(
            MgbaLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity no longer matches the authorized mGBA content",
        ));
    }
    let profile = discover_mgba_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                MgbaLaunchPreflightErrorKind::ProfileNotFound,
                "authorized mGBA profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "profile is not eligible".into()),
        ));
    }
    if let Some(expected) = request.config_identity {
        let Some(config) = profile.config_path.as_ref() else {
            return Err(error(
                MgbaLaunchPreflightErrorKind::ConfigurationChanged,
                "authorized mGBA profile no longer has its inspected configuration",
            ));
        };
        if file(config)
            .map_err(|e| error(MgbaLaunchPreflightErrorKind::ConfigurationChanged, e.detail))?
            != expected
        {
            return Err(error(
                MgbaLaunchPreflightErrorKind::ConfigurationChanged,
                "mGBA configuration changed since authorization",
            ));
        }
    }
    let binding = resolve_mgba_native_launch_binding(&profile)
        .map_err(|e| error(MgbaLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if binding.executable != request.expected_executable {
        return Err(error(
            MgbaLaunchPreflightErrorKind::BindingDrift,
            "mGBA executable binding changed since authorization",
        ));
    }
    let meta = fs::symlink_metadata(&binding.executable).map_err(|e| {
        error(
            MgbaLaunchPreflightErrorKind::BindingUnavailable,
            e.to_string(),
        )
    })?;
    if CapturedFileIdentity::capture(&meta) != request.executable_identity {
        return Err(error(
            MgbaLaunchPreflightErrorKind::BindingDrift,
            "mGBA executable changed since authorization",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments: vec![request.selected_content_path.clone().into_os_string()],
        working_directory: None,
    })
}

pub fn spawn_mgba(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}
