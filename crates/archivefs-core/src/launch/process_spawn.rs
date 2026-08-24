//! Generic, target-neutral process-lifecycle pieces shared by every real
//! launch executor in this crate (today: [`crate::launch::execution`] for
//! RetroArch, [`crate::launch::dolphin_execution`] for Dolphin).
//!
//! This module contains no emulator-specific policy at all: no argv
//! construction, no executable/profile selection, no identity or content
//! inspection. It only ever does three narrow, reusable things:
//!
//! - [`CapturedFileIdentity`] - a point-in-time `(device, inode, size,
//!   mtime)` snapshot a caller uses to detect a file swapped out from under
//!   a launch between preflight and spawn.
//! - [`spawn_watched_process`] - spawns exactly the argv a caller already
//!   built via [`std::process::Command::new`] + [`std::process::Command::args`]
//!   (never a shell), with stdin null, stdout null, stderr piped and
//!   drained on a background thread bounded at
//!   [`PROCESS_STDERR_CAPTURE_LIMIT`]. No environment override, no timeout,
//!   no automatic kill - the caller owns the returned [`WatchedProcess`] for
//!   as long as the user wants the launched program running.
//! - [`read_bounded_stderr`] - the bounded stderr drain
//!   [`spawn_watched_process`] itself uses, exposed separately so it stays
//!   independently testable.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::SystemTime;

/// Caps how much of a launched process's stderr this module ever retains in
/// memory - a diagnostic aid for a failed/crashed launch, never a full log.
/// Matches the existing `64 KiB` bound this crate already uses for
/// subprocess output elsewhere (`dat::archive::external_process`,
/// `run_command_os_with_timeout`).
pub const PROCESS_STDERR_CAPTURE_LIMIT: usize = 64 * 1024;

/// A launch-specific, point-in-time filesystem identity for a content file -
/// captured once during preflight and re-checked immediately before spawn,
/// so a swap of the file at the same path between those two moments is
/// detected rather than silently launched. Mirrors the `(device, inode,
/// size, mtime)` shape this crate already uses for the same purpose
/// elsewhere (`shared_transaction`'s `same_file`, `SharedDirectoryIdentity`;
/// `dolphin_local`'s `DolphinDirectoryIdentity`) - device/inode are the
/// *only* platform-specific part (`0` on non-Unix, where they carry no
/// comparable meaning), size and modification time are always real.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedFileIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl CapturedFileIdentity {
    pub fn capture(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        let (device, inode) = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let (device, inode) = (0u64, 0u64);
        Self {
            device,
            inode,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

/// `fs::symlink_metadata(path)` plus [`CapturedFileIdentity::capture`] in
/// one call, for callers that need both the identity and the raw metadata
/// (e.g. to also check symlink/regular-file state) without inspecting the
/// path twice.
pub fn capture_file_identity(path: &Path) -> std::io::Result<(fs::Metadata, CapturedFileIdentity)> {
    let metadata = fs::symlink_metadata(path)?;
    let identity = CapturedFileIdentity::capture(&metadata);
    Ok((metadata, identity))
}

/// The exact, already-verified argv a caller wants spawned - never
/// constructed here, only carried through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedProcessCommand {
    pub executable: PathBuf,
    pub arguments: Vec<std::ffi::OsString>,
    pub working_directory: Option<PathBuf>,
}

/// What the background watcher thread reports once the process has exited.
#[derive(Debug)]
pub struct ProcessExitReport {
    /// `Err` only when `wait()` on the child itself failed (not when the
    /// process exited non-zero - that is a normal, successful `Ok(status)`
    /// with `status.success() == false`).
    pub status: std::io::Result<ExitStatus>,
    /// Bounded (see [`PROCESS_STDERR_CAPTURE_LIMIT`]) capture of the
    /// process's stderr, for diagnosing a failed/crashed launch. Never a
    /// full, unbounded log.
    pub stderr: Vec<u8>,
}

/// A spawned, still-owned process. Never automatically killed, timed out, or
/// relaunched by this module - the caller owns it for as long as the user
/// wants it running. [`Self::poll`] is the narrow, non-blocking way to
/// notice it has exited, backed by a background thread that only ever
/// drains stderr and waits - it never sends a signal.
pub struct WatchedProcess {
    pub pid: u32,
    receiver: Receiver<ProcessExitReport>,
    exit_report: Option<ProcessExitReport>,
}

impl WatchedProcess {
    /// Non-blocking: returns the exit report once the background watcher
    /// thread has observed the process exit, `None` while it is still
    /// running. Safe to call every GUI frame.
    pub fn poll(&mut self) -> Option<&ProcessExitReport> {
        if self.exit_report.is_none() {
            match self.receiver.try_recv() {
                Ok(report) => self.exit_report = Some(report),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {}
            }
        }
        self.exit_report.as_ref()
    }

    pub fn is_running(&self) -> bool {
        self.exit_report.is_none()
    }
}

/// Spawns exactly the process `command` describes - never a shell, never one
/// concatenated command string.
///
/// - `Command::new(&command.executable)` + `.args(&command.arguments)` -
///   every argument (including any content path) is passed as its own
///   `OsString`, so spaces, quotes, and shell metacharacters in a path are
///   inert data, never re-parsed as syntax.
/// - `stdin` is `Stdio::null()`.
/// - `stdout` is `Stdio::null()` - the launched program is a graphical
///   process; it does not need its own stdout consumed by this process, and
///   inheriting it would tie this process's terminal to the child's for no
///   benefit.
/// - `stderr` is `Stdio::piped()` and drained on a background thread,
///   bounded at [`PROCESS_STDERR_CAPTURE_LIMIT`], purely as a diagnostic aid
///   if the launch fails or crashes.
/// - No environment variables are injected or overridden - the spawned
///   process inherits this process's environment exactly.
/// - The working directory is exactly `command.working_directory` (`None`
///   inherits this process's own working directory).
/// - No timeout, no automatic kill: see [`WatchedProcess`]'s own doc
///   comment.
pub fn spawn_watched_process(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    let mut process = Command::new(&command.executable);
    process.args(&command.arguments);
    process.stdin(Stdio::null());
    process.stdout(Stdio::null());
    process.stderr(Stdio::piped());
    if let Some(working_directory) = &command.working_directory {
        process.current_dir(working_directory);
    }
    let mut child: Child = process.spawn()?;
    let pid = child.id();
    let stderr = child.stderr.take();

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let stderr_bytes = stderr.map(read_bounded_stderr).unwrap_or_default();
        let status = child.wait();
        let _ = sender.send(ProcessExitReport {
            status,
            stderr: stderr_bytes,
        });
    });

    Ok(WatchedProcess {
        pid,
        receiver,
        exit_report: None,
    })
}

pub fn read_bounded_stderr(mut stderr: impl Read) -> Vec<u8> {
    let mut buffer = vec![0u8; PROCESS_STDERR_CAPTURE_LIMIT];
    let mut filled = 0usize;
    while filled < buffer.len() {
        match stderr.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => break,
        }
    }
    buffer.truncate(filled);
    // Drain and discard anything past the cap, so a chatty process still
    // exits cleanly (an unread, full pipe can otherwise block the child on
    // write) without this module ever retaining more than the bound.
    let mut discard = [0u8; 4096];
    while matches!(stderr.read(&mut discard), Ok(read) if read > 0) {}
    buffer
}
