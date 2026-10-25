//! Fresh Tsugaru launch preflight and watched-process execution.

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
use crate::launch::tsugaru_command::{
    TsugaruCommand, build_tsugaru_command_plan, tsugaru_media_format,
};
use crate::patch_manager::{
    TsugaruProfileDiscoveryRoots, discover_tsugaru_profiles, resolve_tsugaru_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsugaruLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub expected_rom_directory: PathBuf,
    pub expected_content_identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsugaruLaunchPreflightErrorKind {
    ContentUnavailable,
    ContentChangedBeforeSpawn,
    ContentFormatUnsupported,
    IdentityMismatch,
    ProfileNotFound,
    ProfileIneligible,
    ExecutableBindingDrift,
    RomDirectoryDrift,
    FirmwareUnavailable,
    CommandBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsugaruLaunchPreflightError {
    pub kind: TsugaruLaunchPreflightErrorKind,
    pub detail: String,
}

#[derive(Debug)]
pub enum TsugaruLaunchExecutionError {
    Preflight(TsugaruLaunchPreflightError),
    Spawn(std::io::Error),
}

fn error(
    kind: TsugaruLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> TsugaruLaunchPreflightError {
    TsugaruLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn direct_file(
    path: &Path,
) -> Result<(fs::Metadata, CapturedFileIdentity), TsugaruLaunchPreflightError> {
    let (metadata, identity) = capture_file_identity(path).map_err(|_| {
        error(
            TsugaruLaunchPreflightErrorKind::ContentUnavailable,
            "selected Tsugaru content is unavailable",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(
            TsugaruLaunchPreflightErrorKind::ContentUnavailable,
            "selected Tsugaru content is not a regular non-symlink file",
        ));
    }
    Ok((metadata, identity))
}

fn candidate(request: &TsugaruLaunchRequest, firmware: FirmwareReadiness) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "tsugaru",
            profile_id: request.profile_id.clone(),
            profile_path: Some(request.expected_executable.clone()),
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(request.selected_content_path.clone()),
            requires_mount: false,
            provenance: "verified FM Towns content".into(),
        },
        firmware,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

pub fn preflight_tsugaru_launch(
    request: &TsugaruLaunchRequest,
    roots: &TsugaruProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<TsugaruCommand, TsugaruLaunchPreflightError> {
    let (_, current_content_identity) = direct_file(&request.selected_content_path)?;
    if current_content_identity != request.expected_content_identity {
        return Err(error(
            TsugaruLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected Tsugaru content changed since authorization",
        ));
    }
    if tsugaru_media_format(&request.selected_content_path).is_none() {
        return Err(error(
            TsugaruLaunchPreflightErrorKind::ContentFormatUnsupported,
            "Tsugaru V1 supports only ISO, CUE, and MDS CD images",
        ));
    }
    if !matches!(identity, CanonicalIdentityStatus::Resolved(value) if value.platform_id == request.expected_platform_id && value.platform_id == "FM Towns")
    {
        return Err(error(
            TsugaruLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity is not the authorized FM Towns identity",
        ));
    }
    let profile = discover_tsugaru_profiles(roots)
        .profiles
        .into_iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                TsugaruLaunchPreflightErrorKind::ProfileNotFound,
                "authorized Tsugaru profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            TsugaruLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "Tsugaru profile is not eligible".into()),
        ));
    }
    if profile.executable.path != request.expected_executable {
        return Err(error(
            TsugaruLaunchPreflightErrorKind::ExecutableBindingDrift,
            "authorized Tsugaru executable binding changed",
        ));
    }
    if profile.rom_directory.as_deref() != Some(request.expected_rom_directory.as_path()) {
        return Err(error(
            TsugaruLaunchPreflightErrorKind::RomDirectoryDrift,
            "authorized Tsugaru ROM directory binding changed",
        ));
    }
    let firmware = match profile.firmware {
        crate::patch_manager::TsugaruFirmwareState::Verified => FirmwareReadiness::Verified,
        crate::patch_manager::TsugaruFirmwareState::PresentUnverified => {
            FirmwareReadiness::PresentUnverified
        }
        crate::patch_manager::TsugaruFirmwareState::Missing => FirmwareReadiness::Missing,
        crate::patch_manager::TsugaruFirmwareState::Unknown => FirmwareReadiness::Unknown,
    };
    let candidate = candidate(request, firmware);
    let plan = build_tsugaru_command_plan(identity, &candidate, &profile);
    let Some(command) = plan.command else {
        return Err(error(
            TsugaruLaunchPreflightErrorKind::CommandBlocked,
            plan.blockers
                .into_iter()
                .map(|blocker| blocker.detail)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    };
    let _ = resolve_tsugaru_native_launch_binding(&profile).map_err(|detail| {
        error(
            TsugaruLaunchPreflightErrorKind::ExecutableBindingDrift,
            detail,
        )
    })?;
    Ok(command)
}

pub fn spawn_tsugaru(command: &TsugaruCommand) -> std::io::Result<WatchedProcess> {
    spawn_watched_process(&PreparedProcessCommand {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
    })
}

pub fn preflight_and_launch_tsugaru(
    request: &TsugaruLaunchRequest,
    roots: &TsugaruProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<WatchedProcess, TsugaruLaunchExecutionError> {
    let command = preflight_tsugaru_launch(request, roots, identity)
        .map_err(TsugaruLaunchExecutionError::Preflight)?;
    spawn_tsugaru(&command).map_err(TsugaruLaunchExecutionError::Spawn)
}
