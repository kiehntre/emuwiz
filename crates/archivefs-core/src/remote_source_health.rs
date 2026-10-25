//! Bounded, read-only availability probes for local and remote-backed paths.
//!
//! A successful `stat` is deliberately not treated as proof that the backing
//! bytes are available.  This is important for archive mounts whose directory
//! metadata can outlive the upstream object.  The probe reads only a small,
//! caller-bounded prefix and never hashes or materialises the source.

use serde::Serialize;
use std::fs;
use std::io;
#[cfg(not(unix))]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_PROBE_BUDGET: Duration = Duration::from_secs(5);
pub const DEFAULT_PROBE_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RemoteSourceAvailability {
    Available,
    AvailableSlow,
    Unavailable,
    StaleMetadata,
    Timeout,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RemoteSourceErrorCategory {
    None,
    NotFound,
    PermissionDenied,
    ReadFailed,
    TimedOut,
    InvalidPath,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSourceProbeRequest {
    pub path: PathBuf,
    pub budget: Duration,
    pub max_bytes: usize,
    pub provenance: String,
}

impl RemoteSourceProbeRequest {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            budget: DEFAULT_PROBE_BUDGET,
            max_bytes: DEFAULT_PROBE_BYTES,
            provenance: "local filesystem path".into(),
        }
    }

    pub fn with_provenance(mut self, provenance: impl Into<String>) -> Self {
        self.provenance = provenance.into();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteSourceProbeResult {
    pub path: PathBuf,
    pub availability: RemoteSourceAvailability,
    pub latency_ms: u128,
    pub bytes_probed: u64,
    pub stat_succeeded: bool,
    pub read_succeeded: bool,
    pub error_category: RemoteSourceErrorCategory,
    pub error: Option<String>,
    pub timestamp_unix_seconds: u64,
    pub provenance: String,
}

impl RemoteSourceProbeResult {
    fn base(request: &RemoteSourceProbeRequest, started: Instant) -> Self {
        Self {
            path: request.path.clone(),
            availability: RemoteSourceAvailability::Unknown,
            latency_ms: started.elapsed().as_millis(),
            bytes_probed: 0,
            stat_succeeded: false,
            read_succeeded: false,
            error_category: RemoteSourceErrorCategory::None,
            error: None,
            timestamp_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            provenance: request.provenance.clone(),
        }
    }
}

/// Probe a path without mutating it or transmitting content hashes.
pub fn probe_remote_source(request: &RemoteSourceProbeRequest) -> RemoteSourceProbeResult {
    let started = Instant::now();
    let mut result = RemoteSourceProbeResult::base(request, started);
    if request.max_bytes == 0 {
        result.availability = RemoteSourceAvailability::Unknown;
        result.error_category = RemoteSourceErrorCategory::InvalidPath;
        result.error = Some("The probe byte limit must be greater than zero.".into());
        result.latency_ms = started.elapsed().as_millis();
        return result;
    }

    let metadata = match fs::metadata(&request.path) {
        Ok(metadata) => metadata,
        Err(error) => {
            result.availability = match error.kind() {
                io::ErrorKind::NotFound => RemoteSourceAvailability::Unavailable,
                io::ErrorKind::PermissionDenied => RemoteSourceAvailability::Unavailable,
                _ => RemoteSourceAvailability::Unknown,
            };
            result.error_category = error_category(&error);
            result.error = Some(error.to_string());
            result.latency_ms = started.elapsed().as_millis();
            return result;
        }
    };
    result.stat_succeeded = true;

    // Directories and empty files have no useful payload prefix to read. Their
    // metadata is still a valid, non-error local observation.
    if !metadata.is_file() || metadata.len() == 0 {
        result.availability = RemoteSourceAvailability::Available;
        result.read_succeeded = true;
        result.latency_ms = started.elapsed().as_millis();
        return result;
    }

    let read = bounded_prefix_read(&request.path, request.max_bytes, request.budget);
    result.bytes_probed = read.bytes;
    result.read_succeeded = read.error.is_none();
    result.error_category = read
        .error
        .as_ref()
        .map_or(RemoteSourceErrorCategory::None, error_category);
    result.error = read.error.as_ref().map(ToString::to_string);
    result.availability = match read.status {
        ReadStatus::Success if result.bytes_probed > 0 => {
            if started.elapsed() > Duration::from_millis(1500) {
                RemoteSourceAvailability::AvailableSlow
            } else {
                RemoteSourceAvailability::Available
            }
        }
        ReadStatus::Success => RemoteSourceAvailability::StaleMetadata,
        ReadStatus::Timeout => RemoteSourceAvailability::Timeout,
        ReadStatus::Failure => match read.error.as_ref().map(io::Error::kind) {
            Some(io::ErrorKind::PermissionDenied) => RemoteSourceAvailability::Unavailable,
            Some(io::ErrorKind::NotFound) => RemoteSourceAvailability::StaleMetadata,
            _ if result.stat_succeeded => RemoteSourceAvailability::StaleMetadata,
            _ => RemoteSourceAvailability::Unavailable,
        },
    };
    result.latency_ms = started.elapsed().as_millis();
    result
}

#[derive(Debug)]
struct ReadOutcome {
    status: ReadStatus,
    bytes: u64,
    error: Option<io::Error>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadStatus {
    Success,
    Failure,
    Timeout,
}

fn error_category(error: &io::Error) -> RemoteSourceErrorCategory {
    match error.kind() {
        io::ErrorKind::NotFound => RemoteSourceErrorCategory::NotFound,
        io::ErrorKind::PermissionDenied => RemoteSourceErrorCategory::PermissionDenied,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidFilename => {
            RemoteSourceErrorCategory::InvalidPath
        }
        io::ErrorKind::TimedOut => RemoteSourceErrorCategory::TimedOut,
        _ => RemoteSourceErrorCategory::ReadFailed,
    }
}

#[cfg(unix)]
fn bounded_prefix_read(path: &Path, max_bytes: usize, budget: Duration) -> ReadOutcome {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return ReadOutcome {
            status: ReadStatus::Failure,
            bytes: 0,
            error: Some(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL",
            )),
        };
    };
    // Allocate before fork: the child must not invoke an allocator while the
    // parent GUI may have other threads holding allocator locks.
    let mut buffer = vec![0_u8; max_bytes];
    let mut pipe_fds = [0; 2];
    // SAFETY: pipe/fork/poll/waitpid are used with fixed-size, private data;
    // the child performs only async-signal-safe libc operations before _exit.
    let pipe_result = unsafe { libc::pipe(pipe_fds.as_mut_ptr()) };
    if pipe_result != 0 {
        return ReadOutcome {
            status: ReadStatus::Failure,
            bytes: 0,
            error: Some(io::Error::last_os_error()),
        };
    }
    let child = unsafe { libc::fork() };
    if child < 0 {
        unsafe {
            libc::close(pipe_fds[0]);
            libc::close(pipe_fds[1]);
        }
        return ReadOutcome {
            status: ReadStatus::Failure,
            bytes: 0,
            error: Some(io::Error::last_os_error()),
        };
    }
    if child == 0 {
        unsafe {
            libc::close(pipe_fds[0]);
            let fd = libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            let mut report = [0_u8; 16];
            if fd < 0 {
                report[8..12].copy_from_slice(
                    &io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(-1)
                        .to_ne_bytes(),
                );
            } else {
                let read = libc::read(fd, buffer.as_mut_ptr().cast(), max_bytes);
                libc::close(fd);
                if read >= 0 {
                    report[..8].copy_from_slice(&(read as u64).to_ne_bytes());
                } else {
                    report[8..12].copy_from_slice(
                        &io::Error::last_os_error()
                            .raw_os_error()
                            .unwrap_or(-1)
                            .to_ne_bytes(),
                    );
                }
            }
            let mut written = 0;
            while written < report.len() {
                let count = libc::write(
                    pipe_fds[1],
                    report[written..].as_ptr().cast(),
                    report.len() - written,
                );
                if count <= 0 {
                    break;
                }
                written += count as usize;
            }
            libc::close(pipe_fds[1]);
            libc::_exit(0);
        }
    }

    unsafe { libc::close(pipe_fds[1]) };
    let timeout_ms = budget.as_millis().min(i32::MAX as u128) as i32;
    let mut poll_fd = libc::pollfd {
        fd: pipe_fds[0],
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    let mut report = [0_u8; 16];
    let mut read_count = 0;
    if ready > 0 {
        while read_count < report.len() {
            let count = unsafe {
                libc::read(
                    pipe_fds[0],
                    report[read_count..].as_mut_ptr().cast(),
                    report.len() - read_count,
                )
            };
            if count <= 0 {
                break;
            }
            read_count += count as usize;
        }
    }
    unsafe { libc::close(pipe_fds[0]) };
    let mut status = 0;
    if ready <= 0 || read_count != report.len() {
        if ready == 0 {
            unsafe { libc::kill(child, libc::SIGKILL) };
        }
        unsafe { libc::waitpid(child, &mut status, 0) };
        return ReadOutcome {
            status: if ready == 0 {
                ReadStatus::Timeout
            } else {
                ReadStatus::Failure
            },
            bytes: 0,
            error: Some(if ready == 0 {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "bounded remote source read timed out",
                )
            } else {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "probe helper returned no result",
                )
            }),
        };
    }
    unsafe { libc::waitpid(child, &mut status, 0) };
    let bytes = u64::from_ne_bytes(report[..8].try_into().unwrap());
    let errno = i32::from_ne_bytes(report[8..12].try_into().unwrap());
    if errno != 0 {
        ReadOutcome {
            status: ReadStatus::Failure,
            bytes: 0,
            error: Some(io::Error::from_raw_os_error(errno)),
        }
    } else {
        ReadOutcome {
            status: ReadStatus::Success,
            bytes,
            error: None,
        }
    }
}

#[cfg(not(unix))]
fn bounded_prefix_read(path: &Path, max_bytes: usize, budget: Duration) -> ReadOutcome {
    use std::sync::mpsc;
    let path = path.to_path_buf();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = fs::File::open(path).and_then(|mut file| {
            let mut buffer = vec![0_u8; max_bytes];
            file.read(&mut buffer)
        });
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(budget) {
        Ok(Ok(bytes)) => ReadOutcome {
            status: ReadStatus::Success,
            bytes: bytes as u64,
            error: None,
        },
        Ok(Err(error)) => ReadOutcome {
            status: ReadStatus::Failure,
            bytes: 0,
            error: Some(error),
        },
        Err(_) => ReadOutcome {
            status: ReadStatus::Timeout,
            bytes: 0,
            error: Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "bounded remote source read timed out",
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn local_file_is_available_and_only_prefix_is_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.bin");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&vec![7_u8; DEFAULT_PROBE_BYTES * 4])
            .unwrap();
        let mut request = RemoteSourceProbeRequest::new(&path);
        request.max_bytes = 17;
        let result = probe_remote_source(&request);
        assert_eq!(result.availability, RemoteSourceAvailability::Available);
        assert_eq!(result.bytes_probed, 17);
        assert!(result.read_succeeded);
    }

    #[test]
    fn empty_file_is_valid_without_a_payload_read() {
        let directory = tempfile::tempdir().unwrap();
        let result = probe_remote_source(&RemoteSourceProbeRequest::new(
            directory.path().join("empty"),
        ));
        assert_eq!(result.availability, RemoteSourceAvailability::Unavailable);
        fs::File::create(directory.path().join("empty")).unwrap();
        let result = probe_remote_source(&RemoteSourceProbeRequest::new(
            directory.path().join("empty"),
        ));
        assert_eq!(result.availability, RemoteSourceAvailability::Available);
        assert_eq!(result.bytes_probed, 0);
    }

    #[test]
    fn missing_path_is_unavailable_without_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing");
        let result = probe_remote_source(&RemoteSourceProbeRequest::new(&path));
        assert_eq!(result.availability, RemoteSourceAvailability::Unavailable);
        assert!(!path.exists());
        assert_eq!(result.error_category, RemoteSourceErrorCategory::NotFound);
    }

    #[test]
    fn repeated_probe_preserves_identity_and_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("game.bin");
        fs::write(&path, b"fixture").unwrap();
        let request =
            RemoteSourceProbeRequest::new(&path).with_provenance("ratarmount-facing source");
        let first = probe_remote_source(&request);
        let second = probe_remote_source(&request);
        assert_eq!(first.path, second.path);
        assert_eq!(first.provenance, "ratarmount-facing source");
        assert_eq!(first.availability, second.availability);
        assert_eq!(first.bytes_probed, second.bytes_probed);
    }
}
