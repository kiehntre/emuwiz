//! A short in-memory log of the processes this EmuWiz started and how each
//! ended. Every emulator route goes through [`super::process_spawn`], so one
//! log covers them all without each route having to report anything.
//!
//! It exists so a caller that only knows "a launch is in progress" (Blue's
//! launch activity) can learn the real process id, its start time and its
//! exit status, exactly as the watcher thread saw them. Bounded, process-local,
//! never written to disk.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Mutex;

const CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitSummary {
    Code(i32),
    Signal {
        signal: i32,
        core_dumped: bool,
    },
    /// `wait()` failed or returned neither a code nor a signal.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedProcess {
    /// Increases by one for every process started; never reused.
    pub sequence: u64,
    pub pid: u32,
    /// Kernel start time in clock ticks, read while the child is still held.
    pub start_ticks: Option<u64>,
    pub executable: PathBuf,
    /// The first few arguments, for telling a launcher from an emulator.
    pub arguments: Vec<String>,
    pub exit: Option<ExitSummary>,
    /// What the [`set_before_start`] gate answered for this very start,
    /// just before the process was created.
    pub gate_token: Option<u64>,
}

/// Runs just before a process is created and returns only when it is
/// content for the process to exist. Its answer is kept with the started
/// process ([`StartedProcess::gate_token`]).
pub type BeforeStart = dyn Fn(&std::path::Path, &[String]) -> Option<u64> + Send + Sync;

static BEFORE_START: Mutex<Option<std::sync::Arc<BeforeStart>>> = Mutex::new(None);

/// Sets the gate every start passes through first. The program that sets it
/// (Blue) uses it to read a game's saves **before** the emulator exists, so
/// the emulator cannot have touched them yet. One gate per program; setting
/// it again replaces it.
pub fn set_before_start(gate: std::sync::Arc<BeforeStart>) {
    if let Ok(mut slot) = BEFORE_START.lock() {
        *slot = Some(gate);
    }
}

/// Called by the spawn primitive before it creates a process. Blocks for as
/// long as the gate does. `None` when no gate is set or it has no answer.
pub(super) fn before_start(
    executable: &std::path::Path,
    arguments: &[std::ffi::OsString],
) -> Option<u64> {
    // The lock is not held while the gate runs.
    let gate = BEFORE_START.lock().ok()?.clone()?;
    let arguments: Vec<String> = arguments
        .iter()
        .take(6)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    gate(executable, &arguments)
}

struct Log {
    next: u64,
    entries: VecDeque<StartedProcess>,
}

static LOG: Mutex<Log> = Mutex::new(Log {
    next: 1,
    entries: VecDeque::new(),
});

fn start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// Called by the spawn primitive right after a child exists.
pub(super) fn record_start(
    pid: u32,
    executable: &std::path::Path,
    arguments: &[std::ffi::OsString],
    gate_token: Option<u64>,
) -> u64 {
    let Ok(mut log) = LOG.lock() else { return 0 };
    let sequence = log.next;
    log.next += 1;
    if log.entries.len() == CAPACITY {
        log.entries.pop_front();
    }
    log.entries.push_back(StartedProcess {
        sequence,
        pid,
        start_ticks: start_ticks(pid),
        executable: executable.to_path_buf(),
        arguments: arguments
            .iter()
            .take(6)
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        exit: None,
        gate_token,
    });
    sequence
}

/// Called by the watcher thread once the child has been waited for.
pub(super) fn record_exit(sequence: u64, status: &std::io::Result<ExitStatus>) {
    use std::os::unix::process::ExitStatusExt;
    let summary = match status {
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => ExitSummary::Code(code),
            (None, Some(signal)) => ExitSummary::Signal {
                signal,
                core_dumped: status.core_dumped(),
            },
            _ => ExitSummary::Unavailable,
        },
        Err(_) => ExitSummary::Unavailable,
    };
    if let Ok(mut log) = LOG.lock() {
        if let Some(entry) = log
            .entries
            .iter_mut()
            .find(|entry| entry.sequence == sequence)
        {
            entry.exit = Some(summary);
        }
    }
}

/// The sequence number the next started process will get. Take this before
/// a launch; everything [`started_since`] returns for it belongs after.
pub fn next_sequence() -> u64 {
    LOG.lock().map(|log| log.next).unwrap_or(0)
}

/// Processes started at or after `sequence`, oldest first, with their exit
/// if they have ended.
pub fn started_since(sequence: u64) -> Vec<StartedProcess> {
    LOG.lock()
        .map(|log| {
            log.entries
                .iter()
                .filter(|entry| entry.sequence >= sequence)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_started_process_is_listed_with_its_exit_once_it_has_one() {
        let mark = next_sequence();
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        let sequence = record_start(
            child.id(),
            std::path::Path::new("/bin/sh"),
            &["-c".into(), "exit 3".into()],
            None,
        );
        let seen = started_since(mark);
        let entry = seen
            .iter()
            .find(|entry| entry.sequence == sequence)
            .unwrap();
        assert_eq!(entry.pid, child.id());
        assert!(entry.start_ticks.is_some());
        assert_eq!(entry.exit, None);
        record_exit(sequence, &child.wait());
        let seen = started_since(mark);
        let entry = seen
            .iter()
            .find(|entry| entry.sequence == sequence)
            .unwrap();
        assert_eq!(entry.exit, Some(ExitSummary::Code(3)));
        assert!(started_since(next_sequence()).is_empty());
    }

    #[test]
    fn the_gate_runs_before_the_process_exists_and_its_answer_stays_with_the_process() {
        use super::super::process_spawn::{PreparedProcessCommand, spawn_watched_process};
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("the-gate-was-here");
        // Other tests start processes too; the gate answers only for ours.
        let ours = format!("gate-order-{}", std::process::id());
        let mark = next_sequence();
        {
            let (marker, ours) = (marker.clone(), ours.clone());
            set_before_start(std::sync::Arc::new(move |_, arguments| {
                if !arguments.iter().any(|argument| argument.contains(&ours)) {
                    return None;
                }
                // Nothing of this start exists yet.
                assert!(started_since(mark).iter().all(|process| {
                    !process
                        .arguments
                        .iter()
                        .any(|argument| argument.contains(&ours))
                }));
                // The start waits for the gate, however long it takes.
                std::thread::sleep(std::time::Duration::from_millis(300));
                std::fs::write(&marker, b"done").unwrap();
                Some(4242)
            }));
        }
        // The child succeeds only if the gate had finished before it ran.
        let command = PreparedProcessCommand {
            executable: "/bin/sh".into(),
            arguments: vec![
                "-c".into(),
                format!("test -e '{}' # {ours}", marker.display()).into(),
            ],
            working_directory: None,
        };
        let _process = spawn_watched_process(&command).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let entry = loop {
            let entry = started_since(mark).into_iter().find(|process| {
                process
                    .arguments
                    .iter()
                    .any(|argument| argument.contains(&ours))
            });
            if let Some(entry) = entry.filter(|entry| entry.exit.is_some()) {
                break entry;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the child never ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(entry.gate_token, Some(4242));
        assert_eq!(entry.exit, Some(ExitSummary::Code(0)));
    }
}
