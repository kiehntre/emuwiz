//! Evidence from a launched process: how it ended and what it wrote to
//! standard error.
//!
//! The launch executor already owns the process and reports
//! [`ProcessExitReport`](crate::launch::process_spawn::ProcessExitReport);
//! this module only turns that report into evidence. It starts nothing and
//! signals nothing.

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use super::evidence::{
    EvidenceArea, EvidenceId, EvidenceSet, ExitOutcome, Fact, LogStream, ProcessExit, SpawnFailure,
    Trust,
};
use super::signatures::{self, HOST_EMULATOR, HOST_SIGNATURES, Signature};
use super::text;
use crate::launch::process_spawn::ProcessExitReport;

/// The outcome for a process that could not be started.
pub fn spawn_failure(error: &std::io::Error) -> ExitOutcome {
    ExitOutcome::SpawnFailed {
        reason: match error.kind() {
            std::io::ErrorKind::NotFound => SpawnFailure::ProgramNotFound,
            std::io::ErrorKind::PermissionDenied => SpawnFailure::PermissionDenied,
            _ => SpawnFailure::Other,
        },
        os_error: error.raw_os_error(),
    }
}

/// The outcome for a process that was waited on.
pub fn exit_outcome(status: &std::io::Result<ExitStatus>) -> ExitOutcome {
    match status {
        Err(_) => ExitOutcome::StatusUnavailable,
        Ok(status) => match (status.signal(), status.code()) {
            (Some(signal), _) => ExitOutcome::Signaled {
                signal,
                core_dumped: status.core_dumped(),
            },
            (None, Some(code)) => ExitOutcome::Exited { code },
            (None, None) => ExitOutcome::StatusUnavailable,
        },
    }
}

/// What is known about a signal number on this platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalInfo {
    pub name: &'static str,
    pub kind: SignalKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalKind {
    /// The program itself faulted or aborted.
    Fault,
    /// Something asked the program to stop: the user, EmuWiz, a session
    /// ending. Not a crash.
    StopRequest,
    /// Forcibly ended from outside. The kernel's out-of-memory handling
    /// does this, and so does a user's forced stop; the signal alone does
    /// not say which.
    ForcedKill,
    Other,
}

pub fn signal_info(signal: i32) -> Option<SignalInfo> {
    let (name, kind) = match signal {
        libc::SIGSEGV => ("SIGSEGV", SignalKind::Fault),
        libc::SIGABRT => ("SIGABRT", SignalKind::Fault),
        libc::SIGBUS => ("SIGBUS", SignalKind::Fault),
        libc::SIGILL => ("SIGILL", SignalKind::Fault),
        libc::SIGFPE => ("SIGFPE", SignalKind::Fault),
        libc::SIGSYS => ("SIGSYS", SignalKind::Fault),
        libc::SIGTRAP => ("SIGTRAP", SignalKind::Fault),
        libc::SIGTERM => ("SIGTERM", SignalKind::StopRequest),
        libc::SIGINT => ("SIGINT", SignalKind::StopRequest),
        libc::SIGHUP => ("SIGHUP", SignalKind::StopRequest),
        libc::SIGQUIT => ("SIGQUIT", SignalKind::StopRequest),
        libc::SIGKILL => ("SIGKILL", SignalKind::ForcedKill),
        libc::SIGPIPE => ("SIGPIPE", SignalKind::Other),
        libc::SIGXCPU => ("SIGXCPU", SignalKind::Other),
        libc::SIGXFSZ => ("SIGXFSZ", SignalKind::Other),
        _ => return None,
    };
    Some(SignalInfo { name, kind })
}

/// The signal a shell-style launcher would be relaying with this exit
/// code. Shells and many wrappers report a child killed by signal `N` as
/// exit code `128 + N`, but a program can also simply exit with such a
/// code, so this is a possibility, never a fact.
pub fn possibly_relayed_signal(code: i32) -> Option<i32> {
    let signal = code.checked_sub(128)?;
    (1..=64).contains(&signal).then_some(signal)
}

/// Record how a process ended. `duration` is the time from start to exit
/// as measured by the caller; `launcher` names the wrapper when the watched
/// process is not the emulator itself.
pub fn record_exit(
    set: &mut EvidenceSet,
    outcome: ExitOutcome,
    duration: Option<Duration>,
    launcher: Option<&str>,
) -> Option<EvidenceId> {
    set.observe(
        EvidenceArea::ProcessExit,
        Trust::Measured,
        "exit status of the launched process, as reported by the operating system",
        Fact::ProcessExit(ProcessExit {
            outcome,
            duration_ms: duration.map(|duration| duration.as_millis().min(u64::MAX as u128) as u64),
            launcher: launcher.map(|name| text::sanitise(name, text::VALUE_LIMIT)),
        }),
    )
}

/// Record an executor's exit report: the exit itself, then standard error
/// scanned against the host signatures and the emulator's own
/// (`emulator_signatures`). Returns the id of the exit item.
pub fn record_exit_report(
    set: &mut EvidenceSet,
    report: &ProcessExitReport,
    duration: Option<Duration>,
    launcher: Option<&str>,
    emulator: &str,
    emulator_signatures: &[Signature],
) -> Option<EvidenceId> {
    let exit = record_exit(set, exit_outcome(&report.status), duration, launcher);
    record_stderr(set, &report.stderr, emulator, emulator_signatures);
    exit
}

/// Scan captured standard error. The executor keeps only the first part of
/// the stream, so the coverage is marked truncated when the capture is
/// full: an error printed later would not be in it.
pub fn record_stderr(
    set: &mut EvidenceSet,
    stderr: &[u8],
    emulator: &str,
    emulator_signatures: &[Signature],
) -> Vec<EvidenceId> {
    let examined = text::examine(stderr);
    let capture_full = stderr.len() >= crate::launch::process_spawn::PROCESS_STDERR_CAPTURE_LIMIT;
    let mut ids = Vec::new();
    for (name, table) in [
        (HOST_EMULATOR, HOST_SIGNATURES),
        (emulator, emulator_signatures),
    ] {
        if table.is_empty() {
            continue;
        }
        let mut scan = signatures::scan(&examined, table, name, LogStream::Stderr);
        scan.coverage.truncated |= capture_full;
        ids.extend(signatures::record_scan(
            set,
            scan,
            "standard error of the launched process",
        ));
    }
    ids
}

#[cfg(test)]
#[path = "process/tests.rs"]
mod tests;
