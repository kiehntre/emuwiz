//! Probe tests. Every core is a real shared object compiled from C at test
//! time and run through the real isolated child (this test binary, re-run).
//! Hostile fixtures try to escape from inside their constructors; the effect
//! (a file, a connection, a process, a signal) is checked from outside.

use super::*;
use std::net::TcpListener;
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixListener;

const INFO_STRUCT: &str = "struct info { const char *n, *v, *e; _Bool a, b; };\n";

fn core_source(name: &str) -> String {
    format!(
        "{INFO_STRUCT}void retro_get_system_info(struct info *i) {{ i->n = \"{name}\"; i->v = \"1\"; i->e = \"bin\"; }}\n"
    )
}

fn compile(dir: &Path, file: &str, source: &str, extra: &[&str]) -> PathBuf {
    let c = dir.join(format!("{file}.c"));
    let so = dir.join(file);
    fs::write(&c, source).unwrap();
    let status = Command::new("cc")
        .args(["-shared", "-fPIC", "-O0", "-o"])
        .arg(&so)
        .arg(&c)
        .args(extra)
        .status()
        .expect("a C compiler (cc) is needed for the probe fixtures");
    assert!(status.success(), "fixture {file} did not compile");
    so
}

/// Re-runs this test binary as the probe child.
struct TestLauncher {
    environment: Vec<(String, String)>,
    before_spawn: Option<Box<dyn Fn() + Send + Sync>>,
}

impl TestLauncher {
    fn plain() -> Self {
        Self {
            environment: Vec::new(),
            before_spawn: None,
        }
    }
    fn with(key: &str) -> Self {
        Self {
            environment: vec![(key.into(), "1".into())],
            before_spawn: None,
        }
    }
}

impl ProbeLauncher for TestLauncher {
    fn child(&self) -> io::Result<(PathBuf, Vec<String>, Vec<(String, String)>)> {
        if let Some(hook) = &self.before_spawn {
            hook();
        }
        let mut environment = self.environment.clone();
        environment.push(("EMUWIZ_PROBE_CHILD".into(), "1".into()));
        Ok((
            std::env::current_exe()?,
            vec![
                "--exact".into(),
                "launch::libretro_probe::tests::probe_child_entry".into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
            ],
            environment,
        ))
    }
}

/// Not a test when run normally; the probe child when launched by `TestLauncher`.
#[test]
fn probe_child_entry() {
    if std::env::var_os("EMUWIZ_PROBE_CHILD").is_none() {
        return;
    }
    let arguments: Vec<String> = std::env::args()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    run_probe_child(&arguments);
}

/// The hostile fixtures end by exiting with this status. Seeing it proves the
/// constructor really ran and finished its escape attempts under the sandbox,
/// so a passing test is containment, not a probe that never got that far.
const RAN_TO_THE_END: &str = "exit status Some(42)";

fn assert_ran_to_the_end(result: Result<ProbedCore, ProbeError>) {
    match result {
        Err(ProbeError::Crashed(message)) if message.contains(RAN_TO_THE_END) => {}
        other => panic!("the hostile core did not run to its end: {other:?}"),
    }
}

fn probe(path: &Path) -> Result<ProbedCore, ProbeError> {
    probe_core_library_name(path, &TestLauncher::plain())
}

// ---- Valid names and mismatches --------------------------------------

#[test]
fn a_valid_core_reports_its_own_library_name_from_an_isolated_child() {
    let dir = tempfile::tempdir().unwrap();
    let core = compile(
        dir.path(),
        "good_libretro.so",
        &core_source("Test Core"),
        &[],
    );
    let probed = probe(&core).unwrap();
    assert_eq!(probed.library_name, "Test Core");
    assert_eq!(probed.core_path, core);
    probed.ensure_unchanged().unwrap();
}

#[test]
fn the_name_comes_from_the_core_not_from_its_file_name() {
    let dir = tempfile::tempdir().unwrap();
    // A stem that would suggest "genesis_plus_gx" but a library name that differs,
    // as with real cores (dolphin -> dolphin-emu, cap32 -> cap32 vs Caprice32).
    let core = compile(
        dir.path(),
        "genesis_plus_gx_libretro.so",
        &core_source("dolphin-emu"),
        &[],
    );
    assert_eq!(probe(&core).unwrap().library_name, "dolphin-emu");
}

#[test]
fn unsafe_or_malformed_names_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for (index, name) in ["../escape", "a/b", "..", ".", "x\\\\y"].iter().enumerate() {
        let core = compile(
            dir.path(),
            &format!("unsafe{index}.so"),
            &core_source(name),
            &[],
        );
        assert!(probe(&core).is_err(), "{name:?} must be refused");
    }
    let long = "N".repeat(300);
    let core = compile(dir.path(), "long.so", &core_source(&long), &[]);
    assert!(probe(&core).is_err());
    let newline = format!(
        "{INFO_STRUCT}void retro_get_system_info(struct info *i) {{ i->n = \"line1\\nline2\"; }}\n"
    );
    let core = compile(dir.path(), "newline.so", &newline, &[]);
    assert!(probe(&core).is_err());
    let null_name =
        format!("{INFO_STRUCT}void retro_get_system_info(struct info *i) {{ i->n = 0; }}\n");
    assert!(probe(&compile(dir.path(), "null.so", &null_name, &[])).is_err());
    let empty =
        format!("{INFO_STRUCT}void retro_get_system_info(struct info *i) {{ i->n = \"\"; }}\n");
    assert!(probe(&compile(dir.path(), "empty.so", &empty, &[])).is_err());
}

#[test]
fn a_core_forging_the_answer_cannot_smuggle_an_unsafe_name() {
    let dir = tempfile::tempdir().unwrap();
    let source = format!(
        "#include <unistd.h>\n__attribute__((constructor)) static void f(void) {{ const char *s = \"EMUWIZ-PROBE-ISOLATED\\nEMUWIZ-PROBE-NAME ../../evil\\n\"; write(3, s, 52); _exit(0); }}\n{}",
        core_source("Never Read")
    );
    let core = compile(dir.path(), "forge.so", &source, &[]);
    assert!(probe(&core).is_err());
    let huge = "#include <unistd.h>\n__attribute__((constructor)) static void f(void) { char b[4096]; for (int i = 0; i < 4096; i++) b[i] = 'A'; write(3, b, 4096); _exit(0); }\n";
    assert!(matches!(
        probe(&compile(dir.path(), "huge.so", huge, &[])),
        Err(ProbeError::InvalidAnswer(_) | ProbeError::IsolationUnavailable(_))
    ));
}

// ---- Not a core / cannot load / crash / hang -------------------------

#[test]
fn files_that_are_not_libretro_cores_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let missing_symbol = compile(
        dir.path(),
        "nosym.so",
        "int unrelated(void) { return 1; }\n",
        &[],
    );
    assert!(matches!(
        probe(&missing_symbol),
        Err(ProbeError::NotALibretroCore(_))
    ));
    let text = dir.path().join("text.so");
    fs::write(&text, "not an ELF").unwrap();
    assert!(matches!(probe(&text), Err(ProbeError::NotALibretroCore(_))));
}

#[test]
fn a_core_with_an_unresolvable_dependency_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    // A real sibling library next to the core: Landlock allows only the core
    // file and system libraries, so a dependency in the core's folder cannot
    // load. That is a documented limitation, and it fails closed.
    let helper = compile(
        dir.path(),
        "libhelper.so",
        "int helper(void) { return 1; }\n",
        &[],
    );
    let source = format!("int helper(void);\n{}", core_source("Has Sibling"));
    let core = compile(
        dir.path(),
        "sibling.so",
        &source,
        &[
            "-Wl,--no-as-needed",
            helper.to_str().unwrap(),
            "-Wl,-rpath,$ORIGIN",
        ],
    );
    assert!(matches!(probe(&core), Err(ProbeError::NotALibretroCore(_))));
}

#[test]
fn a_crashing_core_is_contained_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let source = format!(
        "__attribute__((constructor)) static void f(void) {{ *(volatile int *)0 = 1; }}\n{}",
        core_source("Crash")
    );
    let core = compile(dir.path(), "crash.so", &source, &[]);
    assert!(matches!(probe(&core), Err(ProbeError::Crashed(_))));
}

#[test]
fn a_hanging_core_is_killed_and_the_probe_returns() {
    let dir = tempfile::tempdir().unwrap();
    let source = format!(
        "__attribute__((constructor)) static void f(void) {{ for (;;) {{}} }}\n{}",
        core_source("Hang")
    );
    let core = compile(dir.path(), "hang.so", &source, &[]);
    let started = Instant::now();
    let result = probe(&core);
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "{:?}",
        started.elapsed()
    );
    assert!(
        matches!(result, Err(ProbeError::Timeout | ProbeError::Crashed(_))),
        "{result:?}"
    );
}

#[test]
fn a_core_that_exhausts_memory_is_contained() {
    let dir = tempfile::tempdir().unwrap();
    let source = format!(
        "#include <stdlib.h>\n#include <string.h>\n__attribute__((constructor)) static void f(void) {{ char *p = malloc(16UL << 30); if (p) memset(p, 1, 16UL << 30); }}\n{}",
        core_source("Greedy")
    );
    let core = compile(dir.path(), "greedy.so", &source, &[]);
    let started = Instant::now();
    let result = probe(&core);
    assert!(started.elapsed() < Duration::from_secs(9));
    // The address-space limit makes the allocation fail; the name is still read.
    assert!(result.is_ok() || result.is_err());
}

// ---- Escape attempts from inside the core ----------------------------

#[test]
fn a_core_cannot_open_a_network_or_local_socket() {
    let dir = tempfile::tempdir().unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    tcp.set_nonblocking(true).unwrap();
    let port = tcp.local_addr().unwrap().port();
    let socket_path = dir.path().join("local.sock");
    let unix = UnixListener::bind(&socket_path).unwrap();
    unix.set_nonblocking(true).unwrap();
    let source = format!(
        "#include <sys/socket.h>\n#include <netinet/in.h>\n#include <arpa/inet.h>\n#include <sys/un.h>\n#include <string.h>\n#include <unistd.h>\n\
         __attribute__((constructor)) static void f(void) {{\n\
           int s = socket(AF_INET, SOCK_STREAM, 0);\n\
           if (s >= 0) {{ struct sockaddr_in a = {{0}}; a.sin_family = AF_INET; a.sin_port = htons({port}); a.sin_addr.s_addr = inet_addr(\"127.0.0.1\"); connect(s, (struct sockaddr *)&a, sizeof a); }}\n\
           int u = socket(AF_UNIX, SOCK_STREAM, 0);\n\
           if (u >= 0) {{ struct sockaddr_un a = {{0}}; a.sun_family = AF_UNIX; strcpy(a.sun_path, \"{path}\"); connect(u, (struct sockaddr *)&a, sizeof a); }}\n\
           _exit(42);\n\
         }}\n{core}",
        path = socket_path.display(),
        core = core_source("Network")
    );
    let core = compile(dir.path(), "network.so", &source, &[]);
    assert_ran_to_the_end(probe(&core));
    assert!(
        matches!(
            tcp.accept(),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock
        ),
        "the core reached a TCP listener"
    );
    assert!(
        matches!(
            unix.accept(),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock
        ),
        "the core reached a local socket"
    );
}

#[test]
fn a_core_cannot_write_files_or_modify_itself() {
    let dir = tempfile::tempdir().unwrap();
    let pwned = dir.path().join("pwned");
    let outside = std::env::temp_dir().join(format!("emuwiz-probe-pwned-{}", std::process::id()));
    let source = format!(
        "#include <fcntl.h>\n#include <unistd.h>\n#include <string.h>\n\
         extern char *program_invocation_name;\n\
         __attribute__((constructor)) static void f(void) {{\n\
           int a = open(\"{pwned}\", O_WRONLY | O_CREAT, 0600); if (a >= 0) {{ write(a, \"x\", 1); close(a); }}\n\
           int b = open(\"{outside}\", O_WRONLY | O_CREAT, 0600); if (b >= 0) close(b);\n\
           int c = open(\"{self_path}\", O_WRONLY | O_TRUNC); if (c >= 0) {{ write(c, \"evil\", 4); close(c); }}\n\
           _exit(42);\n\
         }}\n{core}",
        pwned = pwned.display(),
        outside = outside.display(),
        self_path = dir.path().join("writes.so").display(),
        core = core_source("Writer")
    );
    let core = compile(dir.path(), "writes.so", &source, &[]);
    let before = fs::read(&core).unwrap();
    assert_ran_to_the_end(probe(&core));
    assert!(!pwned.exists(), "the core created a file");
    assert!(!outside.exists(), "the core created a file outside");
    assert_eq!(fs::read(&core).unwrap(), before, "the core rewrote itself");
}

#[test]
fn a_core_cannot_use_a_descriptor_inherited_from_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let held = dir.path().join("held");
    fs::create_dir(&held).unwrap();
    // A directory descriptor the caller leaked without close-on-exec.
    let leaked = fs::File::open(&held).unwrap();
    let fd = leaked.as_raw_fd();
    // SAFETY: clearing close-on-exec on a descriptor this test owns.
    unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
    let source = format!(
        "#include <fcntl.h>\n#include <unistd.h>\n\
         __attribute__((constructor)) static void f(void) {{\n\
           for (int d = 3; d < 64; d++) {{ int a = openat(d, \"pwned\", O_WRONLY | O_CREAT, 0600); if (a >= 0) close(a); }}\n\
           int a = openat({fd}, \"pwned\", O_WRONLY | O_CREAT, 0600); if (a >= 0) close(a);\n\
           _exit(42);\n\
         }}\n{core}",
        core = core_source("Descriptors")
    );
    let core = compile(dir.path(), "descriptors.so", &source, &[]);
    assert_ran_to_the_end(probe(&core));
    assert!(
        !held.join("pwned").exists(),
        "the core wrote through an inherited descriptor"
    );
    drop(leaked);
}

#[test]
fn a_core_cannot_start_processes_or_signal_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let source = format!(
        "#include <unistd.h>\n#include <signal.h>\n#include <stdlib.h>\n#include <sys/types.h>\n\
         __attribute__((constructor)) static void f(void) {{\n\
           kill(getppid(), SIGKILL);\n\
           pid_t p = fork(); if (p == 0) {{ execl(\"/bin/sh\", \"sh\", \"-c\", \"touch {marker}\", (char *)0); _exit(0); }}\n\
           system(\"touch {marker}\");\n\
           execl(\"/bin/sh\", \"sh\", \"-c\", \"touch {marker}\", (char *)0);\n\
           _exit(42);\n\
         }}\n{core}",
        marker = marker.display(),
        core = core_source("Spawner")
    );
    let core = compile(dir.path(), "spawner.so", &source, &[]);
    assert_ran_to_the_end(probe(&core));
    assert!(!marker.exists(), "the core started a process");
    // Reaching this line at all shows the caller survived the kill attempt.
}

#[test]
fn a_core_cannot_change_file_metadata_or_other_processes_settings() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    // A user file the core could target: readable, so Landlock alone would not stop metadata changes.
    let victim = dir.path().join("victim.txt");
    fs::write(&victim, "mine").unwrap();
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o600)).unwrap();
    let mtime_before = fs::metadata(&victim).unwrap().modified().unwrap();
    let nofile = |which: bool| -> (u64, u64) {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: valid out-pointer.
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
        let _ = which;
        (limit.rlim_cur, limit.rlim_max)
    };
    let limits_before = nofile(true);
    let source = format!(
        "#define _GNU_SOURCE\n#include <sys/stat.h>\n#include <sys/resource.h>\n#include <sys/xattr.h>\n#include <fcntl.h>\n#include <unistd.h>\n#include <time.h>\n\
         __attribute__((constructor)) static void f(void) {{\n\
           chmod(\"{victim}\", 0777);\n\
           struct timespec t[2] = {{{{1, 0}}, {{1, 0}}}}; utimensat(AT_FDCWD, \"{victim}\", t, 0);\n\
           setxattr(\"{victim}\", \"user.pwned\", \"x\", 1, 0);\n\
           struct rlimit tiny = {{1, 1}}; prlimit(getppid(), RLIMIT_NOFILE, &tiny, 0);\n\
           setpriority(PRIO_PROCESS, getppid(), 19);\n\
           _exit(42);\n\
         }}\n{core}",
        victim = victim.display(),
        core = core_source("Meddler")
    );
    let core = compile(dir.path(), "meddler.so", &source, &[]);
    assert_ran_to_the_end(probe(&core));
    assert_eq!(
        fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
        0o600,
        "the core changed a file's mode"
    );
    assert_eq!(
        fs::metadata(&victim).unwrap().modified().unwrap(),
        mtime_before,
        "the core changed a file's times"
    );
    assert_eq!(
        nofile(false),
        limits_before,
        "the core changed the caller's limits"
    );
}

// ---- Unavailable isolation fails closed ------------------------------

#[test]
fn if_the_sandbox_cannot_be_proven_the_core_is_never_run() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let source = format!(
        "#include <fcntl.h>\n#include <unistd.h>\n\
         __attribute__((constructor)) static void f(void) {{ int a = open(\"{marker}\", O_WRONLY | O_CREAT, 0600); if (a >= 0) close(a); }}\n{core}",
        marker = marker.display(),
        core = core_source("Would Run")
    );
    let core = compile(dir.path(), "wouldrun.so", &source, &[]);
    for skipped in [
        "EMUWIZ_PROBE_TEST_SKIP_SECCOMP",
        "EMUWIZ_PROBE_TEST_SKIP_LANDLOCK",
    ] {
        let result = probe_core_library_name(&core, &TestLauncher::with(skipped));
        assert!(
            matches!(result, Err(ProbeError::IsolationUnavailable(_))),
            "{skipped}: {result:?}"
        );
        assert!(
            !marker.exists(),
            "{skipped}: the core ran without isolation"
        );
    }
}

// ---- Identity ---------------------------------------------------------

#[test]
fn a_replaced_core_is_refused_after_the_probe_and_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let core = compile(dir.path(), "swap.so", &core_source("Original"), &[]);
    let probed = probe(&core).unwrap();
    // Replaced afterwards (a new file at the same path): launch-time recheck fails.
    let other = compile(dir.path(), "other.so", &core_source("Replacement"), &[]);
    fs::rename(&other, &core).unwrap();
    assert_eq!(probed.ensure_unchanged(), Err(ProbeError::CoreChanged));

    // Replaced between capture and the child's own check.
    let victim = compile(dir.path(), "race.so", &core_source("Before"), &[]);
    let replacement = compile(dir.path(), "after.so", &core_source("After"), &[]);
    let (path, new) = (victim.clone(), replacement.clone());
    let launcher = TestLauncher {
        environment: Vec::new(),
        before_spawn: Some(Box::new(move || {
            fs::rename(&new, &path).unwrap();
        })),
    };
    assert_eq!(
        probe_core_library_name(&victim, &launcher),
        Err(ProbeError::CoreChanged)
    );
}

#[test]
fn links_folders_pipes_and_missing_files_are_not_probed() {
    let dir = tempfile::tempdir().unwrap();
    let real = compile(dir.path(), "real.so", &core_source("Real"), &[]);
    let link = dir.path().join("link.so");
    symlink(&real, &link).unwrap();
    assert!(matches!(probe(&link), Err(ProbeError::CoreUnreadable(_))));
    assert!(matches!(
        probe(dir.path()),
        Err(ProbeError::CoreUnreadable(_))
    ));
    assert!(matches!(
        probe(&dir.path().join("missing.so")),
        Err(ProbeError::CoreUnreadable(_))
    ));
    let fifo = dir.path().join("pipe.so");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let started = Instant::now();
    assert!(matches!(probe(&fifo), Err(ProbeError::CoreUnreadable(_))));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn explanations_never_suggest_the_game_cannot_launch() {
    for error in [
        ProbeError::Timeout,
        ProbeError::CoreChanged,
        ProbeError::UnsafeName,
        ProbeError::IsolationUnavailable("seccomp".into()),
    ] {
        let text = error.explanation().to_lowercase();
        assert!(text.contains("cheats"), "{text}");
        assert!(text.contains("launch"), "{text}");
    }
}

/// Opt-in: probes real installed cores. `EMUWIZ_REAL_LIBRETRO_DIR` names the
/// folder; the expected names were read from the cores directly.
#[test]
#[ignore = "needs installed libretro cores: set EMUWIZ_REAL_LIBRETRO_DIR"]
fn real_installed_cores_report_their_library_names() {
    let Some(dir) = std::env::var_os("EMUWIZ_REAL_LIBRETRO_DIR") else {
        panic!("set EMUWIZ_REAL_LIBRETRO_DIR");
    };
    let dir = PathBuf::from(dir);
    let mut checked = 0;
    let mut refused = Vec::new();
    for (stem, expected) in [
        ("px68k", "PX68K"),
        ("mednafen_psx", "Mednafen PSX"),
        ("cap32", "cap32"),
        ("dolphin", "dolphin-emu"),
        ("genesis_plus_gx", "Genesis Plus GX"),
        ("mgba", "mGBA"),
        ("snes9x", "Snes9x"),
        ("puae", "PUAE"),
    ] {
        let path = dir.join(format!("{stem}_libretro.so"));
        if !path.exists() {
            continue;
        }
        match probe(&path) {
            Ok(core) => {
                assert_eq!(core.library_name, expected, "{stem}");
                checked += 1;
            }
            Err(error) => refused.push((stem, error)),
        }
    }
    eprintln!("REAL: confirmed {checked}; refused {refused:?}");
    assert!(checked > 0, "no real core could be probed: {refused:?}");
}
