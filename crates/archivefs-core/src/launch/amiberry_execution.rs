//! Fresh Amiberry launch preflight and shared-process execution.
//!
//! The command planner remains the authority for supported media and argv.
//! This module only revalidates the facts authorized by that plan immediately
//! before spawn, then delegates process creation to the shared watcher.

use std::fs;
use std::path::Path;

use crate::launch::amiberry_command::{
    AmiberryCommand, AmiberryCommandPlan, AmiberryKickstartEvidence, AmiberryLaunchRequest,
    build_amiberry_command_plan,
};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::readiness::LaunchBlocker;
use crate::patch_manager::AmigaMachineProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmiberryLaunchPreflightErrorKind {
    BindingUnavailable,
    BindingDrift,
    ProfileUnavailable,
    ProfileDrift,
    ContentUnavailable,
    ContentDrift,
    KickstartUnavailable,
    KickstartDrift,
    MachineDrift,
    CommandBlocked,
    CommandMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryLaunchPreflightError {
    pub kind: AmiberryLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(
    kind: AmiberryLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> AmiberryLaunchPreflightError {
    AmiberryLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn checked_file(
    path: &Path,
    expected: CapturedFileIdentity,
    kind: AmiberryLaunchPreflightErrorKind,
    label: &str,
) -> Result<(), AmiberryLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|cause| error(kind, format!("{label} is unavailable: {cause}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(kind, format!("{label} is not a safe regular file")));
    }
    if CapturedFileIdentity::capture(&metadata) != expected {
        return Err(error(kind, format!("{label} changed since authorization")));
    }
    Ok(())
}

fn check_executable(
    path: &Path,
    expected: CapturedFileIdentity,
) -> Result<(), AmiberryLaunchPreflightError> {
    checked_file(
        path,
        expected,
        AmiberryLaunchPreflightErrorKind::BindingDrift,
        "Amiberry executable",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::symlink_metadata(path)
            .map_err(|cause| {
                error(
                    AmiberryLaunchPreflightErrorKind::BindingUnavailable,
                    cause.to_string(),
                )
            })?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(error(
                AmiberryLaunchPreflightErrorKind::BindingUnavailable,
                "Amiberry executable is not executable",
            ));
        }
    }
    Ok(())
}

fn check_kickstart(
    authorized: &AmiberryKickstartEvidence,
    current: &AmiberryKickstartEvidence,
) -> Result<(), AmiberryLaunchPreflightError> {
    if authorized != current {
        return Err(error(
            AmiberryLaunchPreflightErrorKind::KickstartDrift,
            "Kickstart evidence changed since authorization",
        ));
    }
    let Some(path) = current.path.as_ref() else {
        return Ok(());
    };
    let Some(identity) = current.identity else {
        return Err(error(
            AmiberryLaunchPreflightErrorKind::KickstartUnavailable,
            "configured Kickstart has no captured identity",
        ));
    };
    checked_file(
        path,
        identity,
        AmiberryLaunchPreflightErrorKind::KickstartDrift,
        "Kickstart",
    )
}

fn prepared(command: AmiberryCommand) -> PreparedProcessCommand {
    PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    }
}

/// Revalidates the authorized Amiberry binding, profile, content, machine,
/// Kickstart, and capability evidence immediately before spawning.
pub fn preflight_amiberry_launch(
    request: &AmiberryLaunchRequest,
    identity: &CanonicalIdentityStatus,
    current_machine: &AmigaMachineProfile,
    current_kickstart: &AmiberryKickstartEvidence,
    current_ipf_backend_available: bool,
) -> Result<PreparedProcessCommand, AmiberryLaunchPreflightError> {
    check_executable(&request.executable, request.executable_identity)?;
    checked_file(
        &request.profile,
        request.profile_identity,
        AmiberryLaunchPreflightErrorKind::ProfileDrift,
        "Amiberry profile",
    )?;
    checked_file(
        &request.selected_content,
        request.content_identity,
        AmiberryLaunchPreflightErrorKind::ContentDrift,
        "selected Amiga media",
    )?;
    check_kickstart(&request.kickstart_evidence, current_kickstart)?;

    if request.machine_model.trim().is_empty()
        || current_machine.machine_model.as_deref() != Some(request.machine_model.as_str())
    {
        return Err(error(
            AmiberryLaunchPreflightErrorKind::MachineDrift,
            "machine evidence no longer matches the authorized request",
        ));
    }

    let mut fresh_request = request.clone();
    fresh_request.kickstart_evidence = current_kickstart.clone();
    fresh_request.ipf_backend_available = current_ipf_backend_available;
    let plan: AmiberryCommandPlan =
        build_amiberry_command_plan(identity, &fresh_request, current_machine);
    if let Some(blocker) = plan.blockers.into_iter().next() {
        return Err(error(
            AmiberryLaunchPreflightErrorKind::CommandBlocked,
            format_blocker(blocker),
        ));
    }
    plan.command.map(prepared).ok_or_else(|| {
        error(
            AmiberryLaunchPreflightErrorKind::CommandMissing,
            "Amiberry produced no command",
        )
    })
}

fn format_blocker(blocker: LaunchBlocker) -> String {
    format!("Amiberry command plan blocked launch: {blocker:?}")
}

pub use crate::launch::process_spawn::ProcessExitReport as AmiberryLaunchExitReport;

pub struct LaunchedAmiberryProcess {
    pub pid: u32,
    pub command: PreparedProcessCommand,
    watched: WatchedProcess,
}

impl LaunchedAmiberryProcess {
    pub fn poll(&mut self) -> Option<&AmiberryLaunchExitReport> {
        self.watched.poll()
    }

    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}

#[derive(Debug)]
pub enum AmiberryLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum AmiberryLaunchExecutionError {
    Preflight(AmiberryLaunchPreflightError),
    Spawn(AmiberryLaunchSpawnError),
}

impl From<AmiberryLaunchPreflightError> for AmiberryLaunchExecutionError {
    fn from(value: AmiberryLaunchPreflightError) -> Self {
        Self::Preflight(value)
    }
}

impl From<AmiberryLaunchSpawnError> for AmiberryLaunchExecutionError {
    fn from(value: AmiberryLaunchSpawnError) -> Self {
        Self::Spawn(value)
    }
}

pub fn spawn_amiberry(
    command: PreparedProcessCommand,
) -> Result<LaunchedAmiberryProcess, AmiberryLaunchSpawnError> {
    let watched =
        process_spawn::spawn_watched_process(&command).map_err(AmiberryLaunchSpawnError::Spawn)?;
    Ok(LaunchedAmiberryProcess {
        pid: watched.pid,
        command,
        watched,
    })
}

pub fn preflight_and_launch_amiberry(
    request: &AmiberryLaunchRequest,
    identity: &CanonicalIdentityStatus,
    current_machine: &AmigaMachineProfile,
    current_kickstart: &AmiberryKickstartEvidence,
    current_ipf_backend_available: bool,
) -> Result<LaunchedAmiberryProcess, AmiberryLaunchExecutionError> {
    let command = preflight_amiberry_launch(
        request,
        identity,
        current_machine,
        current_kickstart,
        current_ipf_backend_available,
    )?;
    Ok(spawn_amiberry(command)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::amiberry_command::{AmiberryLaunchRequest, AmiberryMediaFormat};
    use crate::launch::planning::ResolvedIdentity;
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: std::path::PathBuf,
        request: AmiberryLaunchRequest,
        machine: AmigaMachineProfile,
        kickstart: AmiberryKickstartEvidence,
        identity: CanonicalIdentityStatus,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "archivefs-amiberry-execution-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            let executable = root.join("amiberry");
            let profile = root.join("game profile.uae");
            let content = root.join("disk one.adf");
            let kickstart_path = root.join("kick.rom");
            File::create(&profile)
                .unwrap()
                .write_all(b"config")
                .unwrap();
            File::create(&content).unwrap().write_all(b"disk").unwrap();
            File::create(&kickstart_path)
                .unwrap()
                .write_all(b"kick")
                .unwrap();
            File::create(&executable)
                .unwrap()
                .write_all(b"#!/bin/sh\nexit 0\n")
                .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let capture = |path: &std::path::Path| {
                CapturedFileIdentity::capture(&fs::metadata(path).unwrap())
            };
            let mut machine = AmigaMachineProfile {
                machine_model: Some("A500".into()),
                ..Default::default()
            };
            machine.floppy_mounts.push(content.clone());
            let kickstart = AmiberryKickstartEvidence {
                path: Some(kickstart_path.clone()),
                state: crate::patch_manager::AmigaKickstartState::PresentUnverified,
                hash_verified: false,
                identity: Some(capture(&kickstart_path)),
            };
            Self {
                root: root.clone(),
                request: AmiberryLaunchRequest {
                    executable: executable.clone(),
                    profile: profile.clone(),
                    canonical_platform: "Amiga".into(),
                    machine_model: "A500".into(),
                    selected_content: content.clone(),
                    media_format: AmiberryMediaFormat::Adf,
                    kickstart_evidence: kickstart.clone(),
                    identity_evidence: "structural+profile".into(),
                    content_identity: capture(&content),
                    profile_identity: capture(&profile),
                    executable_identity: capture(&executable),
                    ipf_backend_available: false,
                },
                machine,
                kickstart,
                identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                    platform_id: "Amiga".into(),
                    game_key: "fixture".into(),
                }),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn valid_preflight_and_spawn_use_shared_direct_argv() {
        let fixture = Fixture::new();
        let command = preflight_amiberry_launch(
            &fixture.request,
            &fixture.identity,
            &fixture.machine,
            &fixture.kickstart,
            false,
        )
        .unwrap();
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("--config"),
                fixture.request.profile.clone().into_os_string()
            ]
        );
        let mut process = spawn_amiberry(command).unwrap();
        while process.is_running() {
            std::thread::sleep(std::time::Duration::from_millis(2));
            let _ = process.poll();
        }
        assert!(process.poll().unwrap().status.as_ref().unwrap().success());
    }

    #[test]
    fn drift_and_unsafe_inputs_fail_closed() {
        let fixture = Fixture::new();
        let changed = fixture.request.clone();
        File::create(&changed.profile)
            .unwrap()
            .write_all(b"changed profile")
            .unwrap();
        assert_eq!(
            preflight_amiberry_launch(
                &changed,
                &fixture.identity,
                &fixture.machine,
                &fixture.kickstart,
                false
            )
            .unwrap_err()
            .kind,
            AmiberryLaunchPreflightErrorKind::ProfileDrift
        );
        let machine_fixture = Fixture::new();
        let mut machine = machine_fixture.machine.clone();
        machine.machine_model = Some("A1200".into());
        assert_eq!(
            preflight_amiberry_launch(
                &machine_fixture.request,
                &machine_fixture.identity,
                &machine,
                &machine_fixture.kickstart,
                false
            )
            .unwrap_err()
            .kind,
            AmiberryLaunchPreflightErrorKind::MachineDrift
        );
        assert_eq!(
            preflight_amiberry_launch(
                &machine_fixture.request,
                &machine_fixture.identity,
                &machine_fixture.machine,
                &machine_fixture.kickstart,
                false
            )
            .unwrap()
            .arguments[0],
            OsString::from("--config")
        );
    }
}
