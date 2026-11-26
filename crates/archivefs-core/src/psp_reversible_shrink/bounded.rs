//! Runs an external tool (maxcso) with a monotonic deadline.
//!
//! Both output pipes are drained on their own threads, so a tool that writes
//! more than a pipe buffer can never deadlock against us; only the last
//! [`TAIL_LIMIT`] bytes of each stream are kept. When the deadline passes, the
//! child started here is stopped through its own [`std::process::Child`]
//! handle (never by name or pattern) and then reaped. A process stuck in
//! uninterruptible kernel I/O cannot be stopped by any signal; that case is
//! reported as "not reaped" rather than claimed as terminated.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// At most this many bytes of each output stream are kept for diagnostics.
pub(super) const TAIL_LIMIT: usize = 4096;
/// How long `maxcso --version` may take.
pub(super) const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const DECOMPRESS_BASE_SECS: u64 = 120;
const DECOMPRESS_BYTES_PER_EXTRA_SEC: u64 = 16 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(20);
/// After SIGKILL, how long to wait for the kernel to let the child go.
const REAP_GRACE: Duration = Duration::from_secs(5);
/// After a normal exit, how long to wait for the drain threads (a
/// grandchild could keep a pipe open; it is not waited for indefinitely).
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// `maxcso --decompress` deadline: 120 s plus 1 s per 16 MiB of ISO.
/// Overflow-free for every `u64` size.
pub(super) fn decompress_timeout(iso_size: u64) -> Duration {
    Duration::from_secs(
        DECOMPRESS_BASE_SECS.saturating_add(iso_size / DECOMPRESS_BYTES_PER_EXTRA_SEC),
    )
}

#[derive(Debug)]
pub(super) struct Finished {
    pub status: ExitStatus,
    pub stdout_tail: Vec<u8>,
    pub stderr_tail: Vec<u8>,
}

#[derive(Debug)]
pub(super) enum BoundedError {
    /// The tool could not be started (or the deadline is not representable).
    Spawn(io::Error),
    /// Waiting for the tool failed; it was stopped through its handle.
    Wait(io::Error),
    /// The deadline passed. SIGKILL was sent through the child's own handle
    /// (`stop_error` if even that failed); `reaped` says whether it then
    /// exited within the grace period.
    TimedOut {
        after: Duration,
        stop_error: Option<String>,
        reaped: bool,
        pid: u32,
        stderr_tail: Vec<u8>,
    },
}

type Tail = Arc<Mutex<VecDeque<u8>>>;

fn drain(mut reader: impl Read + Send + 'static, tail: Tail) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let mut kept = tail.lock().unwrap_or_else(|poison| poison.into_inner());
                    kept.extend(&buffer[..read]);
                    let excess = kept.len().saturating_sub(TAIL_LIMIT);
                    kept.drain(..excess);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}

fn snapshot(tail: &Tail) -> Vec<u8> {
    tail.lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .copied()
        .collect()
}

fn wait_for_drains(threads: &[JoinHandle<()>], grace: Duration) {
    let until = Instant::now() + grace;
    while threads.iter().any(|thread| !thread.is_finished()) && Instant::now() < until {
        std::thread::sleep(POLL);
    }
}

/// Runs `command` (stdin closed, stdout and stderr captured) until it exits
/// or `timeout` passes on the monotonic clock.
pub(super) fn run_bounded(
    mut command: Command,
    timeout: Duration,
) -> Result<Finished, BoundedError> {
    let started = Instant::now();
    let deadline = started.checked_add(timeout).ok_or_else(|| {
        BoundedError::Spawn(io::Error::other("the timeout is too large to represent"))
    })?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(BoundedError::Spawn)?;
    let pid = child.id();
    let stdout_tail: Tail = Arc::default();
    let stderr_tail: Tail = Arc::default();
    let mut threads = Vec::new();
    if let Some(out) = child.stdout.take() {
        threads.push(drain(out, Arc::clone(&stdout_tail)));
    }
    if let Some(err) = child.stderr.take() {
        threads.push(drain(err, Arc::clone(&stderr_tail)));
    }
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                wait_for_drains(&threads, DRAIN_GRACE);
                return Ok(Finished {
                    status,
                    stdout_tail: snapshot(&stdout_tail),
                    stderr_tail: snapshot(&stderr_tail),
                });
            }
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    // Our own child, through its own handle: never by name.
                    let stop_error = child.kill().err().map(|error| error.to_string());
                    let reap_until = Instant::now() + REAP_GRACE;
                    let reaped = loop {
                        match child.try_wait() {
                            Ok(Some(_)) => break true,
                            Ok(None) if Instant::now() < reap_until => std::thread::sleep(POLL),
                            _ => break false,
                        }
                    };
                    if reaped {
                        wait_for_drains(&threads, DRAIN_GRACE);
                    }
                    return Err(BoundedError::TimedOut {
                        after: timeout,
                        stop_error,
                        reaped,
                        pid,
                        stderr_tail: snapshot(&stderr_tail),
                    });
                }
                std::thread::sleep(POLL.min(deadline - now));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.try_wait();
                return Err(BoundedError::Wait(error));
            }
        }
    }
}

/// The kept bytes as one trimmed line of text for a report.
pub(super) fn tail_text(tail: &[u8]) -> String {
    let text = String::from_utf8_lossy(tail);
    let text = text.trim();
    if text.is_empty() {
        "no error output".to_string()
    } else {
        text.replace('\n', " | ")
    }
}

/// What happened when a timed-out tool was stopped, without claiming more
/// than is known: a process in uninterruptible kernel I/O ignores SIGKILL.
pub(super) fn stop_summary(stop_error: Option<&str>, reaped: bool) -> String {
    match (stop_error, reaped) {
        (None, true) => "; EmuWiz sent it SIGKILL and it exited".to_string(),
        (None, false) => format!(
            "; EmuWiz sent it SIGKILL but it had not exited {} s later (it may be stuck in kernel I/O and may still be running)",
            REAP_GRACE.as_secs()
        ),
        (Some(error), true) => format!("; sending it SIGKILL failed ({error}), but it has exited"),
        (Some(error), false) => {
            format!("; sending it SIGKILL failed ({error}) and it may still be running")
        }
    }
}
