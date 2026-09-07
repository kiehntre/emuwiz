//! Fresh XRoar launch preflight and watched-process execution.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::planning::{
    CandidatePreference, CanonicalIdentityStatus, LaunchCandidate, LaunchContainerKind,
    LaunchContentKind, LaunchContentRef, LaunchTarget,
};
use crate::launch::process_spawn::{
    CapturedFileIdentity, PreparedProcessCommand, WatchedProcess, capture_file_identity,
    spawn_watched_process,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::launch::xroar_command::{XRoarCommand, build_xroar_command_plan, xroar_media_format};
use crate::patch_manager::{
    XRoarFirmwareState, XRoarProfileDiscoveryRoots, discover_xroar_profiles,
    resolve_xroar_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub expected_content_identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XRoarLaunchPreflightErrorKind {
    ContentUnavailable,
    ContentChangedBeforeSpawn,
    ContentFormatUnsupported,
    IdentityMismatch,
    ProfileNotFound,
    ProfileIneligible,
    ExecutableBindingDrift,
    FirmwareUnavailable,
    CommandBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarLaunchPreflightError {
    pub kind: XRoarLaunchPreflightErrorKind,
    pub detail: String,
}

#[derive(Debug)]
pub enum XRoarLaunchExecutionError {
    Preflight(XRoarLaunchPreflightError),
    Spawn(std::io::Error),
}

fn error(
    kind: XRoarLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> XRoarLaunchPreflightError {
    XRoarLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn direct_file(
    path: &Path,
) -> Result<(fs::Metadata, CapturedFileIdentity), XRoarLaunchPreflightError> {
    let (metadata, identity) = capture_file_identity(path).map_err(|_| {
        error(
            XRoarLaunchPreflightErrorKind::ContentUnavailable,
            "selected XRoar content is unavailable",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(
            XRoarLaunchPreflightErrorKind::ContentUnavailable,
            "selected XRoar content is not a regular non-symlink file",
        ));
    }
    Ok((metadata, identity))
}

fn candidate(request: &XRoarLaunchRequest, firmware: FirmwareReadiness) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "xroar",
            profile_id: request.profile_id.clone(),
            profile_path: Some(request.expected_executable.clone()),
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::Executable),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(request.selected_content_path.clone()),
            requires_mount: false,
            provenance: "verified Dragon/CoCo CAS content".into(),
        },
        firmware,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

pub fn preflight_xroar_launch(
    request: &XRoarLaunchRequest,
    roots: &XRoarProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<XRoarCommand, XRoarLaunchPreflightError> {
    let (_, current_identity) = direct_file(&request.selected_content_path)?;
    if current_identity != request.expected_content_identity {
        return Err(error(
            XRoarLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected XRoar content changed since authorization",
        ));
    }
    if xroar_media_format(&request.selected_content_path).is_none() {
        return Err(error(
            XRoarLaunchPreflightErrorKind::ContentFormatUnsupported,
            "XRoar V1 supports only direct .cas tape images",
        ));
    }
    let resolved_platform = match identity {
        CanonicalIdentityStatus::Resolved(value) => value.platform_id.as_str(),
        _ => "",
    };
    if resolved_platform != request.expected_platform_id
        || !matches!(resolved_platform, "Dragon / Tandy CoCo")
    {
        return Err(error(
            XRoarLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity is not the authorized Dragon/CoCo family identity",
        ));
    }
    let profile = discover_xroar_profiles(roots)
        .profiles
        .into_iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                XRoarLaunchPreflightErrorKind::ProfileNotFound,
                "authorized XRoar profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            XRoarLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "XRoar profile is not eligible".into()),
        ));
    }
    if profile.executable.path != request.expected_executable {
        return Err(error(
            XRoarLaunchPreflightErrorKind::ExecutableBindingDrift,
            "authorized XRoar executable binding changed",
        ));
    }
    let firmware = match profile.firmware {
        XRoarFirmwareState::Verified => FirmwareReadiness::Verified,
        XRoarFirmwareState::PresentUnverified => FirmwareReadiness::PresentUnverified,
        XRoarFirmwareState::Missing => FirmwareReadiness::Missing,
        XRoarFirmwareState::Unknown => FirmwareReadiness::Unknown,
    };
    let plan = build_xroar_command_plan(identity, &candidate(request, firmware), &profile);
    let Some(command) = plan.command else {
        return Err(error(
            XRoarLaunchPreflightErrorKind::CommandBlocked,
            plan.blockers
                .into_iter()
                .map(|blocker| blocker.detail)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    };
    resolve_xroar_native_launch_binding(&profile).map_err(|detail| {
        error(
            XRoarLaunchPreflightErrorKind::ExecutableBindingDrift,
            detail,
        )
    })?;
    Ok(command)
}

pub fn spawn_xroar(command: &XRoarCommand) -> std::io::Result<WatchedProcess> {
    spawn_watched_process(&PreparedProcessCommand {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
    })
}

pub fn preflight_and_launch_xroar(
    request: &XRoarLaunchRequest,
    roots: &XRoarProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<WatchedProcess, XRoarLaunchExecutionError> {
    let command = preflight_xroar_launch(request, roots, identity)
        .map_err(XRoarLaunchExecutionError::Preflight)?;
    spawn_xroar(&command).map_err(XRoarLaunchExecutionError::Spawn)
}
