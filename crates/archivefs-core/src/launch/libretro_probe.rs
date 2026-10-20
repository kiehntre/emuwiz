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
//!   no-new-privileges, every inherited descriptor except stdio and the result
//!   pipe closed, resource limits, a Landlock filesystem ruleset that grants
//!   read-only access to the core file and the system library folders and
//!   nothing else, and a seccomp filter that refuses sockets, new processes,
//!   exec, signals to other processes, ptrace, mounts and similar;
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
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::process_spawn::{CapturedFileIdentity, open_regular_file_nonblocking};
use crate::patch_manager::is_safe_cheat_name_part;

/// First argument that turns an EmuWiz executable into the probe child.
pub const PROBE_ARGUMENT: &str = "--emuwiz-internal-libretro-core-probe-v1";

const ISOLATED_LINE: &str = "EMUWIZ-PROBE-ISOLATED";
const NAME_PREFIX: &str = "EMUWIZ-PROBE-NAME ";
const FAIL_PREFIX: &str = "EMUWIZ-PROBE-FAIL ";
/// The descriptor the child writes its answer to.
const RESULT_FD: RawFd = 3;
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

/// A core whose library name was read from the core itself, with the file
/// identity it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedCore {
    pub core_path: PathBuf,
    pub identity: CapturedFileIdentity,
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

fn identity_of(path: &Path) -> io::Result<CapturedFileIdentity> {
    Ok(CapturedFileIdentity::capture(
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
    let identity = CapturedFileIdentity::capture(&metadata);
    drop(file);

    let (program, leading, environment) = launcher
        .child()
        .map_err(|e| ProbeError::Spawn(e.to_string()))?;
    let mut pipe = [0 as c_int; 2];
    // SAFETY: `pipe` is a valid two-element array.
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(ProbeError::Spawn(io::Error::last_os_error().to_string()));
    }
    let (read_end, write_end) = (pipe[0], pipe[1]);
    let mut command = Command::new(program);
    command
        .args(leading)
        .arg(core_path)
        .arg(metadata.dev().to_string())
        .arg(metadata.ino().to_string())
        .arg(metadata.len().to_string())
        .arg(metadata.mtime().to_string())
        .arg(metadata.mtime_nsec().to_string())
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    // SAFETY: the closure only calls async-signal-safe `dup2`/`fcntl`.
    unsafe {
        command.pre_exec(move || {
            if write_end == RESULT_FD {
                if libc::fcntl(RESULT_FD, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
            } else if libc::dup2(write_end, RESULT_FD) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let spawned = command.spawn();
    // The parent's copy of the write end must close or the reader never ends.
    // SAFETY: both descriptors were created above and are closed once.
    unsafe { libc::close(write_end) };
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            // SAFETY: as above.
            unsafe { libc::close(read_end) };
            return Err(ProbeError::Spawn(error.to_string()));
        }
    };
    // SAFETY: `read_end` is a fresh pipe descriptor owned by this File.
    let mut reader = unsafe { fs::File::from_raw_fd(read_end) };
    let collector = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut reader)
            .take(MAX_OUTPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes);
        bytes
    });

    let deadline = Instant::now() + WALL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => break None,
            Err(error) => {
                kill_group(&child);
                let _ = child.wait();
                let _ = collector.join();
                return Err(ProbeError::Crashed(error.to_string()));
            }
        }
    };
    let Some(status) = status else {
        kill_group(&child);
        let _ = child.wait();
        let _ = collector.join();
        return Err(ProbeError::Timeout);
    };
    // No group kill here: the child is already reaped, its id could be reused,
    // and fork/clone are refused so it cannot have left anything behind.
    let output = collector.join().unwrap_or_default();
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
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(ProbeError::InvalidAnswer("answer too long".into()));
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

fn kill_group(child: &std::process::Child) {
    // SAFETY: signalling the helper's own process group (`process_group(0)` at
    // spawn), only while the helper has not been reaped; never the caller's group.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
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

fn child_steps(arguments: &[String]) -> Result<String, String> {
    let [path, dev, ino, size, mtime, mtime_nsec] = arguments else {
        return Err("isolation wrong argument count".into());
    };
    let expected = (
        dev.parse::<u64>().map_err(|_| "isolation bad dev")?,
        ino.parse::<u64>().map_err(|_| "isolation bad ino")?,
        size.parse::<u64>().map_err(|_| "isolation bad size")?,
        mtime.parse::<i64>().map_err(|_| "isolation bad mtime")?,
        mtime_nsec
            .parse::<i64>()
            .map_err(|_| "isolation bad mtime_nsec")?,
    );
    // The result pipe must be the inherited descriptor we were promised.
    if !is_open(RESULT_FD) {
        return Err("isolation no result channel".into());
    }

    // 1. No privilege gain, ever.
    // SAFETY: plain prctl.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!("isolation no_new_privs: errno {}", errno()));
    }
    // 2. Nothing inherited survives except stdio and the result pipe.
    close_inherited_descriptors()?;
    // 3. Resource limits.
    apply_limits()?;
    // 4. Open the exact core file we were told about, no-follow.
    let core = open_regular_file_nonblocking(Path::new(path))
        .map_err(|e| format!("identity unreadable: {e}"))?;
    let meta = core.metadata().map_err(|e| format!("identity {e}"))?;
    if (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
    ) != expected
    {
        return Err("identity changed before probe".into());
    }
    let core_fd = core.as_raw_fd();
    // 5. Filesystem: read-only core file and system libraries only.
    landlock_restrict(core_fd)?;
    // 6. Syscalls: no sockets, processes, exec, foreign signals, ...
    seccomp_restrict()?;
    // 7. Prove all of it by trying to break it.
    prove_isolation(Path::new(path), core_fd)?;
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
        (libc::RLIMIT_FSIZE, 0),
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

fn prove_isolation(core_path: &Path, core_fd: RawFd) -> Result<(), String> {
    let fail = |what: &str| Err(format!("isolation unproven: {what}"));
    // Privileges.
    // SAFETY: plain prctl read.
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 1 {
        return fail("no_new_privs not set");
    }
    // Limits read back.
    for (resource, expected) in [
        (libc::RLIMIT_CORE, 0u64),
        (libc::RLIMIT_CPU, CPU_SECONDS),
        (libc::RLIMIT_AS, ADDRESS_SPACE_BYTES),
        (libc::RLIMIT_NOFILE, OPEN_FILES),
        (libc::RLIMIT_FSIZE, 0),
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
    // Descriptors: only stdio, the result pipe and the core file may be open.
    for fd in 3..(OPEN_FILES as RawFd) {
        let must_be_open = fd == RESULT_FD || fd == core_fd;
        if is_open(fd) != must_be_open {
            return fail("an unexpected descriptor is open or a required one is closed");
        }
    }
    // Network: real attempts must fail with EPERM.
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
        if errno() != libc::EPERM {
            return fail("socket creation was not refused by the sandbox");
        }
    }
    // Writes: creating a file beside the core and opening the core for writing
    // must both be denied.
    if let Some(directory) = core_path.parent() {
        let probe = directory.join(".emuwiz-probe-write-test");
        let c = CString::new(probe.as_os_str().as_bytes()).map_err(|_| "isolation path")?;
        // SAFETY: attempting a create that must fail.
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        if fd >= 0 {
            // SAFETY: undo the unexpected success, then report it.
            unsafe {
                libc::close(fd);
                libc::unlink(c.as_ptr());
            }
            return fail("a file could be created");
        }
    }
    let core_c = CString::new(core_path.as_os_str().as_bytes()).map_err(|_| "isolation path")?;
    // SAFETY: attempting a write-open that must fail.
    let fd = unsafe { libc::open(core_c.as_ptr(), libc::O_WRONLY) };
    if fd >= 0 {
        // SAFETY: close the unexpected descriptor.
        unsafe { libc::close(fd) };
        return fail("the core file could be opened for writing");
    }
    // Reads outside the allowed set must be denied.
    let outside = CString::new("/etc/passwd").unwrap();
    // SAFETY: attempting a read-open that must fail.
    let fd = unsafe { libc::open(outside.as_ptr(), libc::O_RDONLY) };
    if fd >= 0 {
        // SAFETY: close the unexpected descriptor.
        unsafe { libc::close(fd) };
        return fail("a file outside the allowed set could be read");
    }
    // Programs and processes.
    let shell = CString::new("/bin/true").unwrap();
    let argv: [*const c_char; 2] = [shell.as_ptr(), std::ptr::null()];
    let envp: [*const c_char; 1] = [std::ptr::null()];
    // SAFETY: exec must be refused; on success the process image would be
    // replaced by /bin/true, which the parent would then see as a missing proof.
    unsafe { libc::execve(shell.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
    if errno() != libc::EPERM {
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
    // File metadata (not governed by the filesystem rules).
    // SAFETY: attempting a mode change that must be refused.
    if unsafe { libc::chmod(core_c.as_ptr(), 0o777) } == 0 {
        return fail("a file's mode could be changed");
    }
    // Limits on other processes of the same user.
    let tiny = libc::rlimit {
        rlim_cur: 1,
        rlim_max: 1,
    };
    // SAFETY: targeting the parent with a limit that must be refused.
    if unsafe {
        libc::prlimit(
            libc::getppid(),
            libc::RLIMIT_NOFILE,
            &tiny,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return fail("another process's limits could be changed");
    }
    // Signals to other processes.
    // SAFETY: signal 0 to the parent only checks permission; it must be refused.
    let sent = unsafe { libc::kill(libc::getppid(), 0) };
    if sent == 0 {
        return fail("a signal could be sent to another process");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
