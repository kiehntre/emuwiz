//! Fresh, read-only Hatari launch preflight and native process spawn.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::hatari_command::{
    HatariCommand, build_hatari_command_plan, hatari_media_format,
};
use crate::launch::planning::{
    CandidatePreference, CanonicalIdentityStatus, LaunchCandidate, LaunchContainerKind,
    LaunchContentRef, LaunchTarget,
};
use crate::launch::process_spawn::{CapturedFileIdentity, PreparedProcessCommand, WatchedProcess};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::{
    HatariIdentityState, HatariMachineModel, HatariProfileDiscoveryRoots,
    HatariSelectedGameRequest, HatariTosReference, discover_hatari_profiles, inspect_hatari_game,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub expected_config_path: PathBuf,
    pub expected_machine_model: HatariMachineModel,
    pub disk_drive: char,
    pub ipf_backend_available: bool,
    pub expected_content_identity: Option<CapturedFileIdentity>,
    pub expected_config_identity: Option<CapturedFileIdentity>,
    pub tos_references: Vec<HatariTosReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HatariLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    ContentNotRegularFile,
    ContentChangedBeforeSpawn,
    ContentFormatUnsupported,
    IdentityMismatch,
    ProfileNotFound,
    ProfileIneligible,
    ExecutableMissing,
    ExecutableUnsafe,
    ExecutableNotExecutable,
    ConfigurationMissing,
    ConfigurationChanged,
    MachineMismatch,
    TosUnavailable,
    BindingDrift,
    CommandBlocked,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariLaunchPreflightError {
    pub kind: HatariLaunchPreflightErrorKind,
    pub detail: String,
}
#[derive(Debug)]
pub enum HatariLaunchExecutionError {
    Preflight(HatariLaunchPreflightError),
    Spawn(std::io::Error),
}
fn error(
    kind: HatariLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> HatariLaunchPreflightError {
    HatariLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn regular_no_symlink(
    path: &Path,
    kind: HatariLaunchPreflightErrorKind,
    symlink_kind: HatariLaunchPreflightErrorKind,
) -> Result<fs::Metadata, HatariLaunchPreflightError> {
    let meta = fs::symlink_metadata(path).map_err(|_| error(kind, "path is not available"))?;
    if meta.file_type().is_symlink() {
        return Err(error(symlink_kind, "path is a symlink"));
    }
    if !meta.is_file() {
        return Err(error(kind, "path is not a regular file"));
    }
    Ok(meta)
}

pub fn preflight_hatari_launch(
    request: &HatariLaunchRequest,
    roots: &HatariProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<HatariCommand, HatariLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            HatariLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content path must be absolute",
        ));
    }
    let content_meta = regular_no_symlink(
        &request.selected_content_path,
        HatariLaunchPreflightErrorKind::ContentNotFound,
        HatariLaunchPreflightErrorKind::ContentIsSymlink,
    )?;
    if hatari_media_format(&request.selected_content_path).is_none() {
        return Err(error(
            HatariLaunchPreflightErrorKind::ContentFormatUnsupported,
            "unsupported Hatari media format",
        ));
    }
    if let Some(expected) = request.expected_content_identity
        && CapturedFileIdentity::capture(&content_meta) != expected
    {
        return Err(error(
            HatariLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected media changed since authorization",
        ));
    }
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(value)
            if value.platform_id == request.expected_platform_id
                && value.platform_id == "AtariST" =>
        {
            value
        }
        _ => {
            return Err(error(
                HatariLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity is not the authorized AtariST identity",
            ));
        }
    };
    let discovery = discover_hatari_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                HatariLaunchPreflightErrorKind::ProfileNotFound,
                "authorized Hatari profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            HatariLaunchPreflightErrorKind::ProfileIneligible,
            "authorized Hatari profile is no longer eligible",
        ));
    }
    if profile.config_path != request.expected_config_path {
        return Err(error(
            HatariLaunchPreflightErrorKind::ConfigurationChanged,
            "Hatari profile configuration binding changed",
        ));
    }
    let config_meta = regular_no_symlink(
        &profile.config_path,
        HatariLaunchPreflightErrorKind::ConfigurationMissing,
        HatariLaunchPreflightErrorKind::ConfigurationChanged,
    )?;
    // The session never loads this file: only allow-listed hardware keys
    // are read from it at spawn, and the machine model is re-checked below.
    // Hatari rewriting it (any instance saving on exit) is therefore not a
    // reason to refuse; only a changed machine model is.
    let _ = (&config_meta, request.expected_config_identity);
    let executable = profile
        .executable_candidates
        .iter()
        .find(|candidate| candidate.path == request.expected_executable)
        .ok_or_else(|| {
            error(
                HatariLaunchPreflightErrorKind::BindingDrift,
                "authorized Hatari executable binding was not rediscovered",
            )
        })?;
    let exe_meta = regular_no_symlink(
        &executable.path,
        HatariLaunchPreflightErrorKind::ExecutableMissing,
        HatariLaunchPreflightErrorKind::ExecutableUnsafe,
    )?;
    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&exe_meta.permissions()) & 0o111 == 0 {
        return Err(error(
            HatariLaunchPreflightErrorKind::ExecutableNotExecutable,
            "Hatari executable is not executable",
        ));
    }
    let inspection = inspect_hatari_game(
        profile,
        &HatariSelectedGameRequest {
            canonical_platform: Some("AtariST".into()),
            identity_state: HatariIdentityState::Verified,
            verified_title: Some(resolved.game_key.clone()),
        },
        &request.tos_references,
    );
    if inspection.config.machine.model != request.expected_machine_model {
        return Err(error(
            HatariLaunchPreflightErrorKind::MachineMismatch,
            "Hatari machine model changed since authorization",
        ));
    }
    let firmware =
        crate::launch::readiness::hatari_firmware_readiness(inspection.health.tos.health);
    let readiness = match firmware {
        FirmwareReadiness::Verified => LaunchReadiness::Ready,
        FirmwareReadiness::PresentUnverified => LaunchReadiness::ReadyWithWarnings,
        FirmwareReadiness::Missing | FirmwareReadiness::Unknown => LaunchReadiness::Blocked,
        FirmwareReadiness::NotRequired => LaunchReadiness::Ready,
    };
    let candidate = LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "hatari",
            profile_id: profile.profile_id.clone(),
            profile_path: Some(profile.config_path.clone()),
        },
        content: LaunchContentRef {
            kind: None,
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(request.selected_content_path.clone()),
            requires_mount: false,
            provenance: "fresh Hatari media preflight".into(),
        },
        firmware,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness,
        preference: CandidatePreference::SoleEligible,
    };
    let plan = build_hatari_command_plan(
        identity,
        &candidate,
        profile,
        &inspection,
        &request.selected_content_path,
        request.expected_machine_model,
        request.disk_drive,
        request.ipf_backend_available,
    );
    match (plan.command, plan.blockers.is_empty()) {
        (Some(command), true) => Ok(command),
        (_, _) => Err(error(
            HatariLaunchPreflightErrorKind::CommandBlocked,
            format!("Hatari launch was blocked: {:?}", plan.blockers),
        )),
    }
}

/// Spawns Hatari inside a private, disposable session: fresh `HOME`/`XDG_*`,
/// a read-only copy of the selected disk, drive B off, floppy write
/// protection on, and no inherited configuration. The command's own
/// `arguments` are a preview and are never passed through.
pub fn spawn_hatari(command: &HatariCommand) -> std::io::Result<WatchedProcess> {
    let sessions_root = crate::app_dirs::data_dir()
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .join("hatari-sessions");
    spawn_hatari_in(command, &sessions_root)
}

pub fn spawn_hatari_in(
    command: &HatariCommand,
    sessions_root: &Path,
) -> std::io::Result<WatchedProcess> {
    spawn_hatari_session(
        command,
        sessions_root,
        &crate::launch::hatari_session::SessionOptions::default(),
    )
}

fn spawn_hatari_session(
    command: &HatariCommand,
    sessions_root: &Path,
    options: &crate::launch::hatari_session::SessionOptions<'_>,
) -> std::io::Result<WatchedProcess> {
    let session =
        crate::launch::hatari_session::prepare_session_with(command, sessions_root, options)
            .map_err(std::io::Error::other)?;
    let spawned = crate::launch::process_spawn::spawn_watched_process_with_environment(
        &PreparedProcessCommand {
            executable: command.executable.clone(),
            arguments: session.arguments.clone(),
            working_directory: Some(session.working_directory.clone()),
        },
        &session.environment,
    );
    let process = match spawned {
        Ok(process) => process,
        Err(error) => {
            if let Some(save_dir) = &session.save_dir {
                crate::launch::hatari_session::release_game_lock(save_dir);
            }
            return Err(error);
        }
    };
    // Hatari is already running on the saved disk; a lock-record failure is
    // logged but must not orphan the running game.
    if let Err(error) = crate::launch::hatari_session::record_session_pid(&session, process.pid) {
        eprintln!("archivefs: {error}");
    }
    Ok(process)
}

/// The full-file SHA-256 the authorised identity was derived from, when the
/// identity is a local media hash.
fn authorised_media_sha256(identity: &CanonicalIdentityStatus) -> Option<String> {
    match identity {
        CanonicalIdentityStatus::Resolved(value) => value
            .game_key
            .strip_prefix("media-sha256:")
            .map(str::to_string),
        _ => None,
    }
}
pub fn preflight_and_launch_hatari(
    request: &HatariLaunchRequest,
    roots: &HatariProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<WatchedProcess, HatariLaunchExecutionError> {
    let command = preflight_hatari_launch(request, roots, identity)
        .map_err(HatariLaunchExecutionError::Preflight)?;
    let data_dir = crate::app_dirs::data_dir().map_err(|error| {
        HatariLaunchExecutionError::Spawn(std::io::Error::other(error.to_string()))
    })?;
    let expected = authorised_media_sha256(identity);
    let saves_root = data_dir.join("hatari-saves");
    spawn_hatari_session(
        &command,
        &data_dir.join("hatari-sessions"),
        &crate::launch::hatari_session::SessionOptions {
            expected_sha256: expected.as_deref(),
            saves_root: Some(&saves_root),
            hardware: crate::launch::hatari_session::read_hardware_arguments(
                &request.expected_config_path,
                request.expected_machine_model,
            ),
        },
    )
    .map_err(HatariLaunchExecutionError::Spawn)
}

#[cfg(test)]
mod tests;
