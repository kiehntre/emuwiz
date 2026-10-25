//! Generic, target-neutral process-lifecycle pieces shared by every real
//! launch executor in this crate (today: [`crate::launch::execution`] for
//! RetroArch, [`crate::launch::dolphin_execution`] for Dolphin).
//!
//! This module contains no emulator-specific policy at all: no argv
//! construction, no executable/profile selection, no identity or content
//! inspection. It provides these narrow, reusable pieces:
//!
//! - [`CapturedFileIdentity`] - a point-in-time `(device, inode, size,
//!   mtime)` snapshot a caller uses to detect a file swapped out from under
//!   a launch between preflight and spawn.
//! - [`spawn_watched_process`] - spawns exactly the argv a caller already
//!   built via [`std::process::Command::new`] + [`std::process::Command::args`]
//!   (never a shell), with stdin null, stdout null, stderr piped and
//!   drained on a background thread bounded at
//!   [`PROCESS_STDERR_CAPTURE_LIMIT`]. The default entry point has no environment override, no timeout,
//!   no automatic kill - the caller owns the returned [`WatchedProcess`] for
//!   as long as the user wants the launched program running.
//! - [`read_bounded_stderr`] - the bounded stderr drain
//!   [`spawn_watched_process`] itself uses, exposed separately so it stays
//!   independently testable.
//! - Explicit, scoped [`WatchedProcess::request_graceful_stop`], using
//!   launch-captured ownership rather than a numeric PID. It never forces
//!   termination and refuses unsupervised whole-process-tree requests.
//!
//! Linux also has a crate-private, opt-in preservation-workspace spawn seam:
//! per-child environment, inherited workspace lease, and lifecycle callbacks.
//! Existing callers do not use it and retain their original behavior.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// Caps how much of a launched process's stderr this module ever retains in
/// memory - a diagnostic aid for a failed/crashed launch, never a full log.
/// Matches the existing `64 KiB` bound this crate already uses for
/// subprocess output elsewhere (`dat::archive::external_process`,
/// `run_command_os_with_timeout`).
pub const PROCESS_STDERR_CAPTURE_LIMIT: usize = 64 * 1024;

/// Opens `path` for reading without ever blocking or following a link, and
/// only if what was opened is a regular file.
///
/// A plain `File::open` on a FIFO blocks until a writer appears, which would
/// freeze whichever thread asked; on a link it silently reads the target.
/// `O_NONBLOCK` makes opening a FIFO return at once, `O_NOFOLLOW` refuses a
/// link as the final path component (a link in a parent directory is still
/// resolved), and the opened handle's own type is checked, so a path swapped
/// for a FIFO, device, directory or link between an earlier check and this
/// open is refused rather than waited on or followed. It does not help with a
/// regular file on a hung network or FUSE mount, which can still stall.
pub fn open_regular_file_nonblocking(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

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

/// The exact structured command description shared by read-only preview and
/// the executor. It is deliberately an alias of the executor's existing
/// argv-shaped value: preview never stores one, and spawning still requires
/// a freshly rebuilt value after preflight.
pub type LaunchCommandSpec = PreparedProcessCommand;

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
    /// Diagnostic only. Changing this value cannot redirect a stop request.
    pub pid: u32,
    receiver: Receiver<ProcessExitReport>,
    exit_report: Option<ProcessExitReport>,
    stop_identity: OwnedChildIdentity,
    stop_requested: bool,
}

/// The caller must explicitly choose the scope of a stop request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GracefulStopScope {
    /// Only the child captured at launch. No claim about descendants.
    LaunchedChild,
    /// Refused: this launcher does not own a supervised process tree.
    ProcessTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GracefulStopRefusal {
    /// Neither a shared process group nor a `/proc` ancestry snapshot is
    /// sufficient authority to signal descendants, including orphans.
    ProcessTreeNotSupervised,
    UnsupportedPlatform,
    OwnershipUnavailable {
        operation: &'static str,
        raw_os_error: Option<i32>,
    },
    ExitObservationFailed {
        raw_os_error: Option<i32>,
    },
    SignalFailed {
        raw_os_error: Option<i32>,
    },
    InvalidGracePeriod,
}

/// Describes the launched child only, never an entire emulator tree. Child
/// exit can precede the stderr drain, exit report and workspace callback;
/// callers must continue polling for those existing lifecycle operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GracefulStopOutcome {
    AlreadyExited,
    ExitedAfterRequest,
    /// The observation period ended without observing child exit. No
    /// second signal or escalation was sent. Subsequent calls only observe.
    StillRunning {
        request_already_pending: bool,
    },
    Refused(GracefulStopRefusal),
}

enum OwnedChildIdentity {
    #[cfg(target_os = "linux")]
    ProcessFd(std::os::fd::OwnedFd),
    Unavailable {
        operation: &'static str,
        raw_os_error: Option<i32>,
    },
    #[cfg(not(target_os = "linux"))]
    UnsupportedPlatform,
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

    /// Explicitly requests one SIGTERM to the owned launched child, then
    /// observes it for at most `grace_period`. Repeated requests never send
    /// a second SIGTERM (which can itself force some emulators to stop).
    /// No SIGKILL, timeout-triggered signal, drop-triggered signal, numeric
    /// PID lookup, process-group signal or descendant walk is performed.
    ///
    /// Linux ownership is a pidfd opened by the child itself before exec
    /// and transferred to this handle. It survives child exit and numeric
    /// PID reuse without ever referring to the replacement process. Other
    /// platforms, missing ownership and whole-tree requests fail closed.
    pub fn request_graceful_stop(
        &mut self,
        scope: GracefulStopScope,
        grace_period: Duration,
    ) -> GracefulStopOutcome {
        if scope == GracefulStopScope::ProcessTree {
            return GracefulStopOutcome::Refused(GracefulStopRefusal::ProcessTreeNotSupervised);
        }
        let Some(deadline) = Instant::now().checked_add(grace_period) else {
            return GracefulStopOutcome::Refused(GracefulStopRefusal::InvalidGracePeriod);
        };
        match &self.stop_identity {
            OwnedChildIdentity::Unavailable {
                operation,
                raw_os_error,
            } => GracefulStopOutcome::Refused(GracefulStopRefusal::OwnershipUnavailable {
                operation,
                raw_os_error: *raw_os_error,
            }),
            #[cfg(not(target_os = "linux"))]
            OwnedChildIdentity::UnsupportedPlatform => {
                let _ = deadline;
                GracefulStopOutcome::Refused(GracefulStopRefusal::UnsupportedPlatform)
            }
            #[cfg(target_os = "linux")]
            OwnedChildIdentity::ProcessFd(fd) => {
                use std::os::fd::AsRawFd;
                let fd = fd.as_raw_fd();
                match pidfd_has_exited(fd) {
                    Ok(true) => return GracefulStopOutcome::AlreadyExited,
                    Ok(false) => {}
                    Err(error) => {
                        return GracefulStopOutcome::Refused(
                            GracefulStopRefusal::ExitObservationFailed {
                                raw_os_error: error.raw_os_error(),
                            },
                        );
                    }
                }
                let request_already_pending = self.stop_requested;
                if !request_already_pending {
                    // SAFETY: fd is a live OwnedFd acquired from this exact
                    // child before exec. A null siginfo and zero flags target
                    // only that process; PID/group numbers are never used.
                    let result = unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            fd,
                            libc::SIGTERM,
                            std::ptr::null::<libc::siginfo_t>(),
                            0u32,
                        )
                    };
                    if result < 0 {
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() == Some(libc::ESRCH) {
                            return GracefulStopOutcome::AlreadyExited;
                        }
                        return GracefulStopOutcome::Refused(GracefulStopRefusal::SignalFailed {
                            raw_os_error: error.raw_os_error(),
                        });
                    }
                    self.stop_requested = true;
                }
                loop {
                    match pidfd_has_exited(fd) {
                        Ok(true) => return GracefulStopOutcome::ExitedAfterRequest,
                        Ok(false) => {}
                        Err(error) => {
                            return GracefulStopOutcome::Refused(
                                GracefulStopRefusal::ExitObservationFailed {
                                    raw_os_error: error.raw_os_error(),
                                },
                            );
                        }
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return GracefulStopOutcome::StillRunning {
                            request_already_pending,
                        };
                    }
                    thread::sleep(remaining.min(Duration::from_millis(10)));
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn pidfd_has_exited(fd: i32) -> std::io::Result<bool> {
    let mut observation = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one initialized pollfd, valid for this non-blocking call.
        if unsafe { libc::poll(&mut observation, 1, 0) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if observation.revents & (libc::POLLNVAL | libc::POLLERR) != 0 {
            return Err(std::io::Error::from_raw_os_error(libc::EBADF));
        }
        return Ok(observation.revents & (libc::POLLIN | libc::POLLHUP) != 0);
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
    spawn_watched_process_inner(command, &[], &[], None, None, false)
}

/// Like [`spawn_watched_process`] but the child starts with an EMPTY
/// environment (a sandbox launcher such as `bwrap` keeps its own environment
/// readable at `/proc/1/environ` inside the sandbox, so it must not inherit
/// this process's), and every descriptor above stderr that this
/// process holds is closed on exec, so a descriptor that was accidentally left
/// inheritable (a database, a log, a socket) can never reach the child. Used
/// for untrusted-content launches that run inside an OS sandbox. If the
/// kernel cannot do this the spawn fails; it never falls back.
pub(crate) fn spawn_watched_process_closing_fds(
    command: &PreparedProcessCommand,
) -> std::io::Result<WatchedProcess> {
    spawn_watched_process_inner(command, &[], &[], None, None, true)
}

/// Like [`spawn_watched_process`] but sets `environment` on the child only
/// (the parent environment is untouched). For launches that must redirect an
/// emulator's profile directory, such as a per-launch cheat runtime.
pub(crate) fn spawn_watched_process_with_environment(
    command: &PreparedProcessCommand,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
) -> std::io::Result<WatchedProcess> {
    spawn_watched_process_inner(command, environment, &[], None, None, false)
}

/// Opt-in preservation workspace seam. Legacy callers retain their exact
/// command/env/cwd behavior. The lifecycle callback is owned by the watcher,
/// not the GUI's selected game, so switching selection cannot delete a live
/// launch's media. The child inherits the workspace lease across exec.
#[cfg(target_os = "linux")]
pub(crate) fn spawn_watched_process_isolated(
    command: &PreparedProcessCommand,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
    lease: &std::fs::File,
    on_start: impl FnOnce(u32) + 'static,
    on_exit: impl FnOnce(&ProcessExitReport) + Send + 'static,
) -> std::io::Result<WatchedProcess> {
    use std::os::fd::AsRawFd;
    spawn_watched_process_inner(
        command,
        environment,
        &[lease.as_raw_fd()],
        Some(Box::new(on_start)),
        Some(Box::new(on_exit)),
        false,
    )
}

type ExitCallback = Box<dyn FnOnce(&ProcessExitReport) + Send>;

/// Opt-in descriptor-bound assets. The caller retains the files through spawn
/// and the callback retains their workspace until the watched child exits.
/// Descriptor inheritance changes only in the child; unrelated launches keep
/// their existing environment, stdio and descriptor behavior.
#[cfg(target_os = "linux")]
pub(crate) fn spawn_watched_process_with_assets(
    command: &PreparedProcessCommand,
    files: &[&std::fs::File],
    on_exit: impl FnOnce(&ProcessExitReport) + Send + 'static,
) -> std::io::Result<WatchedProcess> {
    use std::os::fd::AsRawFd;
    let fds: Vec<_> = files.iter().map(|file| file.as_raw_fd()).collect();
    if fds.iter().any(|fd| *fd < 3) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "launch assets must be above stdin/stdout/stderr",
        ));
    }
    spawn_watched_process_inner(command, &[], &fds, None, Some(Box::new(on_exit)), false)
}

fn spawn_watched_process_inner(
    command: &PreparedProcessCommand,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
    inherited_files: &[i32],
    on_start: Option<Box<dyn FnOnce(u32)>>,
    on_exit: Option<ExitCallback>,
    close_inherited_fds: bool,
) -> std::io::Result<WatchedProcess> {
    // Each test-only ETXTBSY retry needs a fresh command and ownership
    // channel, so a failed exec's descriptor can never name a later child.
    let (mut child, stop_identity) = spawn_child(|| {
        let mut process = Command::new(&command.executable);
        process.args(&command.arguments);
        if close_inherited_fds {
            process.env_clear();
        }
        process.envs(environment.iter().map(|(key, value)| (key, value)));
        process.stdin(Stdio::null());
        process.stdout(Stdio::null());
        process.stderr(Stdio::piped());
        if let Some(working_directory) = &command.working_directory {
            process.current_dir(working_directory);
        }
        #[cfg(target_os = "linux")]
        if !inherited_files.is_empty() {
            use std::os::unix::process::CommandExt;
            let fds = inherited_files.to_vec();
            // SAFETY: only async-signal-safe fcntl is called after fork. The
            // caller keeps this fd alive through spawn; CLOEXEC changes only in
            // the child, never in another thread/the parent environment.
            unsafe {
                process.pre_exec(move || {
                    for fd in &fds {
                        if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = inherited_files;
        #[cfg(target_os = "linux")]
        if close_inherited_fds {
            use std::os::unix::process::CommandExt;
            // SAFETY: close_range is one async-signal-safe syscall. It only
            // marks descriptors 3.. close-on-exec in the child after fork.
            // It runs before the ownership hook registered below, whose own
            // socket is sent to the parent before exec and needs no
            // inheritance.
            unsafe {
                process.pre_exec(|| {
                    const CLOSE_RANGE_CLOEXEC: libc::c_uint = 4;
                    if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, CLOSE_RANGE_CLOEXEC)
                        != 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        spawn_with_owned_identity(&mut process)
    })?;
    let pid = child.id();
    if let Some(on_start) = on_start {
        on_start(pid);
    }
    let stderr = child.stderr.take();

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let stderr_bytes = stderr.map(read_bounded_stderr).unwrap_or_default();
        let status = child.wait();
        let report = ProcessExitReport {
            status,
            stderr: stderr_bytes,
        };
        if let Some(on_exit) = on_exit {
            on_exit(&report);
        }
        let _ = sender.send(report);
    });

    Ok(WatchedProcess {
        pid,
        receiver,
        exit_report: None,
        stop_identity,
        stop_requested: false,
    })
}

#[cfg(not(target_os = "linux"))]
fn spawn_with_owned_identity(
    command: &mut Command,
) -> std::io::Result<(Child, OwnedChildIdentity)> {
    Ok((command.spawn()?, OwnedChildIdentity::UnsupportedPlatform))
}

/// Capture ownership before exec, without looking up a child by PID in the
/// parent. In particular, this remains safe if SIGCHLD is ignored or another
/// library reaps a child prematurely. SCM_RIGHTS transfers the child's own
/// pidfd through a private, CLOEXEC socket pair; no emulator inherits it.
/// Capture failure disables stopping, not an otherwise valid launch.
#[cfg(target_os = "linux")]
fn spawn_with_owned_identity(
    command: &mut Command,
) -> std::io::Result<(Child, OwnedChildIdentity)> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::net::UnixDatagram;
    use std::os::unix::process::CommandExt;

    // Command installs stdin/stdout/stderr before pre_exec. If the parent's
    // stdio is closed, socketpair can otherwise allocate 0, 1 or 2 and those
    // sockets would be replaced by that setup. Keep both strictly above 2.
    let above_stdio = |socket: OwnedFd| -> std::io::Result<OwnedFd> {
        if socket.as_raw_fd() >= 3 {
            return Ok(socket);
        }
        // SAFETY: socket remains owned through the duplication. The new fd
        // is uniquely owned and CLOEXEC, like the original private socket.
        let fd = unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    };
    let channels = UnixDatagram::pair()
        .and_then(|(parent, child)| Ok((above_stdio(parent.into())?, above_stdio(child.into())?)));
    let (parent, child_channel) = match channels {
        Ok(pair) => pair,
        Err(error) => {
            return Ok((
                command.spawn()?,
                OwnedChildIdentity::Unavailable {
                    operation: "ownership socket pair above stdio",
                    raw_os_error: error.raw_os_error(),
                },
            ));
        }
    };
    let parent_fd = parent.as_raw_fd();
    let child_fd = child_channel.as_raw_fd();
    // SAFETY: all post-fork operations below are direct syscalls, use only
    // initialized stack storage, and allocate or lock nothing. The socket
    // pair stays alive in the parent through spawn. No parent fd flags or
    // signal dispositions are changed. Existing lease pre_exec runs first.
    unsafe {
        command.pre_exec(move || {
            // pre_exec selects std's fork path. When the parent's stdio is
            // closed, a null/pipe source can already be fd 0, 1 or 2: dup2
            // of a descriptor onto itself does not clear CLOEXEC. Preserve
            // the explicitly configured stdio through exec in that case.
            for stdio in 0..=2 {
                let flags = libc::fcntl(stdio, libc::F_GETFD);
                if flags < 0 || libc::fcntl(stdio, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            libc::close(parent_fd);
            let pidfd = libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0u32) as i32;
            let mut error = if pidfd < 0 {
                *libc::__errno_location()
            } else {
                0
            };
            let mut data = libc::iovec {
                iov_base: std::ptr::addr_of_mut!(error).cast(),
                iov_len: std::mem::size_of::<i32>(),
            };
            // usize storage supplies cmsghdr alignment on 32/64-bit Linux.
            let mut control = [0usize; 4];
            let mut message: libc::msghdr = std::mem::zeroed();
            message.msg_iov = &mut data;
            message.msg_iovlen = 1;
            if pidfd >= 0 {
                message.msg_control = control.as_mut_ptr().cast();
                message.msg_controllen =
                    libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize;
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as usize;
                libc::CMSG_DATA(header).cast::<i32>().write(pidfd);
            }
            while libc::sendmsg(child_fd, &message, libc::MSG_NOSIGNAL) < 0 {
                if *libc::__errno_location() != libc::EINTR {
                    break;
                }
            }
            if pidfd >= 0 {
                libc::close(pidfd);
            }
            libc::close(child_fd);
            Ok(())
        });
    }
    let child = command.spawn()?;
    drop(child_channel);
    let mut error = 0i32;
    let mut data = libc::iovec {
        iov_base: std::ptr::addr_of_mut!(error).cast(),
        iov_len: std::mem::size_of::<i32>(),
    };
    let mut control = [0usize; 4];
    // SAFETY: msghdr's zero representation is valid; pointers below refer
    // to live, correctly aligned stack buffers throughout recvmsg/parsing.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut data;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = std::mem::size_of_val(&control);
    let received = unsafe {
        libc::recvmsg(
            parent.as_raw_fd(),
            &mut message,
            libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC,
        )
    };
    let identity = if received < 0 {
        OwnedChildIdentity::Unavailable {
            operation: "ownership descriptor transfer",
            raw_os_error: std::io::Error::last_os_error().raw_os_error(),
        }
    } else {
        // Only our pre_exec sends to this socket, and sends exactly one fd.
        // Own any received fd immediately, including malformed/truncated
        // messages, so every refusal closes it instead of leaking it.
        let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
        let descriptor = if !header.is_null()
            && unsafe {
                (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                    && (*header).cmsg_len
                        == libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as usize
            } {
            Some(unsafe { OwnedFd::from_raw_fd(libc::CMSG_DATA(header).cast::<i32>().read()) })
        } else {
            None
        };
        if received != std::mem::size_of::<i32>() as isize
            || message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0
        {
            OwnedChildIdentity::Unavailable {
                operation: "ownership transfer protocol",
                raw_os_error: Some(libc::EPROTO),
            }
        } else if error != 0 {
            OwnedChildIdentity::Unavailable {
                operation: "child pidfd_open",
                raw_os_error: Some(error),
            }
        } else if let Some(descriptor) = descriptor {
            // recvmsg can also allocate 0/1/2 when parent stdio is closed.
            // Keep the ownership capability private even if the caller
            // subsequently reopens or restores its standard descriptors.
            match above_stdio(descriptor) {
                Ok(descriptor) => OwnedChildIdentity::ProcessFd(descriptor),
                Err(error) => OwnedChildIdentity::Unavailable {
                    operation: "ownership descriptor above stdio",
                    raw_os_error: error.raw_os_error(),
                },
            }
        } else {
            OwnedChildIdentity::Unavailable {
                operation: "missing ownership descriptor",
                raw_os_error: Some(libc::EPROTO),
            }
        }
    };
    Ok((child, identity))
}

/// Production spawn: exactly one `Command::spawn`, no retry, no other
/// behaviour. This is the only spawn path any shipped binary ever compiles.
#[cfg(not(test))]
#[inline]
fn spawn_child(
    mut spawn: impl FnMut() -> std::io::Result<(Child, OwnedChildIdentity)>,
) -> std::io::Result<(Child, OwnedChildIdentity)> {
    spawn()
}

/// Test-build-only spawn: identical to the production path except it retries
/// a handful of times, with a short back-off, on Linux `ETXTBSY` ("Text
/// file busy").
///
/// This exists purely for this crate's own `launch::*_execution` unit
/// tests. Each of those writes a tiny fake executable script into a
/// temporary fixture directory and then immediately spawns it; under the
/// full `cargo test` suite at high parallelism, another test thread's
/// `fork`/`posix_spawn` can transiently hold a dup'd write descriptor to a
/// *sibling* freshly-written script at the instant this thread `execve`s
/// its own, which the kernel reports as `ETXTBSY`. It is always transient -
/// the descriptor is `O_CLOEXEC` and closes as soon as that other child
/// finishes its own `execve` - so a bounded retry clears it. `#[cfg(test)]`
/// keeps this entirely out of every non-test build: production launch
/// behaviour is byte-for-byte the `#[cfg(not(test))]` function above.
#[cfg(test)]
fn spawn_child(
    mut spawn: impl FnMut() -> std::io::Result<(Child, OwnedChildIdentity)>,
) -> std::io::Result<(Child, OwnedChildIdentity)> {
    const MAX_ATTEMPTS: u32 = 10;
    let mut attempt = 0;
    loop {
        match spawn() {
            Err(error)
                if attempt + 1 < MAX_ATTEMPTS && error.raw_os_error() == Some(libc::ETXTBSY) =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(5 * u64::from(attempt)));
            }
            result => return result,
        }
    }
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

#[cfg(test)]
mod open_regular_tests {
    use super::*;
    use std::io::Read as _;
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;
    use std::time::Duration;

    fn mkfifo(path: &Path) {
        let status = Command::new("mkfifo").arg(path).status().unwrap();
        assert!(status.success(), "mkfifo is needed for this fixture");
    }

    /// Runs `work` on a thread and fails (instead of hanging the suite) if it
    /// does not finish promptly.
    fn within<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(work());
        });
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the open blocked")
    }

    #[test]
    fn a_plain_open_of_a_fifo_blocks_which_is_why_the_helper_exists() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        mkfifo(&fifo);
        let (sender, receiver) = mpsc::channel();
        let path = fifo.clone();
        let blocked = thread::spawn(move || {
            let _ = fs::File::open(&path);
            let _ = sender.send(());
        });
        assert!(
            receiver.recv_timeout(Duration::from_millis(400)).is_err(),
            "File::open on a FIFO with no writer should be waiting"
        );
        // Release it: opening the write end completes the blocked read open.
        let _writer = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("released");
        blocked.join().unwrap();
    }

    #[test]
    fn a_fifo_is_refused_at_once_and_never_waited_on() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        mkfifo(&fifo);
        let error = within(move || open_regular_file_nonblocking(&fifo).unwrap_err());
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn links_directories_and_devices_are_refused_and_nothing_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        fs::write(&real, b"payload").unwrap();
        let link = dir.path().join("link.bin");
        symlink(&real, &link).unwrap();
        assert!(open_regular_file_nonblocking(&link).is_err());
        assert!(open_regular_file_nonblocking(dir.path()).is_err());
        assert!(open_regular_file_nonblocking(Path::new("/dev/null")).is_err());
        assert!(open_regular_file_nonblocking(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn a_regular_file_opens_and_reads_normally() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        fs::write(&real, b"payload").unwrap();
        let mut text = String::new();
        open_regular_file_nonblocking(&real)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "payload");
    }
}
