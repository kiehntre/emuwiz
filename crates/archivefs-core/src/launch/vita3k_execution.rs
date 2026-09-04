//! Fresh preflight and safe native spawn for an installed Vita3K title.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::vita3k_command::VITA3K_SUPPORTED_PLATFORM_ID;
use crate::patch_manager::{
    Vita3kProfileDiscoveryRoots, discover_vita3k_profiles, inspect_installed_title,
    resolve_vita3k_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kLaunchRequest {
    pub profile_id: String,
    pub selected_title_path: PathBuf,
    pub expected_title_id: String,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub expected_executable: PathBuf,
    pub title_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
    pub config_identity: Option<CapturedFileIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vita3kLaunchPreflightErrorKind {
    PathNotAbsolute,
    TitleMissing,
    TitleUnsafe,
    TitleMetadataInvalid,
    TitleIdMismatch,
    IdentityMismatch,
    ProfileNotFound,
    ProfileIneligible,
    ExecutableMissing,
    ExecutableUnsafe,
    ExecutableChanged,
    ConfigurationChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kLaunchPreflightError {
    pub kind: Vita3kLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: Vita3kLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> Vita3kLaunchPreflightError {
    Vita3kLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn identity(path: &Path) -> Result<CapturedFileIdentity, Vita3kLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            Vita3kLaunchPreflightErrorKind::TitleMissing,
            io_error.to_string(),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(preflight_error(
            Vita3kLaunchPreflightErrorKind::TitleUnsafe,
            "installed title is not a real directory",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

pub fn preflight_vita3k_launch(
    request: &Vita3kLaunchRequest,
    roots: &Vita3kProfileDiscoveryRoots,
    identity_status: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, Vita3kLaunchPreflightError> {
    if !request.selected_title_path.is_absolute() {
        return Err(preflight_error(
            Vita3kLaunchPreflightErrorKind::PathNotAbsolute,
            "installed title path must be absolute",
        ));
    }
    if identity(&request.selected_title_path)? != request.title_identity {
        return Err(preflight_error(
            Vita3kLaunchPreflightErrorKind::TitleMissing,
            "installed title changed since authorization",
        ));
    }
    let title = inspect_installed_title(
        &discover_vita3k_profiles(roots)
            .profiles
            .into_iter()
            .find(|profile| profile.profile_id == request.profile_id)
            .ok_or_else(|| {
                preflight_error(
                    Vita3kLaunchPreflightErrorKind::ProfileNotFound,
                    "authorized Vita3K profile was not rediscovered",
                )
            })?,
        &request.expected_title_id,
    )
    .map_err(|detail| {
        preflight_error(Vita3kLaunchPreflightErrorKind::TitleMetadataInvalid, detail)
    })?;
    if title.title_id != request.expected_title_id {
        return Err(preflight_error(
            Vita3kLaunchPreflightErrorKind::TitleIdMismatch,
            "installed title ID changed",
        ));
    }
    let resolved = match identity_status {
        CanonicalIdentityStatus::Resolved(identity)
            if identity.platform_id == VITA3K_SUPPORTED_PLATFORM_ID
                && identity.game_key == request.expected_game_key =>
        {
            identity
        }
        _ => {
            return Err(preflight_error(
                Vita3kLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity is not the authorized PlayStation Vita title",
            ));
        }
    };
    let _ = resolved;
    let profile = discover_vita3k_profiles(roots)
        .profiles
        .into_iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                Vita3kLaunchPreflightErrorKind::ProfileNotFound,
                "authorized Vita3K profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(preflight_error(
            Vita3kLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "profile is ineligible".into()),
        ));
    }
    if let Some(expected) = request.config_identity {
        let config = profile.config_path.as_ref().ok_or_else(|| {
            preflight_error(
                Vita3kLaunchPreflightErrorKind::ConfigurationChanged,
                "Vita3K configuration disappeared",
            )
        })?;
        let metadata = fs::symlink_metadata(config).map_err(|io_error| {
            preflight_error(
                Vita3kLaunchPreflightErrorKind::ConfigurationChanged,
                io_error.to_string(),
            )
        })?;
        if CapturedFileIdentity::capture(&metadata) != expected {
            return Err(preflight_error(
                Vita3kLaunchPreflightErrorKind::ConfigurationChanged,
                "Vita3K configuration changed",
            ));
        }
    }
    let binding = resolve_vita3k_native_launch_binding(&profile).map_err(|blocker| {
        preflight_error(
            Vita3kLaunchPreflightErrorKind::ExecutableMissing,
            blocker.detail,
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(preflight_error(
            Vita3kLaunchPreflightErrorKind::ExecutableChanged,
            "Vita3K executable binding changed",
        ));
    }
    let executable_metadata = fs::symlink_metadata(&binding.executable).map_err(|io_error| {
        preflight_error(
            Vita3kLaunchPreflightErrorKind::ExecutableMissing,
            io_error.to_string(),
        )
    })?;
    if executable_metadata.file_type().is_symlink() || !executable_metadata.is_file() {
        return Err(preflight_error(
            Vita3kLaunchPreflightErrorKind::ExecutableUnsafe,
            "Vita3K executable is not a real file",
        ));
    }
    if CapturedFileIdentity::capture(&executable_metadata) != request.executable_identity {
        return Err(preflight_error(
            Vita3kLaunchPreflightErrorKind::ExecutableChanged,
            "Vita3K executable changed",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments: vec![
            "--installed-path".into(),
            request.expected_title_id.clone().into(),
        ],
        working_directory: None,
    })
}

pub fn spawn_vita3k(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}
