//! Fresh FS-UAE execution preflight.
//!
//! This module revalidates the exact profile, configuration, Kickstart
//! evidence, Amiga identity, executable, and selected media before spawning.
//! It performs no configuration, firmware, or media writes.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::fsuae_command::{
    FsUaeCommand, build_fsuae_command_plan, resolve_fsuae_native_launch_binding,
};
use crate::launch::input_projection::VerifiedIdentityFact;
use crate::launch::planning::{
    CandidatePreference, CanonicalIdentityStatus, LaunchCandidate, LaunchContainerKind,
    LaunchContentKind, LaunchContentRef, LaunchTarget,
};
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::{
    AmigaEmulatorKind, AmigaGameRequest, AmigaProfileDiscoveryRoots, discover_amiga_profiles,
    inspect_amiga_whdload_game,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUaeLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
    pub config_path: PathBuf,
    pub config_identity: CapturedFileIdentity,
    pub caps_backend_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsUaeLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    ContentNotRegularFile,
    ContentUnsupported,
    ContentChanged,
    IdentityUnresolved,
    IdentityMismatch,
    ProfileNotFound,
    WrongEmulator,
    ProfileIneligible,
    ConfigurationChanged,
    KickstartUnavailable,
    BindingUnavailable,
    BindingDrift,
    CommandBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUaeLaunchPreflightError {
    pub kind: FsUaeLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(
    kind: FsUaeLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> FsUaeLaunchPreflightError {
    FsUaeLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn file_identity(path: &Path) -> Result<CapturedFileIdentity, FsUaeLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| {
        error(
            FsUaeLaunchPreflightErrorKind::ContentNotFound,
            e.to_string(),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::ContentIsSymlink,
            "selected media is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::ContentNotRegularFile,
            "selected media is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn platform_firmware(profile: &crate::patch_manager::AmigaGameInspection) -> FirmwareReadiness {
    match profile.health.kickstart.state {
        crate::patch_manager::AmigaKickstartState::PresentUnverified => {
            FirmwareReadiness::PresentUnverified
        }
        crate::patch_manager::AmigaKickstartState::Unknown => FirmwareReadiness::Unknown,
        _ => FirmwareReadiness::Missing,
    }
}

/// Freshly validates and rebuilds a native FS-UAE command. Identity facts are
/// caller-supplied fresh evidence; this function never promotes media suffixes.
pub fn preflight_fsuae_launch(
    request: &FsUaeLaunchRequest,
    roots: &AmigaProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
    facts: &[VerifiedIdentityFact],
) -> Result<PreparedProcessCommand, FsUaeLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected media path must be absolute",
        ));
    }
    let current_content = file_identity(&request.selected_content_path)?;
    if current_content != request.content_identity {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::ContentChanged,
            "selected media changed since authorization",
        ));
    }
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(value) => value,
        CanonicalIdentityStatus::Unknown => {
            return Err(error(
                FsUaeLaunchPreflightErrorKind::IdentityUnresolved,
                "fresh Amiga identity is unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(error(
                FsUaeLaunchPreflightErrorKind::IdentityMismatch,
                "fresh Amiga identity conflicts",
            ));
        }
    };
    if resolved.platform_id != "Amiga"
        || request.expected_platform_id != resolved.platform_id
        || resolved.game_key != request.expected_game_key
    {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::IdentityMismatch,
            "fresh Amiga identity does not match the authorized request",
        ));
    }
    let fact_identity = facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::AmigaIdentity(value) => Some(value.as_str()),
        _ => None,
    });
    if fact_identity != Some(request.expected_game_key.as_str()) {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::IdentityMismatch,
            "fresh verified Amiga identity fact does not match the request",
        ));
    }
    let discovery = discover_amiga_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                FsUaeLaunchPreflightErrorKind::ProfileNotFound,
                "authorized FS-UAE profile was not rediscovered",
            )
        })?;
    if profile.emulator != AmigaEmulatorKind::FsUae {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::WrongEmulator,
            "selected profile is not FS-UAE",
        ));
    }
    if !profile.eligible {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::ProfileIneligible,
            "selected FS-UAE profile is ineligible",
        ));
    }
    if profile.global_config_path.as_deref() != Some(request.config_path.as_path())
        && !profile.profile_paths.contains(&request.config_path)
    {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::ConfigurationChanged,
            "selected FS-UAE configuration is no longer bound to the profile",
        ));
    }
    if file_identity(&request.config_path).map_err(|e| {
        error(
            FsUaeLaunchPreflightErrorKind::ConfigurationChanged,
            e.detail,
        )
    })? != request.config_identity
    {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::ConfigurationChanged,
            "FS-UAE configuration changed since authorization",
        ));
    }
    let inspection = inspect_amiga_whdload_game(
        profile,
        &AmigaGameRequest {
            verified_amiga_identity: Some(request.expected_game_key.clone()),
            ..AmigaGameRequest::default()
        },
    );
    if matches!(
        inspection.health.kickstart.state,
        crate::patch_manager::AmigaKickstartState::Missing
            | crate::patch_manager::AmigaKickstartState::NotConfigured
            | crate::patch_manager::AmigaKickstartState::Unreadable
    ) {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::KickstartUnavailable,
            "FS-UAE Kickstart evidence is not launch-ready",
        ));
    }
    let binding = resolve_fsuae_native_launch_binding(profile)
        .map_err(|e| error(FsUaeLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if binding.executable != request.expected_executable {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::BindingDrift,
            "FS-UAE executable binding changed since authorization",
        ));
    }
    if file_identity(&binding.executable)
        .map_err(|e| error(FsUaeLaunchPreflightErrorKind::BindingUnavailable, e.detail))?
        != request.executable_identity
    {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::BindingDrift,
            "FS-UAE executable changed since authorization",
        ));
    }
    let content = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(request.selected_content_path.clone()),
        requires_mount: false,
        provenance: "fresh FS-UAE preflight".into(),
    };
    let firmware = platform_firmware(&inspection);
    let candidate = LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "fsuae",
            profile_id: profile.profile_id.clone(),
            profile_path: Some(profile.configuration_root.clone()),
        },
        content,
        firmware,
        blockers: vec![],
        warnings: vec![],
        readiness: if firmware == FirmwareReadiness::PresentUnverified {
            LaunchReadiness::ReadyWithWarnings
        } else {
            LaunchReadiness::Ready
        },
        preference: CandidatePreference::SoleEligible,
    };
    let plan = build_fsuae_command_plan(
        identity,
        Some(request.expected_game_key.as_str()),
        &candidate,
        profile,
        &inspection,
        &Ok(binding),
        request.caps_backend_available,
    );
    if !plan.blockers.is_empty() {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::CommandBlocked,
            format!("FS-UAE command plan has {} blocker(s)", plan.blockers.len()),
        ));
    }
    let command: FsUaeCommand = plan.command.expect("unblocked plan has a command");
    if file_identity(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            FsUaeLaunchPreflightErrorKind::ContentChanged,
            "selected media changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    })
}

pub fn spawn_fsuae(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}
