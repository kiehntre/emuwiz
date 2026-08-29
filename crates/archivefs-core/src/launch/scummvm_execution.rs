//! Native ScummVM launch preflight and execution for extracted game folders.
//!
//! Preflight re-runs the installed ScummVM detector against the selected
//! folder, then rebuilds the structured command from that fresh evidence.
//! Spawning delegates to the shared direct-argv process watcher; no shell is
//! ever involved and no ScummVM configuration is rewritten or isolated for
//! the actual launch.

use std::fs;
use std::path::PathBuf;

use crate::game_identity::inspect_scummvm_directory_with_executable;
use crate::launch::evidence_bridge::canonical_identity_from_game_report;
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{self, PreparedProcessCommand, WatchedProcess};
use crate::launch::scummvm_command::{
    ScummVmCommand, build_scummvm_command_plan, resolve_scummvm_native_launch_binding_at,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmLaunchRequest {
    pub selected_game_folder: PathBuf,
    pub expected_game_key: String,
    pub expected_executable: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScummVmLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    ContentNotDirectory,
    IdentityUnresolved,
    IdentityMismatch,
    ScummVmGameIdUnavailable,
    BindingUnavailable,
    BindingDrift,
    CommandBlocked,
    CommandMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmLaunchPreflightError {
    pub kind: ScummVmLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(
    kind: ScummVmLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> ScummVmLaunchPreflightError {
    ScummVmLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum ScummVmLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum ScummVmLaunchExecutionError {
    Preflight(ScummVmLaunchPreflightError),
    Spawn(ScummVmLaunchSpawnError),
}

impl From<ScummVmLaunchPreflightError> for ScummVmLaunchExecutionError {
    fn from(value: ScummVmLaunchPreflightError) -> Self {
        Self::Preflight(value)
    }
}

impl From<ScummVmLaunchSpawnError> for ScummVmLaunchExecutionError {
    fn from(value: ScummVmLaunchSpawnError) -> Self {
        Self::Spawn(value)
    }
}

/// Revalidates the selected folder, detector evidence, and executable before
/// returning the command that may be spawned. The expected executable is
/// checked exactly, preventing silent substitution after readiness inspection.
pub fn preflight_scummvm_launch(
    request: &ScummVmLaunchRequest,
) -> Result<ScummVmCommand, ScummVmLaunchPreflightError> {
    let folder = &request.selected_game_folder;
    if !folder.is_absolute() {
        return Err(error(
            ScummVmLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "ScummVM game folder must be an absolute path",
        ));
    }
    let metadata = fs::symlink_metadata(folder).map_err(|io_error| {
        error(
            ScummVmLaunchPreflightErrorKind::ContentNotFound,
            format!("game folder is unavailable: {io_error}"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            ScummVmLaunchPreflightErrorKind::ContentIsSymlink,
            "ScummVM game folder is a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(error(
            ScummVmLaunchPreflightErrorKind::ContentNotDirectory,
            "ScummVM launch requires an extracted game directory",
        ));
    }

    let binding = resolve_scummvm_native_launch_binding_at(&request.expected_executable)
        .map_err(|detail| error(ScummVmLaunchPreflightErrorKind::BindingUnavailable, detail))?;
    let report = inspect_scummvm_directory_with_executable(folder, &binding.executable);
    let (identity, facts) = canonical_identity_from_game_report(&report);
    let verified_id = report.verified_scummvm_game_id().ok_or_else(|| {
        error(
            ScummVmLaunchPreflightErrorKind::ScummVmGameIdUnavailable,
            "fresh ScummVM detection did not provide a verified engine:game ID",
        )
    })?;
    let CanonicalIdentityStatus::Resolved(resolved) = &identity else {
        let kind = match identity {
            CanonicalIdentityStatus::Conflicting => {
                ScummVmLaunchPreflightErrorKind::IdentityMismatch
            }
            CanonicalIdentityStatus::Unknown => ScummVmLaunchPreflightErrorKind::IdentityUnresolved,
            CanonicalIdentityStatus::Resolved(_) => unreachable!(),
        };
        return Err(error(
            kind,
            "fresh ScummVM identity did not resolve uniquely",
        ));
    };
    if resolved.platform_id != "ScummVM"
        || resolved.game_key != request.expected_game_key
        || verified_id != request.expected_game_key
        || !facts.iter().any(|fact| {
            matches!(fact, crate::launch::input_projection::VerifiedIdentityFact::ScummVmGameId(id) if id == verified_id)
        })
    {
        return Err(error(
            ScummVmLaunchPreflightErrorKind::IdentityMismatch,
            "fresh detector identity does not match the user-authorized ScummVM game ID",
        ));
    }

    let command_plan =
        build_scummvm_command_plan(&identity, Some(verified_id), folder, &Ok(binding));
    if !command_plan.blockers.is_empty() {
        return Err(error(
            ScummVmLaunchPreflightErrorKind::CommandBlocked,
            format!(
                "ScummVM command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    command_plan.command.ok_or_else(|| {
        error(
            ScummVmLaunchPreflightErrorKind::CommandMissing,
            "ScummVM command plan produced no command",
        )
    })
}

pub use crate::launch::process_spawn::ProcessExitReport as ScummVmLaunchExitReport;

pub struct LaunchedScummVmProcess {
    pub pid: u32,
    pub command: ScummVmCommand,
    watched: WatchedProcess,
}

impl LaunchedScummVmProcess {
    pub fn poll(&mut self) -> Option<&ScummVmLaunchExitReport> {
        self.watched.poll()
    }

    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}

pub fn spawn_scummvm(
    command: ScummVmCommand,
) -> Result<LaunchedScummVmProcess, ScummVmLaunchSpawnError> {
    let prepared = PreparedProcessCommand {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(ScummVmLaunchSpawnError::Spawn)?;
    Ok(LaunchedScummVmProcess {
        pid: watched.pid,
        command,
        watched,
    })
}

pub fn preflight_and_launch_scummvm(
    request: &ScummVmLaunchRequest,
) -> Result<LaunchedScummVmProcess, ScummVmLaunchExecutionError> {
    Ok(spawn_scummvm(preflight_scummvm_launch(request)?)?)
}

#[cfg(test)]
mod tests;
