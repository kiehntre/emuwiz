//! Ask an installed libretro core for its own `library_name`.
//!
//! RetroArch names a core's per-game cheat folder after
//! `retro_system_info.library_name`, which differs from the `.info` `corename`,
//! the file stem and RetroArch's own folders for many real cores. The only
//! reliable source is the core itself, so this module calls its exported
//! `retro_get_system_info` -- which means running untrusted native code.
//!
//! # Threat model
//!
//! The core is user-installed code RetroArch will run with full user
//! privileges anyway. What must not happen is the *probe* becoming an
//! amplifier: loading the library must not be able to write a file, reach a
//! network or local socket, start a process, signal EmuWiz, or run inside the
//! GUI. Therefore:
//!
//! * the core is loaded only in a separate, short-lived child (the current
//!   executable re-run with [`PROBE_ARGUMENT`]); never in the calling process;
//! * the child sandboxes itself **before** the core is touched, in this order:
//!   parent-death signal, no-new-privileges, every inherited descriptor except
//!   stdio and the result file closed, resource limits, a Landlock filesystem
//!   ruleset that grants read-only access to the core file and the system
//!   library folders and nothing else, and a seccomp filter that refuses
//!   sockets, new processes, exec, signals to other processes, process-group
//!   changes, ptrace, mounts and similar;
//! * the answer travels through a plain in-memory file (`memfd`), never a pipe:
//!   a pipe could be armed for asynchronous signal delivery (`F_SETOWN`,
//!   `O_ASYNC`) and used to signal the caller;
//! * the child then **proves** each restriction by trying to break it (a real
//!   socket, a real write, a real exec, a real fork, a descriptor audit, a
//!   limit read-back) and only reports success if every attempt fails;
//! * any setup failure, failed proof, timeout, crash, missing symbol or
//!   malformed answer is a refusal. There is no unsandboxed fallback.
//!
//! The returned name is a claim by untrusted code. It is only ever used after
//! being validated as a plain folder-name component; a lying core can at worst
//! make its own cheats not load. The probed file's identity is captured before
//! and after the probe and must be re-checked immediately before launch
//! ([`ProbedCore::ensure_unchanged`]).

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::process_spawn::open_regular_file_nonblocking;
use crate::patch_manager::is_safe_cheat_name_part;

/// First argument that turns an EmuWiz executable into the probe child.
pub const PROBE_ARGUMENT: &str = "--emuwiz-internal-libretro-core-probe-v1";

const ISOLATED_LINE: &str = "EMUWIZ-PROBE-ISOLATED";
const NAME_PREFIX: &str = "EMUWIZ-PROBE-NAME ";
const FAIL_PREFIX: &str = "EMUWIZ-PROBE-FAIL ";
/// The descriptor the child writes its answer to (a memfd, see above).
const RESULT_FD: RawFd = 3;
/// Largest answer file the child may write (RLIMIT_FSIZE).
const RESULT_FILE_LIMIT: u64 = 4096;
const MAX_OUTPUT_BYTES: usize = 512;
const MAX_NAME_BYTES: usize = 200;
const WALL_TIMEOUT: Duration = Duration::from_secs(5);
const CPU_SECONDS: u64 = 3;
const ADDRESS_SPACE_BYTES: u64 = 4 << 30;
const OPEN_FILES: u64 = 64;

/// Why a core's library name could not be established. Every variant means
/// "do not offer cheat launching for this core"; none affects a plain launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The core file is missing, a link, or not a regular file.
    CoreUnreadable(String),
    /// The sandbox could not be established or could not be proven. The core
    /// was not executed.
    IsolationUnavailable(String),
    /// The helper did not finish in time and was killed.
    Timeout,
    /// The helper crashed or was killed (for example by a signal).
    Crashed(String),
    /// The core does not export `retro_get_system_info`, or could not be loaded.
    NotALibretroCore(String),
    /// The answer was missing, oversized or not in the expected form.
    InvalidAnswer(String),
    /// The name is not a safe single folder-name component.
    UnsafeName,
    /// The core file changed while it was being probed, or since.
    CoreChanged,
    /// The helper could not be started.
    Spawn(String),
}

impl ProbeError {
    /// Plain-language reason shown beside a disabled cheat chooser.
    #[must_use]
    pub fn explanation(&self) -> String {
        match self {
            Self::IsolationUnavailable(why) => format!(
                "EmuWiz cannot safely inspect this core on this computer ({why}), so cheats are unavailable for it. The game still launches normally."
            ),
            Self::CoreChanged => "The core file changed, so its name can't be confirmed. Cheats are unavailable; the game still launches normally.".into(),
            other => format!(
                "EmuWiz could not confirm this core's library name ({other:?}), so cheats are unavailable for it. The game still launches normally."
            ),
        }
    }
}

/// The identity of a core file: device, inode, size, and modification and
/// status-change times (the latter moves even if a rewrite restores mtime).
/// A launch-time recheck narrows, but cannot close, the gap between this check
/// and RetroArch loading the file itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime: (i64, i64),
    pub ctime: (i64, i64),
}

impl CoreIdentity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

/// A core whose library name was read from the core itself, with the file
/// identity it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedCore {
    pub core_path: PathBuf,
    pub identity: CoreIdentity,
    pub library_name: String,
}

impl ProbedCore {
    /// The core file must still be exactly the one that was probed. Call this
    /// immediately before launching with cheats.
    pub fn ensure_unchanged(&self) -> Result<(), ProbeError> {
        match identity_of(&self.core_path) {
            Ok(now) if now == self.identity => Ok(()),
            _ => Err(ProbeError::CoreChanged),
        }
    }
}

/// How the child process is started. Production re-runs the current
/// executable; tests re-run the test binary.
pub trait ProbeLauncher {
    /// Program, leading arguments, and extra environment. The environment is
    /// otherwise empty: the probe child gets nothing from the caller's.
    fn child(&self) -> io::Result<(PathBuf, Vec<String>, Vec<(String, String)>)>;
}

/// Re-runs the current executable in probe mode.
pub struct CurrentExecutableLauncher;

impl ProbeLauncher for CurrentExecutableLauncher {
    fn child(&self) -> io::Result<(PathBuf, Vec<String>, Vec<(String, String)>)> {
        Ok((
            std::env::current_exe()?,
            vec![PROBE_ARGUMENT.to_string()],
            Vec::new(),
        ))
    }
}

fn identity_of(path: &Path) -> io::Result<CoreIdentity> {
    Ok(CoreIdentity::of(
        &open_regular_file_nonblocking(path)?.metadata()?,
    ))
}

/// Asks `core_path` for its library name in an isolated child.
pub fn probe_core_library_name(
    core_path: &Path,
    launcher: &dyn ProbeLauncher,
) -> Result<ProbedCore, ProbeError> {
    let file = open_regular_file_nonblocking(core_path)
        .map_err(|e| ProbeError::CoreUnreadable(e.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|e| ProbeError::CoreUnreadable(e.to_string()))?;
    let identity = CoreIdentity::of(&metadata);
    drop(file);

    let (program, leading, environment) = launcher
        .child()
        .map_err(|e| ProbeError::Spawn(e.to_string()))?;
    // The answer file. A plain in-memory file, not a pipe: a pipe can be armed
    // for asynchronous signal delivery to its owner, which would let a hostile
    // core signal this process.
    let name = CString::new("emuwiz-libretro-probe").unwrap();
    // SAFETY: memfd_create with a valid NUL-terminated name.
    let result_fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if result_fd < 0 {
        return Err(ProbeError::Spawn(io::Error::last_os_error().to_string()));
    }
    let mut command = Command::new(program);
    command
        .args(leading)
        .arg(core_path)
        .arg(identity.device.to_string())
        .arg(identity.inode.to_string())
        .arg(identity.size.to_string())
        .arg(identity.mtime.0.to_string())
        .arg(identity.mtime.1.to_string())
        .arg(identity.ctime.0.to_string())
        .arg(identity.ctime.1.to_string())
        .arg(std::process::id().to_string())
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    // SAFETY: the closure only calls async-signal-safe `dup2`/`fcntl`.
    unsafe {
        command.pre_exec(move || {
            if result_fd == RESULT_FD {
                if libc::fcntl(RESULT_FD, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
            } else if libc::dup2(result_fd, RESULT_FD) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let spawned = command.spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            // SAFETY: closing the descriptor created above, once.
            unsafe { libc::close(result_fd) };
            return Err(ProbeError::Spawn(error.to_string()));
        }
    };

    let deadline = Instant::now() + WALL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => break None,
            Err(error) => {
                terminate(&mut child);
                // SAFETY: closing the descriptor created above, once.
                unsafe { libc::close(result_fd) };
                return Err(ProbeError::Crashed(error.to_string()));
            }
        }
    };
    let Some(status) = status else {
        terminate(&mut child);
        // SAFETY: closing the descriptor created above, once.
        unsafe { libc::close(result_fd) };
        return Err(ProbeError::Timeout);
    };
    // No group kill here: the child is already reaped, its id could be reused,
    // and fork/clone are refused so it cannot have left anything behind.
    let output = read_result(result_fd);
    // SAFETY: closing the descriptor created above, once.
    unsafe { libc::close(result_fd) };
    let output = output?;
    let text = String::from_utf8_lossy(&output).into_owned();

    if let Some(failure) = text.lines().find_map(|line| line.strip_prefix(FAIL_PREFIX)) {
        return Err(map_failure(failure));
    }
    if !status.success() {
        use std::os::unix::process::ExitStatusExt;
        return Err(ProbeError::Crashed(match status.signal() {
            Some(signal) => format!("terminated by signal {signal}"),
            None => format!("exit status {:?}", status.code()),
        }));
    }
    let lines: Vec<&str> = text.lines().collect();
    let [isolated, answer] = lines.as_slice() else {
        return Err(ProbeError::InvalidAnswer(
            "expected exactly the isolation proof and one name".into(),
        ));
    };
    if *isolated != ISOLATED_LINE {
        return Err(ProbeError::IsolationUnavailable(
            "the helper did not prove its isolation".into(),
        ));
    }
    let name = answer
        .strip_prefix(NAME_PREFIX)
        .ok_or_else(|| ProbeError::InvalidAnswer("no name line".into()))?;
    if name.len() > MAX_NAME_BYTES || !is_safe_cheat_name_part(name) {
        return Err(ProbeError::UnsafeName);
    }
    // The file must still be the one that was probed.
    match identity_of(core_path) {
        Ok(after) if after == identity => Ok(ProbedCore {
            core_path: core_path.to_path_buf(),
            identity,
            library_name: name.to_string(),
        }),
        _ => Err(ProbeError::CoreChanged),
    }
}

/// Ends a helper that overran its time. The helper itself is signalled by id
/// first (it is not yet reaped, so the id is its own), then its group.
fn terminate(child: &mut std::process::Child) {
    let _ = child.kill();
    // SAFETY: signalling the helper's own process group (`process_group(0)` at
    // spawn), only while the helper has not been reaped; never the caller's group.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.wait();
}

/// The helper's answer, bounded. The helper has exited, so the file is final.
fn read_result(fd: RawFd) -> Result<Vec<u8>, ProbeError> {
    // SAFETY: fstat on a descriptor this function's caller owns.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(ProbeError::InvalidAnswer("answer unreadable".into()));
    }
    if stat.st_size as u64 > MAX_OUTPUT_BYTES as u64 {
        return Err(ProbeError::InvalidAnswer("answer too long".into()));
    }
    let mut buffer = vec![0u8; stat.st_size as usize];
    let mut read = 0usize;
    while read < buffer.len() {
        // SAFETY: reading into the unfilled tail of a live buffer.
        let count = unsafe {
            libc::pread(
                fd,
                buffer[read..].as_mut_ptr().cast::<c_void>(),
                buffer.len() - read,
                read as libc::off_t,
            )
        };
        if count <= 0 {
            break;
        }
        read += count as usize;
    }
    buffer.truncate(read);
    Ok(buffer)
}

fn map_failure(failure: &str) -> ProbeError {
    let (kind, detail) = failure.split_once(' ').unwrap_or((failure, ""));
    let detail = detail.to_string();
    match kind {
        "isolation" => ProbeError::IsolationUnavailable(detail),
        "identity" => ProbeError::CoreChanged,
        "unloadable" | "nosymbol" => ProbeError::NotALibretroCore(detail),
        "name" => ProbeError::UnsafeName,
        other => ProbeError::InvalidAnswer(format!("{other} {detail}")),
    }
}

// =====================================================================
// The child
// =====================================================================

/// Runs in the helper. Never returns.
pub fn run_probe_child(arguments: &[String]) -> ! {
    let outcome = child_steps(arguments);
    let code = match outcome {
        Ok(name) => {
            write_result(&format!("{NAME_PREFIX}{name}\n"));
            0
        }
        Err(failure) => {
            write_result(&format!("{FAIL_PREFIX}{failure}\n"));
            2
        }
    };
    // SAFETY: terminating without running destructors or atexit handlers the
    // loaded core may have registered.
    unsafe { libc::_exit(code) }
}

/// Dispatch used by the GUI: if the process was started in probe mode, run it.
/// Must be the very first thing `main` does, before any thread or file.
pub fn dispatch_if_probe_child() {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() == Some(PROBE_ARGUMENT) {
        let rest: Vec<String> = arguments.collect();
        run_probe_child(&rest);
    }
}

fn write_result(text: &str) {
    let bytes = text.as_bytes();
    let mut written = 0;
    while written < bytes.len() {
        // SAFETY: writing from a valid slice to the result descriptor.
        let count = unsafe {
            libc::write(
                RESULT_FD,
                bytes[written..].as_ptr().cast::<c_void>(),
                bytes.len() - written,
            )
        };
        if count <= 0 {
            return;
        }
        written += count as usize;
    }
}

fn errno() -> c_int {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Places the proofs need, chosen BEFORE the restrictions apply so that a
/// refusal afterwards can only be the sandbox: they are certainly writable or
/// readable by ordinary permissions.
struct ProofTargets {
    writable_directory: PathBuf,
    readable_file: PathBuf,
}

fn choose_proof_targets() -> Result<ProofTargets, String> {
    let first = |candidates: &[&str], mode: c_int| {
        candidates.iter().find_map(|path| {
            let c = CString::new(*path).ok()?;
            // SAFETY: access(2) on a fixed path.
            (unsafe { libc::access(c.as_ptr(), mode) } == 0).then(|| PathBuf::from(path))
        })
    };
    let writable_directory = first(&["/tmp", "/var/tmp", "/dev/shm"], libc::W_OK | libc::X_OK)
        .ok_or("isolation no writable place to prove write denial against")?;
    let readable_file = first(
        &[
            "/etc/passwd",
            "/etc/hostname",
            "/etc/os-release",
            "/etc/hosts",
        ],
        libc::R_OK,
    )
    .ok_or("isolation no readable file to prove read denial against")?;
    Ok(ProofTargets {
        writable_directory,
        readable_file,
    })
}

fn child_steps(arguments: &[String]) -> Result<String, String> {
    let [
        path,
        dev,
        ino,
        size,
        mtime,
        mtime_nsec,
        ctime,
        ctime_nsec,
        parent,
    ] = arguments
    else {
        return Err("isolation wrong argument count".into());
    };
    let expected = CoreIdentity {
        device: dev.parse().map_err(|_| "isolation bad dev")?,
        inode: ino.parse().map_err(|_| "isolation bad ino")?,
        size: size.parse().map_err(|_| "isolation bad size")?,
        mtime: (
            mtime.parse().map_err(|_| "isolation bad mtime")?,
            mtime_nsec.parse().map_err(|_| "isolation bad mtime_nsec")?,
        ),
        ctime: (
            ctime.parse().map_err(|_| "isolation bad ctime")?,
            ctime_nsec.parse().map_err(|_| "isolation bad ctime_nsec")?,
        ),
    };
    let parent: i32 = parent.parse().map_err(|_| "isolation bad parent")?;

    // 0. Die with the caller: no orphan survives it. The check closes the race
    // where the caller died before this call took effect.
    // SAFETY: plain prctl.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) } != 0 {
        return Err(format!("isolation pdeathsig: errno {}", errno()));
    }
    // SAFETY: getppid cannot fail.
    if unsafe { libc::getppid() } != parent {
        return Err("isolation the caller is gone".into());
    }
    // The result file must be the plain in-memory file we were promised.
    let mut result_stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat on the inherited result descriptor.
    if unsafe { libc::fstat(RESULT_FD, &mut result_stat) } != 0
        || (result_stat.st_mode & libc::S_IFMT) != libc::S_IFREG
    {
        return Err("isolation the result channel is not a plain file".into());
    }

    // 1. No privilege gain, ever.
    // SAFETY: plain prctl.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!("isolation no_new_privs: errno {}", errno()));
    }
    // SAFETY: plain prctl read.
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 1 {
        return Err("isolation unproven: no_new_privs not set".into());
    }
    let mut death_signal: c_int = 0;
    // SAFETY: valid out-pointer for PR_GET_PDEATHSIG.
    if unsafe {
        libc::prctl(
            libc::PR_GET_PDEATHSIG,
            &mut death_signal as *mut c_int,
            0,
            0,
            0,
        )
    } != 0
        || death_signal != libc::SIGKILL
    {
        return Err("isolation unproven: parent-death signal not set".into());
    }
    // Facts the proofs need, gathered while ordinary permissions still apply.
    let targets = choose_proof_targets()?;
    // 2. Nothing inherited survives except stdio and the result file.
    close_inherited_descriptors()?;
    // 3. Resource limits.
    apply_limits()?;
    // 4. Open the exact core file we were told about, no-follow.
    let core = open_regular_file_nonblocking(Path::new(path))
        .map_err(|e| format!("identity unreadable: {e}"))?;
    let meta = core.metadata().map_err(|e| format!("identity {e}"))?;
    if CoreIdentity::of(&meta) != expected {
        return Err("identity changed before probe".into());
    }
    let core_fd = core.as_raw_fd();
    // 5. Filesystem: read-only core file and system libraries only.
    landlock_restrict(core_fd)?;
    // 6. Syscalls: no sockets, processes, exec, foreign signals, ...
    seccomp_restrict()?;
    // 7. Prove all of it by trying to break it.
    prove_isolation(Path::new(path), core_fd, &targets)?;
    write_result(&format!("{ISOLATED_LINE}\n"));

    // 8. Only now run the core.
    let cpath = CString::new(Path::new(path).as_os_str().as_bytes())
        .map_err(|_| "unloadable path contains NUL".to_string())?;
    // SAFETY: loading the (sandboxed) core is the purpose of this process.
    let handle = unsafe { libc::dlopen(cpath.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return Err("unloadable dlopen failed".into());
    }
    let symbol = CString::new("retro_get_system_info").unwrap();
    // SAFETY: handle is a live dlopen handle.
    let function = unsafe { libc::dlsym(handle, symbol.as_ptr()) };
    if function.is_null() {
        return Err("nosymbol retro_get_system_info".into());
    }
    #[repr(C)]
    struct SystemInfo {
        library_name: *const c_char,
        library_version: *const c_char,
        valid_extensions: *const c_char,
        need_fullpath: bool,
        block_extract: bool,
    }
    let mut info = SystemInfo {
        library_name: std::ptr::null(),
        library_version: std::ptr::null(),
        valid_extensions: std::ptr::null(),
        need_fullpath: false,
        block_extract: false,
    };
    // SAFETY: libretro's documented signature `void retro_get_system_info(struct retro_system_info*)`.
    let get: extern "C" fn(*mut SystemInfo) = unsafe { std::mem::transmute(function) };
    get(&mut info);
    if info.library_name.is_null() {
        return Err("name no library_name".into());
    }
    // Bounded read: never trust the core's string to be terminated sensibly.
    // SAFETY: reading at most MAX_NAME_BYTES+1 bytes from a pointer the core gave us;
    // a hostile pointer faults inside this sandboxed child, which is a refusal.
    let bytes: Vec<u8> = (0..=MAX_NAME_BYTES)
        .map(|i| unsafe { *info.library_name.cast::<u8>().add(i) })
        .take_while(|b| *b != 0)
        .collect();
    if bytes.len() > MAX_NAME_BYTES {
        return Err("name too long".into());
    }
    let name = String::from_utf8(bytes).map_err(|_| "name not utf8".to_string())?;
    if name.contains(['\n', '\r']) {
        return Err("name newline".into());
    }
    Ok(name)
}

fn is_open(fd: RawFd) -> bool {
    // SAFETY: F_GETFD on any integer is harmless.
    unsafe { libc::fcntl(fd, libc::F_GETFD) != -1 }
}

fn close_inherited_descriptors() -> Result<(), String> {
    // Keep 0,1,2 and RESULT_FD; close everything else.
    // SAFETY: close_range with valid arguments.
    let high = unsafe { libc::syscall(libc::SYS_close_range, RESULT_FD as u32 + 1, u32::MAX, 0) };
    if high != 0 {
        // Older kernel: close the range one by one.
        for fd in (RESULT_FD + 1)..4096 {
            // SAFETY: closing an arbitrary integer descriptor is harmless.
            unsafe { libc::close(fd) };
        }
    }
    Ok(())
}

fn apply_limits() -> Result<(), String> {
    let limits: [(libc::__rlimit_resource_t, u64); 7] = [
        (libc::RLIMIT_CORE, 0),
        (libc::RLIMIT_CPU, CPU_SECONDS),
        (libc::RLIMIT_AS, ADDRESS_SPACE_BYTES),
        (libc::RLIMIT_NOFILE, OPEN_FILES),
        (libc::RLIMIT_FSIZE, RESULT_FILE_LIMIT),
        (libc::RLIMIT_NPROC, 0),
        (libc::RLIMIT_MEMLOCK, 0),
    ];
    for (resource, value) in limits {
        let limit = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        // SAFETY: valid rlimit pointer.
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(format!("isolation setrlimit {resource}: errno {}", errno()));
        }
    }
    Ok(())
}

// ---- Landlock ---------------------------------------------------------

const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: c_int = 1;
const ACCESS_FS_READ_FILE: u64 = 1 << 2;
const ACCESS_FS_READ_DIR: u64 = 1 << 3;

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

#[repr(C, packed)]
struct LandlockPathBeneath {
    allowed_access: u64,
    parent_fd: i32,
}

fn landlock_restrict(core_fd: RawFd) -> Result<(), String> {
    #[cfg(test)]
    if std::env::var_os("EMUWIZ_PROBE_TEST_SKIP_LANDLOCK").is_some() {
        return Ok(());
    }
    // SAFETY: querying the ABI version takes no pointer.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<LandlockRulesetAttr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if abi < 1 {
        return Err(format!(
            "isolation landlock unavailable (abi {abi}, errno {})",
            errno()
        ));
    }
    // Every filesystem access this kernel can restrict is denied by default.
    let mut handled: u64 = (1 << 13) - 1; // EXECUTE .. MAKE_SYM (ABI 1)
    if abi >= 2 {
        handled |= 1 << 13; // REFER
    }
    if abi >= 3 {
        handled |= 1 << 14; // TRUNCATE
    }
    if abi >= 5 {
        handled |= 1 << 15; // IOCTL_DEV
    }
    let attr = LandlockRulesetAttr {
        handled_access_fs: handled,
    };
    // SAFETY: valid attr pointer and size.
    let ruleset = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr as *const LandlockRulesetAttr,
            std::mem::size_of::<LandlockRulesetAttr>(),
            0u32,
        )
    };
    if ruleset < 0 {
        return Err(format!("isolation landlock create: errno {}", errno()));
    }
    let ruleset = ruleset as c_int;
    let read_only = ACCESS_FS_READ_FILE | ACCESS_FS_READ_DIR;
    let add = |fd: c_int, access: u64| -> Result<(), String> {
        let rule = LandlockPathBeneath {
            allowed_access: access,
            parent_fd: fd,
        };
        // SAFETY: valid rule pointer for the documented rule type.
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset,
                LANDLOCK_RULE_PATH_BENEATH,
                &rule as *const LandlockPathBeneath,
                0u32,
            )
        };
        if result != 0 {
            return Err(format!("isolation landlock add_rule: errno {}", errno()));
        }
        Ok(())
    };
    // The core file itself (by descriptor: exactly the inode we verified).
    add(core_fd, ACCESS_FS_READ_FILE)?;
    // System library locations needed to resolve the core's own dependencies.
    for path in [
        "/usr",
        "/lib",
        "/lib64",
        "/lib32",
        "/etc/ld.so.cache",
        "/etc/ld.so.conf",
        "/etc/ld.so.conf.d",
    ] {
        let c = CString::new(path).unwrap();
        // SAFETY: opening a fixed path read-only for a rule; closed below.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            continue; // not present on this system
        }
        // A directory rule may carry directory rights; a file rule may not.
        // SAFETY: fstat on the descriptor just opened.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let is_dir = unsafe { libc::fstat(fd, &mut stat) } == 0
            && (stat.st_mode & libc::S_IFMT) == libc::S_IFDIR;
        let result = add(
            fd,
            if is_dir {
                read_only
            } else {
                ACCESS_FS_READ_FILE
            },
        );
        // SAFETY: closing the descriptor we just opened.
        unsafe { libc::close(fd) };
        result?;
    }
    // SAFETY: restrict_self on the ruleset we built, after no_new_privs.
    let restricted = unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0u32) };
    // SAFETY: closing the ruleset descriptor.
    unsafe { libc::close(ruleset) };
    if restricted != 0 {
        return Err(format!(
            "isolation landlock restrict_self: errno {}",
            errno()
        ));
    }
    Ok(())
}

// ---- seccomp ----------------------------------------------------------

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_JMP_JSET_K: u16 = 0x45;
const BPF_RET_K: u16 = 0x06;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_SET_MODE_FILTER: c_int = 1;
const SECCOMP_FILTER_FLAG_TSYNC: c_int = 1;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xC000_00B7;

/// Syscalls refused with EPERM. Everything else a plain library load needs is
/// allowed; the filesystem is Landlock's job.
fn denied_syscalls() -> Vec<i64> {
    let mut list = vec![
        // Network and local sockets.
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_sendto,
        libc::SYS_sendmsg,
        libc::SYS_sendmmsg,
        // New processes and programs.
        libc::SYS_execve,
        libc::SYS_execveat,
        libc::SYS_clone,
        libc::SYS_clone3,
        // Reaching other processes (EmuWiz itself runs as the same user).
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_kill,
        libc::SYS_tkill,
        libc::SYS_tgkill,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_send_signal,
        // Changing the world around it.
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_setns,
        libc::SYS_unshare,
        libc::SYS_open_by_handle_at,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_userfaultfd,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_reboot,
        libc::SYS_kexec_load,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_acct,
        libc::SYS_settimeofday,
        libc::SYS_clock_settime,
        libc::SYS_adjtimex,
        libc::SYS_sethostname,
        libc::SYS_setdomainname,
        libc::SYS_quotactl,
        // Other processes' descriptors, memory advice, scheduling and IPC.
        libc::SYS_pidfd_getfd,
        libc::SYS_kcmp,
        libc::SYS_process_madvise,
        libc::SYS_setpriority,
        libc::SYS_sched_setscheduler,
        libc::SYS_sched_setparam,
        libc::SYS_sched_setaffinity,
        libc::SYS_sched_setattr,
        libc::SYS_shmget,
        libc::SYS_shmat,
        libc::SYS_shmctl,
        libc::SYS_shmdt,
        libc::SYS_msgget,
        libc::SYS_msgsnd,
        libc::SYS_msgrcv,
        libc::SYS_msgctl,
        libc::SYS_semget,
        libc::SYS_semop,
        libc::SYS_semctl,
        libc::SYS_semtimedop,
        libc::SYS_mq_open,
        // File metadata: Landlock does not govern these, so they are refused
        // here (a core must not be able to change a user file's mode, owner,
        // times or attributes).
        libc::SYS_fchmod,
        libc::SYS_fchmodat,
        452, // fchmodat2
        libc::SYS_fchown,
        libc::SYS_fchownat,
        libc::SYS_utimensat,
        libc::SYS_setxattr,
        libc::SYS_lsetxattr,
        libc::SYS_fsetxattr,
        libc::SYS_removexattr,
        libc::SYS_lremovexattr,
        libc::SYS_fremovexattr,
        // Process-group and session changes (a core must not leave the group
        // the caller will kill on timeout), thread/parent-death settings,
        // truncation (Landlock only guards it from ABI 3), and a few more ways
        // to affect other processes or the mount namespace.
        libc::SYS_setpgid,
        libc::SYS_setsid,
        libc::SYS_prctl,
        libc::SYS_truncate,
        libc::SYS_ftruncate,
        libc::SYS_ioprio_set,
        libc::SYS_migrate_pages,
        libc::SYS_move_pages,
        libc::SYS_mq_unlink,
        428, // open_tree
        429, // move_mount
        430, // fsopen
        431, // fsconfig
        432, // fsmount
        433, // fspick
        442, // mount_setattr
    ];
    #[cfg(target_arch = "x86_64")]
    list.extend([
        libc::SYS_fork,
        libc::SYS_vfork,
        libc::SYS_iopl,
        libc::SYS_ioperm,
        libc::SYS_chmod,
        libc::SYS_chown,
        libc::SYS_lchown,
        libc::SYS_utime,
        libc::SYS_utimes,
        libc::SYS_futimesat,
        libc::SYS_uselib,
    ]);
    list
}

fn seccomp_restrict() -> Result<(), String> {
    #[cfg(test)]
    if std::env::var_os("EMUWIZ_PROBE_TEST_SKIP_SECCOMP").is_some() {
        return Ok(());
    }
    let denied = denied_syscalls();
    let mut program: Vec<SockFilter> = Vec::new();
    let op = |code, jt, jf, k| SockFilter { code, jt, jf, k };
    // arch check
    program.push(op(BPF_LD_W_ABS, 0, 0, 4));
    program.push(op(BPF_JMP_JEQ_K, 1, 0, AUDIT_ARCH));
    program.push(op(BPF_RET_K, 0, 0, SECCOMP_RET_KILL_PROCESS));
    // syscall number
    program.push(op(BPF_LD_W_ABS, 0, 0, 0));
    let first_test = program.len();
    // Layout: [x32 test?] [denied tests...] [prlimit64 test] then
    //   allow, prl_ld, prl_jeq, allow, deny, kill
    // prlimit64 is allowed only for the calling process itself (pid argument 0):
    // setting limits on another process of the same user is refused.
    let x32 = cfg!(target_arch = "x86_64");
    let tests = denied.len() + usize::from(x32) + 1;
    let allow_at = first_test + tests;
    let prl_ld_at = allow_at + 1;
    let deny_at = allow_at + 4;
    let kill_at = allow_at + 5;
    let mut position = first_test;
    if x32 {
        // The x32 ABI shares AUDIT_ARCH_X86_64; refuse it outright.
        program.push(op(
            BPF_JMP_JSET_K,
            (kill_at - position - 1) as u8,
            0,
            0x4000_0000,
        ));
        position += 1;
    }
    for number in &denied {
        program.push(op(
            BPF_JMP_JEQ_K,
            (deny_at - position - 1) as u8,
            0,
            *number as u32,
        ));
        position += 1;
    }
    program.push(op(
        BPF_JMP_JEQ_K,
        (prl_ld_at - position - 1) as u8,
        0,
        libc::SYS_prlimit64 as u32,
    ));
    program.push(op(BPF_RET_K, 0, 0, SECCOMP_RET_ALLOW)); // allow_at
    program.push(op(BPF_LD_W_ABS, 0, 0, 16)); // prl_ld_at: pid argument
    program.push(op(BPF_JMP_JEQ_K, 0, 1, 0)); // pid == 0 -> allow, else deny
    program.push(op(BPF_RET_K, 0, 0, SECCOMP_RET_ALLOW));
    program.push(op(BPF_RET_K, 0, 0, SECCOMP_RET_ERRNO | libc::EPERM as u32)); // deny_at
    program.push(op(BPF_RET_K, 0, 0, SECCOMP_RET_KILL_PROCESS)); // kill_at
    if program.len() > u16::MAX as usize || denied.len() + 8 > 250 {
        // Jump offsets are 8-bit; a list this long would need a different layout.
        return Err("isolation seccomp program too large".into());
    }
    let fprog = SockFprog {
        len: program.len() as u16,
        filter: program.as_ptr(),
    };
    // SAFETY: valid fprog for the duration of the call.
    let result = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            SECCOMP_FILTER_FLAG_TSYNC,
            &fprog as *const SockFprog,
        )
    };
    if result != 0 {
        return Err(format!("isolation seccomp: errno {}", errno()));
    }
    Ok(())
}

// ---- proof ------------------------------------------------------------

fn prove_isolation(core_path: &Path, core_fd: RawFd, targets: &ProofTargets) -> Result<(), String> {
    let fail = |what: &str| Err(format!("isolation unproven: {what}"));
    let denied_with = |expected: c_int| errno() == expected;
    // Limits read back.
    for (resource, expected) in [
        (libc::RLIMIT_CORE, 0u64),
        (libc::RLIMIT_CPU, CPU_SECONDS),
        (libc::RLIMIT_AS, ADDRESS_SPACE_BYTES),
        (libc::RLIMIT_NOFILE, OPEN_FILES),
        (libc::RLIMIT_FSIZE, RESULT_FILE_LIMIT),
        (libc::RLIMIT_NPROC, 0),
        (libc::RLIMIT_MEMLOCK, 0),
    ] {
        let mut limit = libc::rlimit {
            rlim_cur: 1,
            rlim_max: 1,
        };
        // SAFETY: valid rlimit out-pointer.
        if unsafe { libc::getrlimit(resource, &mut limit) } != 0
            || limit.rlim_cur != expected
            || limit.rlim_max != expected
        {
            return fail("a resource limit did not read back");
        }
    }
    // Descriptors: only stdio, the result file and the core file may be open.
    for fd in 3..(OPEN_FILES as RawFd) {
        let must_be_open = fd == RESULT_FD || fd == core_fd;
        if is_open(fd) != must_be_open {
            return fail("an unexpected descriptor is open or a required one is closed");
        }
    }
    // Network: real attempts must be refused by the sandbox (EPERM).
    for domain in [
        libc::AF_INET,
        libc::AF_INET6,
        libc::AF_UNIX,
        libc::AF_NETLINK,
    ] {
        // SAFETY: socket() with constant arguments.
        let fd = unsafe { libc::socket(domain, libc::SOCK_STREAM, 0) };
        if fd >= 0 {
            // SAFETY: closing the descriptor just created.
            unsafe { libc::close(fd) };
            return fail("a socket could be created");
        }
        if !denied_with(libc::EPERM) {
            return fail("socket creation was not refused by the sandbox");
        }
    }
    // Writes. The place was chosen as certainly writable by ordinary
    // permissions, so only the sandbox can explain EACCES here.
    let probe_name = targets.writable_directory.join(format!(
        ".emuwiz-probe-{}-{}",
        std::process::id(),
        core_fd
    ));
    let create = CString::new(probe_name.as_os_str().as_bytes()).map_err(|_| "isolation path")?;
    // SAFETY: attempting a create that must be refused.
    let fd = unsafe {
        libc::open(
            create.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
    };
    if fd >= 0 {
        // SAFETY: undo the unexpected success, then report it.
        unsafe {
            libc::close(fd);
            libc::unlink(create.as_ptr());
        }
        return fail("a file could be created");
    }
    if !denied_with(libc::EACCES) {
        return fail("file creation was refused for a reason other than the sandbox");
    }
    // Reads outside the allowed set. The file was chosen as readable by
    // ordinary permissions before the restrictions applied.
    let outside =
        CString::new(targets.readable_file.as_os_str().as_bytes()).map_err(|_| "isolation path")?;
    // SAFETY: attempting a read-open that must be refused.
    let fd = unsafe { libc::open(outside.as_ptr(), libc::O_RDONLY) };
    if fd >= 0 {
        // SAFETY: close the unexpected descriptor.
        unsafe { libc::close(fd) };
        return fail("a file outside the allowed set could be read");
    }
    if !denied_with(libc::EACCES) {
        return fail("a read was refused for a reason other than the sandbox");
    }
    // The core itself must not be openable for writing.
    let core_c = CString::new(core_path.as_os_str().as_bytes()).map_err(|_| "isolation path")?;
    // SAFETY: attempting a write-open that must be refused.
    let fd = unsafe { libc::open(core_c.as_ptr(), libc::O_WRONLY) };
    if fd >= 0 {
        // SAFETY: close the unexpected descriptor.
        unsafe { libc::close(fd) };
        return fail("the core file could be opened for writing");
    }
    // Programs and processes.
    let shell = CString::new("/bin/true").unwrap();
    let argv: [*const c_char; 2] = [shell.as_ptr(), std::ptr::null()];
    let envp: [*const c_char; 1] = [std::ptr::null()];
    // SAFETY: exec must be refused; on success the process image would be
    // replaced by /bin/true, which the parent would then see as a missing proof.
    unsafe { libc::execve(shell.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
    if !denied_with(libc::EPERM) {
        return fail("exec was not refused");
    }
    // SAFETY: fork must be refused. If it unexpectedly succeeds the child
    // leaves immediately and the parent reports the failure.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: leave the unexpected child at once.
        unsafe { libc::_exit(0) };
    }
    if pid > 0 {
        return fail("a process could be created");
    }
    // Harmless probes of syscalls the filter refuses before looking at their
    // arguments, so they are refused (EPERM) whether or not the target exists
    // and would change nothing if the sandbox were somehow broken.
    let nowhere = CString::new("/nonexistent-emuwiz-probe").unwrap();
    // SAFETY: a mode change on a path that does not exist.
    if unsafe { libc::chmod(nowhere.as_ptr(), 0) } == 0 || !denied_with(libc::EPERM) {
        return fail("a file's mode could be changed");
    }
    let mut ignored = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: a query-only prlimit on another process; it must be refused.
    if unsafe {
        libc::prlimit(
            libc::getppid(),
            libc::RLIMIT_NOFILE,
            std::ptr::null(),
            &mut ignored,
        )
    } == 0
        || !denied_with(libc::EPERM)
    {
        return fail("another process's limits could be queried or changed");
    }
    // Process-group changes must be refused so the caller's kill reaches us.
    // SAFETY: moving into the parent's group; it must be refused.
    if unsafe { libc::setpgid(0, libc::getpgid(libc::getppid())) } == 0 || !denied_with(libc::EPERM)
    {
        return fail("the process group could be changed");
    }
    // Signals to other processes.
    // SAFETY: signal 0 to the parent only checks permission; it must be refused.
    if unsafe { libc::kill(libc::getppid(), 0) } == 0 || !denied_with(libc::EPERM) {
        return fail("a signal could be sent to another process");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
