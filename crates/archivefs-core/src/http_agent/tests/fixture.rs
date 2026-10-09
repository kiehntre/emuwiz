use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(super) const HEADER_LIMIT: usize = 8192;
const FRAME_LIMIT: usize = 65536;
const READ_DEADLINE: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy)]
pub(super) enum Reply {
    Http,
    ProxyRefusal,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Seen {
    pub accepted: usize,
    pub headers: Vec<Vec<u8>>,
    pub tls_hellos: usize,
    pub errors: Vec<String>,
}

pub(super) struct Server {
    pub address: SocketAddr,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Seen>>,
    thread: Option<JoinHandle<()>>,
    pub joined: Arc<AtomicBool>,
}

impl Server {
    pub fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let worker_stop = stop.clone();
        let worker_seen = seen.clone();
        let thread = thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        worker_seen.lock().unwrap().accepted += 1;
                        let result = serve(&mut stream, reply, &worker_stop);
                        let mut seen = worker_seen.lock().unwrap();
                        match result {
                            Ok(Some(Captured::Header(header))) => seen.headers.push(header),
                            Ok(Some(Captured::TlsHello)) => seen.tls_hellos += 1,
                            Ok(None) => {}
                            Err(error) => seen.errors.push(error.to_string()),
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(POLL);
                    }
                    Err(error) => {
                        worker_seen.lock().unwrap().errors.push(error.to_string());
                        break;
                    }
                }
            }
        });
        Self {
            address,
            stop,
            seen,
            thread: Some(thread),
            joined: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn finish(&mut self) -> Seen {
        self.shutdown();
        self.seen.lock().unwrap().clone()
    }

    pub fn wait_for_connection(&self) {
        let deadline = Instant::now() + READ_DEADLINE;
        while self.seen.lock().unwrap().accepted == 0 {
            assert!(Instant::now() < deadline, "listener accepted no connection");
            thread::sleep(POLL);
        }
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.thread.take() {
            if worker.join().is_err() {
                // Drop must finish cleanup even while another assertion unwinds.
                let mut seen = self.seen.lock().unwrap_or_else(|error| error.into_inner());
                seen.errors.push("listener thread panicked".into());
            }
            self.joined.store(true, Ordering::SeqCst);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum Captured {
    Header(Vec<u8>),
    TlsHello,
}

fn serve(stream: &mut TcpStream, reply: Reply, stop: &AtomicBool) -> io::Result<Option<Captured>> {
    stream.set_read_timeout(Some(POLL))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let deadline = Instant::now() + READ_DEADLINE;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "incomplete request",
            ));
        }
        match stream.read(&mut buffer) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "partial request",
                ));
            }
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }

        if bytes.first() == Some(&22) {
            if bytes.len() > FRAME_LIMIT {
                return Err(io::Error::other("TLS frame limit exceeded"));
            }
            if bytes.len() >= 5 {
                let length = 5 + usize::from(u16::from_be_bytes([bytes[3], bytes[4]]));
                if length > FRAME_LIMIT {
                    return Err(io::Error::other("TLS frame limit exceeded"));
                }
                if bytes.len() >= length {
                    if bytes.get(5) != Some(&1) {
                        return Err(io::Error::other("expected a TLS ClientHello"));
                    }
                    // Deliberately close before a handshake completes. A full
                    // ClientHello proves direct HTTPS routing without a live
                    // TLS service or weakened certificate validation.
                    return Ok(Some(Captured::TlsHello));
                }
            }
        } else {
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                if end + 4 > HEADER_LIMIT {
                    return Err(io::Error::other("HTTP header limit exceeded"));
                }
                bytes.truncate(end + 4);
                stream.write_all(match reply {
                    Reply::Http => b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    // Never forward or complete a tunnel to any destination.
                    Reply::ProxyRefusal => b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                })?;
                return Ok(Some(Captured::Header(bytes)));
            }
            if bytes.len() >= HEADER_LIMIT {
                return Err(io::Error::other("HTTP header limit exceeded"));
            }
        }
    }
}

/// Owns the process and its output handles. No pipes can fill while the parent
/// polls, and an unwind always kills and reaps an unfinished child.
pub(super) struct ManagedChild {
    child: Child,
    stdout: File,
    stderr: File,
    pub reaped: Arc<AtomicBool>,
    pub isolation_root: Option<PathBuf>,
}

pub(super) struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl ManagedChild {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        // These children re-execute this test binary. Its existing isolation
        // constructor uses a sibling <binary>-<pid> tree. A killed child cannot
        // run that constructor's exit handler, so its parent must own cleanup.
        assert_eq!(command.get_program(), std::env::current_exe()?.as_os_str());
        let isolation = crate::test_environment::isolated_root().map(|root| {
            let suffix = format!("-{}", std::process::id());
            let prefix = root
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .strip_suffix(&suffix)
                .expect("test isolation root includes the parent PID");
            (root.parent().unwrap().to_path_buf(), prefix.to_owned())
        });
        let stdout = tempfile::tempfile()?;
        let stderr = tempfile::tempfile()?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::from(stderr.try_clone()?));
        let child = command.spawn()?;
        let isolation_root =
            isolation.map(|(parent, prefix)| parent.join(format!("{prefix}-{}", child.id())));
        Ok(Self {
            child,
            stdout,
            stderr,
            reaped: Arc::new(AtomicBool::new(false)),
            isolation_root,
        })
    }

    pub fn wait(&mut self, timeout: Duration) -> io::Result<Output> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.reaped.store(true, Ordering::SeqCst);
                self.clear_isolation()?;
                return Ok(Output {
                    status,
                    stdout: read_output(&mut self.stdout)?,
                    stderr: read_output(&mut self.stderr)?,
                });
            }
            if Instant::now() >= deadline {
                self.terminate();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child deadline exceeded",
                ));
            }
            thread::sleep(POLL);
        }
    }

    fn terminate(&mut self) {
        if !self.reaped.load(Ordering::SeqCst) {
            let _ = self.child.kill();
            if self.child.wait().is_ok() {
                self.reaped.store(true, Ordering::SeqCst);
            }
        }
        if self.reaped.load(Ordering::SeqCst) {
            let _ = self.clear_isolation();
        }
    }

    fn clear_isolation(&self) -> io::Result<()> {
        let Some(root) = &self.isolation_root else {
            return Ok(());
        };
        match std::fs::symlink_metadata(root) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                std::fs::remove_dir_all(root)
            }
            Ok(_) => Err(io::Error::other("unexpected child isolation root")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn read_output(file: &mut File) -> io::Result<String> {
    const LIMIT: u64 = 65536;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(io::Error::other("child output limit exceeded"));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
