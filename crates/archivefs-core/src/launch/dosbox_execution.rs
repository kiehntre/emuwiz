//! Native DOSBox / DOSBox Staging launch preflight and execution.
//!
//! This is the smallest sibling adapter that spawns an already-built
//! [`DosBoxCommand`]. It adds no planning of its own: preflight re-validates
//! the exact binding and the verified config the readiness step already
//! selected, rebuilds the plan through the unchanged
//! [`build_dosbox_command_plan`], and refuses unless that plan is
//! blocker-free with a command. Spawning delegates to the shared
//! [`crate::launch::process_spawn`] watcher.
//!
//! # What this module never does
//!
//! - It never interprets, re-opens for command selection, or otherwise
//!   reads the meaning of the config's `[autoexec]` section. The only facts
//!   it handles are the executable path, the literal `-conf` flag, the
//!   verified `dosbox.conf` path, and the game directory. It never selects
//!   an EXE/COM/BAT, never synthesizes `mount`/`imgmount`/`boot`/`cd`
//!   arguments, and never rewrites `dosbox.conf` or any game file.
//! - It never runs a shell: the process is spawned via
//!   [`std::process::Command::new`] + `.args(..)` +
//!   `.current_dir(..)` (see [`crate::launch::process_spawn::spawn_watched_process`]),
//!   never `sh -c` and never one concatenated command string.
//! - It never performs a fresh `PATH` search for the executable - it
//!   revalidates the exact path the planner already chose, so discovery and
//!   execution cannot drift apart.
//! - It never adds a timeout, kill, relaunch, or "the game booted" claim: a
//!   successful spawn only means a process started.

use std::path::PathBuf;

use crate::launch::dosbox_command::{
    DosBoxCommand, DosBoxConfigStatus, DosBoxVariant, build_dosbox_command_plan,
    discover_dosbox_config, resolve_dosbox_native_launch_binding_at,
};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{self, PreparedProcessCommand, WatchedProcess};
use crate::safe_read::TrustedRoots;

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The immutable set of already-authorized facts identifying exactly which
/// DOSBox launch the user approved at readiness time. None of it is ever
/// passed to a shell or used to build argv directly - every field only
/// selects what preflight must revalidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxLaunchRequest {
    /// The absolute DOS game directory (also the launch working directory).
    pub game_directory: PathBuf,
    /// The already-resolved canonical identity the user was shown. Preflight
    /// never re-fuses identity; it passes this straight to the planner,
    /// which fails closed on anything that is not `Resolved(DOS)`.
    pub identity: CanonicalIdentityStatus,
    /// The exact DOSBox executable the readiness step selected - revalidated
    /// at this path, never re-discovered.
    pub expected_executable: PathBuf,
    /// The DOSBox family the readiness step selected.
    pub expected_variant: DosBoxVariant,
    /// The verified `dosbox.conf` path the readiness step selected - the
    /// freshly re-discovered config must still resolve `Verified` at exactly
    /// this path.
    pub expected_config_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DosBoxLaunchPreflightErrorKind {
    /// The game directory is not an absolute path.
    GameDirectoryNotAbsolute,
    /// The game directory does not exist or could not be inspected.
    GameDirectoryNotFound,
    /// The game directory is a symlink.
    GameDirectoryIsSymlink,
    /// The game directory is not a directory.
    GameDirectoryNotDirectory,
    /// The expected executable is gone, is a symlink, is not a regular
    /// file, or is no longer executable.
    ExecutableUnavailable,
    /// The freshly re-discovered `dosbox.conf` is missing, malformed, or has
    /// no `[autoexec]` section.
    ConfigNotVerified,
    /// A verified config was found, but not at the authorized path - it was
    /// moved, replaced, or a different file now answers to `dosbox.conf`.
    ConfigPathDrift,
    /// The rebuilt command plan reported one or more blockers.
    CommandBlocked,
    /// The rebuilt command plan produced no command.
    CommandMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxLaunchPreflightError {
    pub kind: DosBoxLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(
    kind: DosBoxLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> DosBoxLaunchPreflightError {
    DosBoxLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum DosBoxLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum DosBoxLaunchExecutionError {
    Preflight(DosBoxLaunchPreflightError),
    Spawn(DosBoxLaunchSpawnError),
}

impl From<DosBoxLaunchPreflightError> for DosBoxLaunchExecutionError {
    fn from(value: DosBoxLaunchPreflightError) -> Self {
        Self::Preflight(value)
    }
}

impl From<DosBoxLaunchSpawnError> for DosBoxLaunchExecutionError {
    fn from(value: DosBoxLaunchSpawnError) -> Self {
        Self::Spawn(value)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Revalidates the game directory, the exact executable binding, and the
/// verified config, then rebuilds and checks the DOSBox command plan.
/// Returns the command that may be spawned, or fails closed.
///
/// No `PATH` search happens here; the executable is checked at exactly
/// [`DosBoxLaunchRequest::expected_executable`]. The config is re-inspected
/// only structurally (via the unchanged [`discover_dosbox_config`]), never
/// interpreted.
pub fn preflight_dosbox_launch(
    request: &DosBoxLaunchRequest,
) -> Result<DosBoxCommand, DosBoxLaunchPreflightError> {
    let game_directory = &request.game_directory;
    if !game_directory.is_absolute() {
        return Err(error(
            DosBoxLaunchPreflightErrorKind::GameDirectoryNotAbsolute,
            "the DOS game directory must be an absolute path",
        ));
    }
    let metadata = std::fs::symlink_metadata(game_directory).map_err(|io_error| {
        error(
            DosBoxLaunchPreflightErrorKind::GameDirectoryNotFound,
            format!("game directory is unavailable: {io_error}"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            DosBoxLaunchPreflightErrorKind::GameDirectoryIsSymlink,
            "the DOS game directory is a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(error(
            DosBoxLaunchPreflightErrorKind::GameDirectoryNotDirectory,
            "the DOS launch input is not a directory",
        ));
    }

    // Revalidate the EXACT executable the planner already chose - no new
    // PATH search, so discovery and execution cannot drift apart.
    let binding = resolve_dosbox_native_launch_binding_at(
        &request.expected_executable,
        request.expected_variant,
    )
    .map_err(|refusal| {
        error(
            DosBoxLaunchPreflightErrorKind::ExecutableUnavailable,
            refusal.detail(),
        )
    })?;

    // Re-discover the config structurally. This never reads the meaning of
    // any [autoexec] line - it only re-confirms the file still parses as a
    // DOSBox config with an [autoexec] section.
    let config_status = discover_dosbox_config(game_directory, &TrustedRoots::none());
    let DosBoxConfigStatus::Verified { config_path, .. } = &config_status else {
        return Err(error(
            DosBoxLaunchPreflightErrorKind::ConfigNotVerified,
            "no verified dosbox.conf ([autoexec] section) is present in the game directory",
        ));
    };
    if config_path != &request.expected_config_path {
        return Err(error(
            DosBoxLaunchPreflightErrorKind::ConfigPathDrift,
            "the verified dosbox.conf is not the one authorized at readiness time",
        ));
    }

    let plan = build_dosbox_command_plan(
        &request.identity,
        game_directory,
        &config_status,
        &Ok(binding),
    );
    if !plan.blockers.is_empty() {
        return Err(error(
            DosBoxLaunchPreflightErrorKind::CommandBlocked,
            format!(
                "DOSBox command plan reported {} blocker(s)",
                plan.blockers.len()
            ),
        ));
    }
    plan.command.ok_or_else(|| {
        error(
            DosBoxLaunchPreflightErrorKind::CommandMissing,
            "DOSBox command plan produced no command",
        )
    })
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

pub use crate::launch::process_spawn::ProcessExitReport as DosBoxLaunchExitReport;

/// A spawned, still-owned DOSBox process. Never automatically killed, timed
/// out, or relaunched - the caller owns it for as long as the user wants it
/// running. A running process is not proof the game booted.
pub struct LaunchedDosBoxProcess {
    pub pid: u32,
    pub command: DosBoxCommand,
    watched: WatchedProcess,
}

impl LaunchedDosBoxProcess {
    pub fn poll(&mut self) -> Option<&DosBoxLaunchExitReport> {
        self.watched.poll()
    }

    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}

/// Spawns exactly the DOSBox command - `<executable> -conf <config path>`
/// with the game directory as the working directory - via the shared
/// direct-argv watcher. No shell, no environment override, no timeout.
pub fn spawn_dosbox(
    command: DosBoxCommand,
) -> Result<LaunchedDosBoxProcess, DosBoxLaunchSpawnError> {
    let prepared = PreparedProcessCommand {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(DosBoxLaunchSpawnError::Spawn)?;
    Ok(LaunchedDosBoxProcess {
        pid: watched.pid,
        command,
        watched,
    })
}

/// Preflight then spawn, in one call.
pub fn preflight_and_launch_dosbox(
    request: &DosBoxLaunchRequest,
) -> Result<LaunchedDosBoxProcess, DosBoxLaunchExecutionError> {
    Ok(spawn_dosbox(preflight_dosbox_launch(request)?)?)
}

#[cfg(test)]
mod tests;
