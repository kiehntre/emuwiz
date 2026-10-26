//! Tests for native Dolphin launch preflight/execution.
//!
//! Every fixture is a real temp directory on disk (a fake `dolphin-emu`
//! shell script, a real `Dolphin.ini`, a synthetic but structurally valid
//! GameCube ISO header) driven through the exact same real discovery/
//! identity/planning functions production code uses -
//! `discover_dolphin_local_profiles`, `resolve_dolphin_native_launch_binding`,
//! `inspect_catalogued_game_identity`, `build_launch_plan`,
//! `build_dolphin_command_plan` - never a shortcut or a mocked plan. Spawn
//! tests genuinely `fork`/`exec` the fake script; no real installed Dolphin
//! is required anywhere.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::{DolphinLocalDiscoveryRoots, DolphinUserDirectoryMode};

#[cfg(target_os = "linux")]
mod graceful_stop {
    use super::*;
    use crate::launch::process_spawn::{
        GracefulStopOutcome, GracefulStopRefusal, GracefulStopScope, PreparedProcessCommand,
        spawn_watched_process_isolated,
    };
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    // All processes are disposable interpreters of this private fixture.
    // Release files provide cooperative test cleanup, including orphans;
    // no test signals a guessed PID, an installed emulator, or a user process.
    const PROBE: &str = r#"#!/usr/bin/python3
import json, os, pathlib, signal, subprocess, sys, time
mode, directory = sys.argv[1:3]
root = pathlib.Path(directory)
running = True
terms = 0
def term(signum, frame):
    global running, terms
    terms += 1
    (root / (mode + '.terms')).write_text(str(terms))
    sys.stderr.write('graceful TERM\n')
    sys.stderr.flush()
    if mode != 'ignore':
        running = False
signal.signal(signal.SIGTERM, term)
if mode == 'stdio':
    os.fstat(0)
    os.fstat(1)
if mode == 'separate-group':
    os.setsid()
child = None
if mode in ('tree', 'branch', 'orphan-wrapper'):
    child_mode = 'branch' if mode == 'tree' else 'leaf'
    child = subprocess.Popen([sys.executable, __file__, child_mode, str(root)],
                             stderr=subprocess.DEVNULL if mode == 'orphan-wrapper' else None)
sys.stderr.write('synthetic diagnostic\n')
sys.stderr.flush()
(root / (mode + '.ready')).write_text(json.dumps({'pid': os.getpid(), 'pgrp': os.getpgrp()}))
if mode == 'orphan-wrapper':
    (root / (mode + '.done')).write_text('done')
    sys.exit(0)
heartbeat = 0
while running and not (root / 'release').exists():
    heartbeat += 1
    (root / (mode + '.heartbeat')).write_text(str(heartbeat))
    time.sleep(0.01)
# A late stop may arrive after cooperative release, including while Python
# resets handlers during interpreter shutdown. Keep TERM blocked throughout
# teardown so this successful fixture exit stays successful.
signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGTERM})
if child is not None:
    child.wait(timeout=5)
(root / (mode + '.done')).write_text('done')
if mode == 'exit-race':
    while not (root / 'finish').exists():
        time.sleep(0.01)
    assert terms == 0, 'TERM was delivered during fixture teardown'
"#;

    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "synthetic process did not reach checkpoint"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    struct SyntheticProcess {
        fixture: Fixture,
        mode: &'static str,
        watched: Option<WatchedProcess>,
        exit_callbacks: Arc<AtomicUsize>,
        start_callbacks: Arc<AtomicUsize>,
    }

    impl SyntheticProcess {
        fn new(mode: &'static str) -> Self {
            let fixture = Fixture::new("graceful-stop");
            let executable = fixture.write_executable("probe", PROBE.as_bytes());
            let command = PreparedProcessCommand {
                executable,
                arguments: vec![mode.into(), fixture.root.clone().into_os_string()],
                working_directory: None,
            };
            let lease = fs::File::create(fixture.path("lease")).unwrap();
            let exit_callbacks = Arc::new(AtomicUsize::new(0));
            let start_callbacks = Arc::new(AtomicUsize::new(0));
            let exit_count = Arc::clone(&exit_callbacks);
            let start_count = Arc::clone(&start_callbacks);
            let watched = spawn_watched_process_isolated(
                &command,
                &[],
                &lease,
                move |_| {
                    start_count.fetch_add(1, Ordering::SeqCst);
                },
                move |_| {
                    exit_count.fetch_add(1, Ordering::SeqCst);
                },
            )
            .unwrap();
            let result = Self {
                fixture,
                mode,
                watched: Some(watched),
                exit_callbacks,
                start_callbacks,
            };
            until(|| {
                fs::read(result.fixture.path(&format!("{mode}.ready")))
                    .ok()
                    .is_some_and(|bytes| {
                        serde_json::from_slice::<serde_json::Value>(&bytes).is_ok()
                    })
            });
            result
        }

        fn stop(&mut self, scope: GracefulStopScope, duration: Duration) -> GracefulStopOutcome {
            self.watched
                .as_mut()
                .unwrap()
                .request_graceful_stop(scope, duration)
        }

        fn exit_report(&mut self) -> &ProcessExitReport {
            let watched = self.watched.as_mut().unwrap();
            until(|| watched.poll().is_some());
            watched.poll().unwrap()
        }

        fn release(&self) {
            fs::write(self.fixture.path("release"), b"release").unwrap();
        }

        fn heartbeat(&self, mode: &str) -> u64 {
            fs::read_to_string(self.fixture.path(&format!("{mode}.heartbeat")))
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0)
        }

        fn assert_alive(&self, mode: &str) {
            let before = self.heartbeat(mode);
            until(|| self.heartbeat(mode) > before);
            assert!(!self.fixture.path(&format!("{mode}.done")).exists());
            assert!(!self.fixture.path(&format!("{mode}.terms")).exists());
        }
    }

    impl Drop for SyntheticProcess {
        fn drop(&mut self) {
            self.release();
            fs::write(self.fixture.path("finish"), b"finish").unwrap();
            // Wait for the existing watcher/callback, even if its public
            // handle was dropped. Private descendants exit on the same file.
            until(|| self.exit_callbacks.load(Ordering::SeqCst) == 1);
            if self.mode == "orphan-wrapper" {
                until(|| self.fixture.path("leaf.done").exists());
            }
        }
    }

    #[test]
    fn process_spawn_single_owned_child_stops_gracefully_and_reports_stderr() {
        let mut process = SyntheticProcess::new("single");
        assert_eq!(
            process.stop(GracefulStopScope::LaunchedChild, Duration::from_secs(3)),
            GracefulStopOutcome::ExitedAfterRequest
        );
        let report = process.exit_report();
        assert!(report.status.as_ref().unwrap().success());
        assert_eq!(report.stderr, b"synthetic diagnostic\ngraceful TERM\n");
        assert_eq!(process.start_callbacks.load(Ordering::SeqCst), 1);
        assert_eq!(process.exit_callbacks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn process_spawn_owned_child_and_grandchild_tree_request_refuses_without_signalling() {
        let mut process = SyntheticProcess::new("tree");
        until(|| process.fixture.path("leaf.ready").exists());
        assert_eq!(
            process.stop(GracefulStopScope::ProcessTree, Duration::ZERO),
            GracefulStopOutcome::Refused(GracefulStopRefusal::ProcessTreeNotSupervised)
        );
        for mode in ["tree", "branch", "leaf"] {
            process.assert_alive(mode);
        }
    }

    #[test]
    fn process_spawn_exited_wrapper_does_not_claim_or_stop_its_orphaned_child() {
        let mut process = SyntheticProcess::new("orphan-wrapper");
        until(|| process.fixture.path("leaf.ready").exists());
        assert!(process.exit_report().status.as_ref().unwrap().success());
        assert_eq!(
            process.stop(GracefulStopScope::ProcessTree, Duration::ZERO),
            GracefulStopOutcome::Refused(GracefulStopRefusal::ProcessTreeNotSupervised)
        );
        assert_eq!(
            process.stop(GracefulStopScope::LaunchedChild, Duration::ZERO),
            GracefulStopOutcome::AlreadyExited
        );
        process.assert_alive("leaf");
    }

    #[test]
    fn process_spawn_reused_diagnostic_pid_cannot_redirect_the_launch_descriptor() {
        let mut owned = SyntheticProcess::new("single");
        let unrelated = SyntheticProcess::new("separate-group");
        // Simulate a stale/public PID now naming another live process. No
        // global kernel PID counter is changed to force actual numeric reuse.
        owned.watched.as_mut().unwrap().pid = unrelated.watched.as_ref().unwrap().pid;
        assert_eq!(
            owned.stop(GracefulStopScope::LaunchedChild, Duration::from_secs(3)),
            GracefulStopOutcome::ExitedAfterRequest
        );
        unrelated.assert_alive("separate-group");
    }

    #[test]
    fn process_spawn_exited_child_with_reused_pid_does_not_signal_the_replacement() {
        let mut owned = SyntheticProcess::new("single");
        owned.release();
        assert!(owned.exit_report().status.as_ref().unwrap().success());
        let unrelated = SyntheticProcess::new("separate-group");
        owned.watched.as_mut().unwrap().pid = unrelated.watched.as_ref().unwrap().pid;
        assert_eq!(
            owned.stop(GracefulStopScope::LaunchedChild, Duration::ZERO),
            GracefulStopOutcome::AlreadyExited
        );
        unrelated.assert_alive("separate-group");
    }

    #[test]
    fn process_spawn_process_group_reuse_never_authorizes_a_tree_signal() {
        let mut owned = SyntheticProcess::new("single");
        owned.release();
        owned.exit_report();
        let unrelated = SyntheticProcess::new("separate-group");
        let facts: serde_json::Value = serde_json::from_slice(
            &fs::read(unrelated.fixture.path("separate-group.ready")).unwrap(),
        )
        .unwrap();
        assert_eq!(facts["pid"], facts["pgrp"]);
        owned.watched.as_mut().unwrap().pid = unrelated.watched.as_ref().unwrap().pid;
        assert_eq!(
            owned.stop(GracefulStopScope::ProcessTree, Duration::ZERO),
            GracefulStopOutcome::Refused(GracefulStopRefusal::ProcessTreeNotSupervised)
        );
        unrelated.assert_alive("separate-group");
    }

    #[test]
    fn process_spawn_already_exited_child_is_a_truthful_no_op() {
        let mut process = SyntheticProcess::new("single");
        process.release();
        process.exit_report();
        assert_eq!(
            process.stop(GracefulStopScope::LaunchedChild, Duration::ZERO),
            GracefulStopOutcome::AlreadyExited
        );
        assert!(!process.fixture.path("single.terms").exists());
    }

    #[test]
    fn process_spawn_repeated_stop_requests_send_exactly_one_term_without_escalation() {
        let mut process = SyntheticProcess::new("ignore");
        assert_eq!(
            process.stop(GracefulStopScope::LaunchedChild, Duration::from_millis(50)),
            GracefulStopOutcome::StillRunning {
                request_already_pending: false
            }
        );
        until(|| process.fixture.path("ignore.terms").exists());
        assert_eq!(
            process.stop(GracefulStopScope::LaunchedChild, Duration::from_millis(50)),
            GracefulStopOutcome::StillRunning {
                request_already_pending: true
            }
        );
        assert_eq!(
            fs::read_to_string(process.fixture.path("ignore.terms")).unwrap(),
            "1"
        );
        assert!(process.watched.as_mut().unwrap().poll().is_none());
        process.release();
        assert!(process.exit_report().status.as_ref().unwrap().success());
    }

    #[test]
    fn process_spawn_unrelated_process_in_another_group_survives_a_stop() {
        let mut owned = SyntheticProcess::new("single");
        let unrelated = SyntheticProcess::new("separate-group");
        assert_eq!(
            owned.stop(GracefulStopScope::LaunchedChild, Duration::from_secs(3)),
            GracefulStopOutcome::ExitedAfterRequest
        );
        unrelated.assert_alive("separate-group");
    }

    #[test]
    fn process_spawn_exit_concurrent_with_stop_has_no_unowned_fallback() {
        let mut owned = SyntheticProcess::new("exit-race");
        let unrelated = SyntheticProcess::new("separate-group");
        owned.release();
        until(|| owned.fixture.path("exit-race.done").exists());
        // Release from a known teardown checkpoint, then race exit with stop.
        fs::write(owned.fixture.path("finish"), b"finish").unwrap();
        let outcome = owned.stop(GracefulStopScope::LaunchedChild, Duration::from_secs(3));
        assert!(matches!(
            outcome,
            GracefulStopOutcome::AlreadyExited | GracefulStopOutcome::ExitedAfterRequest
        ));
        assert!(owned.exit_report().status.as_ref().unwrap().success());
        unrelated.assert_alive("separate-group");
    }

    #[test]
    fn process_spawn_stop_at_fixture_teardown_preserves_success_and_ownership() {
        let mut owned = SyntheticProcess::new("exit-race");
        let unrelated = SyntheticProcess::new("separate-group");
        owned.watched.as_mut().unwrap().pid = unrelated.watched.as_ref().unwrap().pid;
        owned.release();
        until(|| owned.fixture.path("exit-race.done").exists());
        // Force the late-signal window: the child is alive but has completed
        // its work. Its teardown gate prevents an accidental early exit.
        assert_eq!(
            owned.stop(GracefulStopScope::LaunchedChild, Duration::ZERO),
            GracefulStopOutcome::StillRunning {
                request_already_pending: false
            }
        );
        assert!(!owned.fixture.path("exit-race.terms").exists());
        unrelated.assert_alive("separate-group");
        fs::write(owned.fixture.path("finish"), b"finish").unwrap();
        assert!(owned.exit_report().status.as_ref().unwrap().success());
        assert_eq!(owned.exit_callbacks.load(Ordering::SeqCst), 1);
        assert_eq!(
            owned.stop(GracefulStopScope::LaunchedChild, Duration::ZERO),
            GracefulStopOutcome::AlreadyExited
        );
    }

    #[test]
    fn process_spawn_dropping_watched_handle_sends_no_signal_and_watcher_finishes() {
        let mut process = SyntheticProcess::new("single");
        drop(process.watched.take().unwrap());
        process.assert_alive("single");
        assert_eq!(process.exit_callbacks.load(Ordering::SeqCst), 0);
        process.release();
        until(|| process.exit_callbacks.load(Ordering::SeqCst) == 1);
        assert!(!process.fixture.path("single.terms").exists());
    }

    #[test]
    fn dolphin_wrapper_forwards_owned_child_stop_and_retains_exact_command_facts() {
        let ready = build_ready_fixture("graceful-stop-wrapper");
        fs::write(&ready.request.expected_executable, PROBE.as_bytes()).unwrap();
        let mut command = preflight(&ready).unwrap();
        // Direct spawn test only: synthetic probe argv, no real emulator.
        command.arguments = vec!["single".into(), ready.fixture.root.clone().into_os_string()];
        let expected = command_facts(&command);
        let mut process = spawn_dolphin(command).unwrap();
        until(|| ready.fixture.path("single.ready").exists());
        assert_eq!(
            process.request_graceful_stop(GracefulStopScope::ProcessTree, Duration::ZERO),
            GracefulStopOutcome::Refused(GracefulStopRefusal::ProcessTreeNotSupervised)
        );
        assert_eq!(
            process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::from_secs(3)),
            GracefulStopOutcome::ExitedAfterRequest
        );
        assert_eq!(process.command_facts, expected);
        until(|| process.poll().is_some());
        assert_eq!(
            process.poll().unwrap().stderr,
            b"synthetic diagnostic\ngraceful TERM\n"
        );
        assert_eq!(
            process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO),
            GracefulStopOutcome::AlreadyExited
        );
    }

    #[test]
    fn process_spawn_closed_parent_stdio_does_not_alias_the_ownership_channel() {
        const CHILD_FLAG: &str = "EMUWIZ_CLOSED_STDIO_STOP_TEST_CHILD";
        if std::env::var_os(CHILD_FLAG).is_some() {
            use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
            struct RestoreStdio(OwnedFd, OwnedFd);
            impl Drop for RestoreStdio {
                fn drop(&mut self) {
                    // Only this disposable test subprocess closes stdio;
                    // restore it before the Rust test harness resumes.
                    unsafe {
                        libc::dup2(self.0.as_raw_fd(), 0);
                        libc::dup2(self.1.as_raw_fd(), 1);
                    }
                }
            }
            let fixture = Fixture::new("closed-stdio-stop");
            let executable = fixture.write_executable("probe", PROBE.as_bytes());
            let command = PreparedProcessCommand {
                executable,
                arguments: vec!["stdio".into(), fixture.root.clone().into_os_string()],
                working_directory: None,
            };
            let saved_in = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
            let saved_out = unsafe { libc::fcntl(1, libc::F_DUPFD_CLOEXEC, 3) };
            assert!(saved_in >= 3 && saved_out >= 3);
            let restore = RestoreStdio(unsafe { OwnedFd::from_raw_fd(saved_in) }, unsafe {
                OwnedFd::from_raw_fd(saved_out)
            });
            unsafe {
                libc::close(0);
                libc::close(1);
            }
            let spawned = process_spawn::spawn_watched_process(&command);
            drop(restore);
            let mut watched = spawned.unwrap();
            until(|| fixture.path("stdio.ready").exists());
            assert_eq!(
                watched.request_graceful_stop(
                    GracefulStopScope::LaunchedChild,
                    Duration::from_secs(3)
                ),
                GracefulStopOutcome::ExitedAfterRequest
            );
            until(|| watched.poll().is_some());
            assert!(watched.poll().unwrap().status.as_ref().unwrap().success());
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "launch::dolphin_execution::tests::graceful_stop::process_spawn_closed_parent_stdio_does_not_alias_the_ownership_channel",
                "--test-threads=1",
            ])
            .env(CHILD_FLAG, "disposable-test-child")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-dolphin-execution-{label}-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn write_executable(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.write(relative, contents);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A structurally valid, minimal Dolphin disc header: 6-byte Game ID,
/// disc/revision bytes, and the GameCube magic word at `0x1c` - the only
/// bytes `inspect_catalogued_game_identity` ever reads for a direct ISO.
fn gamecube_iso_bytes(game_id: &[u8; 6], revision: u8) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x440];
    bytes[..6].copy_from_slice(game_id);
    bytes[6] = 1;
    bytes[7] = revision;
    bytes[0x1c..0x20].copy_from_slice(&[0xc2, 0x33, 0x9f, 0x3d]);
    bytes
}

fn base_roots(fixture: &Fixture) -> DolphinLocalDiscoveryRoots {
    DolphinLocalDiscoveryRoots {
        home: fixture.path("home"),
        xdg_config_home: fixture.path("config"),
        xdg_data_home: fixture.path("data"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
        dolphin_emu_userpath_override: None,
        path_override: Some(Vec::new()),
    }
}

/// A fully wired, genuinely `Ready` `Explicit`/`ExplicitRoot` native Dolphin
/// fixture: a fake executable script, a genuine single Dolphin user root
/// (`Config/Dolphin.ini` beneath it, proving `-u <root>` semantics), and a
/// loose GameCube ISO whose verified Game ID becomes the request's expected
/// Game ID - computed via the same `inspect_catalogued_game_identity` the
/// module itself uses, never hand-typed.
///
/// `Explicit`, not `Native`, because only `explicit_configuration_roots`/
/// `explicit_executables` let a test deterministically control which
/// installation type an executable resolves to - `Native` executables are
/// only ever found via a real `PATH` scan
/// (`discover_dolphin_local_executables`), which this suite never touches so
/// the real host environment can never leak into a test.
struct ReadyFixture {
    fixture: Fixture,
    roots: DolphinLocalDiscoveryRoots,
    profile_root: PathBuf,
    request: DolphinLaunchRequest,
}

fn build_ready_fixture(label: &str) -> ReadyFixture {
    let fixture = Fixture::new(label);
    let profile_root = fixture.path("dolphin-portable");
    fs::create_dir_all(profile_root.join("Config")).unwrap();
    fs::write(profile_root.join("Dolphin.ini"), b"[Core]\n").unwrap();
    fs::write(profile_root.join("Config/Dolphin.ini"), b"[Core]\n").unwrap();
    let executable = fixture.write_executable("bin/dolphin-emu", b"#!/bin/sh\nexit 0\n");
    let content = fixture.write("games/game.iso", &gamecube_iso_bytes(b"GALE01", 0));

    let mut roots = base_roots(&fixture);
    roots
        .explicit_configuration_roots
        .push(profile_root.clone());
    roots.explicit_executables.push(executable.clone());

    let request = DolphinLaunchRequest {
        selected_content_path: content,
        expected_game_id: "GALE01".to_string(),
        profile_id: format!("dolphin:{}", profile_root.display()),
        expected_executable: executable,
        expected_user_directory_mode: DolphinUserDirectoryMode::ExplicitRoot(profile_root.clone()),
    };
    ReadyFixture {
        fixture,
        roots,
        profile_root,
        request,
    }
}

fn preflight(ready: &ReadyFixture) -> Result<DolphinCommand, DolphinLaunchPreflightError> {
    preflight_dolphin_launch(&ready.request, &ready.roots)
}

// --- native direct-content Ready candidate passes preflight -----------------

#[test]
fn valid_native_gamecube_iso_succeeds() {
    let ready = build_ready_fixture("ready");
    let command =
        preflight(&ready).expect("a fully wired explicit-root fixture must preflight cleanly");
    assert_eq!(command.executable, ready.request.expected_executable);
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-u"),
            ready.profile_root.clone().into_os_string(),
            OsString::from("-e"),
            ready.request.selected_content_path.clone().into_os_string(),
        ]
    );
    assert_eq!(command.selection.platform_id, "GameCube");
    assert_eq!(command.selection.game_id, "GALE01");
    assert_eq!(
        command.selection.content_path,
        ready.request.selected_content_path
    );
}

// --- changed content identity is detected by the same mechanism used at spawn time ----------

#[test]
fn content_identity_capture_detects_a_swapped_file() {
    let ready = build_ready_fixture("content-changed");
    let path = ready.request.selected_content_path.clone();
    let original = fs::read(&path).unwrap();
    let before = CapturedFileIdentity::capture(&fs::symlink_metadata(&path).unwrap());
    fs::write(&path, b"different bytes entirely, same path").unwrap();
    let after = CapturedFileIdentity::capture(&fs::symlink_metadata(&path).unwrap());
    assert_ne!(
        before, after,
        "captured identity must change when the file changes"
    );
    fs::write(&path, &original).unwrap();
}

// --- symlink content rejected ------------------------------------------------------------------

#[test]
fn symlink_content_is_rejected() {
    let ready = build_ready_fixture("symlink-content");
    let link = ready.fixture.path("games/link.iso");
    symlink(&ready.request.selected_content_path, &link).unwrap();
    let mut request = ready.request.clone();
    request.selected_content_path = link;
    let error = preflight_dolphin_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::ContentIsSymlink
    );
}

// --- unsupported content format rejected ---------------------------------------------------------

#[test]
fn unsupported_extension_is_rejected() {
    let ready = build_ready_fixture("unsupported-format");
    let gcz = ready
        .fixture
        .write("games/game.gcz", &gamecube_iso_bytes(b"GALE01", 0));
    let mut request = ready.request.clone();
    request.selected_content_path = gcz;
    let error = preflight_dolphin_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::ContentFormatUnsupported
    );
}

#[test]
fn mount_input_archive_content_is_rejected() {
    let ready = build_ready_fixture("archive-content");
    let zip = ready.fixture.write("games/game.zip", b"pk not a real zip");
    let mut request = ready.request.clone();
    request.selected_content_path = zip;
    let error = preflight_dolphin_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::ContentRequiresMount
    );
}

// --- identity mismatch rejected -----------------------------------------------------------------

#[test]
fn wrong_expected_game_id_is_rejected() {
    let mut ready = build_ready_fixture("wrong-game-id");
    ready.request.expected_game_id = "RALE01".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::IdentityMismatch
    );
}

// --- profile disappears rejected ----------------------------------------------------------------

#[test]
fn profile_directory_removed_is_rejected() {
    let ready = build_ready_fixture("profile-removed");
    fs::remove_dir_all(&ready.profile_root).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- executable changes/disappears rejected -------------------------------------------------------

#[test]
fn executable_disappearing_is_rejected() {
    let ready = build_ready_fixture("executable-missing");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn executable_replaced_with_a_symlink_is_rejected() {
    let ready = build_ready_fixture("executable-symlink");
    let real = ready
        .fixture
        .write_executable("bin/dolphin-emu-real", b"#!/bin/sh\nexit 0\n");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    symlink(&real, &ready.request.expected_executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- binding drift between readiness and click rejected --------------------------------------------

#[test]
fn binding_drift_between_readiness_and_click_is_rejected() {
    let mut ready = build_ready_fixture("binding-drift");
    // A different executable path than the one that will freshly resolve -
    // simulating a GUI that showed the user a binding, then the real
    // binding changed (or the GUI's own remembered value was stale) before
    // they clicked Launch.
    ready.request.expected_executable = ready.fixture.path("bin/a-different-dolphin-emu");
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, DolphinLaunchPreflightErrorKind::BindingDrift);
}

#[test]
fn binding_mode_drift_is_rejected() {
    let mut ready = build_ready_fixture("binding-mode-drift");
    ready.request.expected_user_directory_mode = DolphinUserDirectoryMode::DefaultNative;
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, DolphinLaunchPreflightErrorKind::BindingDrift);
}

// --- explicit root replaced/symlinked rejected -----------------------------------------------------

#[test]
fn explicit_root_replaced_with_a_symlink_is_rejected() {
    let ready = build_ready_fixture("explicit-root-symlink");
    let real_root = ready.fixture.path("real-portable");
    fs::rename(&ready.profile_root, &real_root).unwrap();
    symlink(&real_root, &ready.profile_root).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- DOLPHIN_EMU_USERPATH / legacy layout conflicts reject DefaultNative --------------------------

/// A `Native` installation-type fixture built entirely from `roots` fields
/// (XDG config/data dirs + `Dolphin.ini`), with no executable candidate at
/// all. `DOLPHIN_EMU_USERPATH`/legacy-layout checks happen before Dolphin's
/// own executable resolution in
/// `resolve_dolphin_native_launch_binding::resolve_default_native_binding`,
/// so this fixture proves the rejection through the real preflight pipeline
/// without ever needing a `PATH`-discovered `Native` executable.
struct NativeConflictFixture {
    fixture: Fixture,
    roots: DolphinLocalDiscoveryRoots,
    request: DolphinLaunchRequest,
}

fn build_native_conflict_fixture(label: &str) -> NativeConflictFixture {
    let fixture = Fixture::new(label);
    let config_root = fixture.path("config/dolphin-emu");
    let data_root = fixture.path("data/dolphin-emu");
    fs::create_dir_all(&config_root).unwrap();
    fs::create_dir_all(&data_root).unwrap();
    fs::write(config_root.join("Dolphin.ini"), b"[Core]\n").unwrap();
    let content = fixture.write("games/game.iso", &gamecube_iso_bytes(b"GALE01", 0));

    let roots = base_roots(&fixture);
    let request = DolphinLaunchRequest {
        selected_content_path: content,
        expected_game_id: "GALE01".to_string(),
        profile_id: format!("dolphin:{}", config_root.display()),
        expected_executable: PathBuf::from("/usr/bin/dolphin-emu"),
        expected_user_directory_mode: DolphinUserDirectoryMode::DefaultNative,
    };
    NativeConflictFixture {
        fixture,
        roots,
        request,
    }
}

#[test]
fn dolphin_emu_userpath_override_rejects_default_native() {
    let mut ready = build_native_conflict_fixture("userpath-override");
    ready.roots.dolphin_emu_userpath_override = Some(ready.fixture.path("override-root"));
    let error = preflight_dolphin_launch(&ready.request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn legacy_dolphin_emu_directory_rejects_default_native() {
    let ready = build_native_conflict_fixture("legacy-precedence");
    fs::create_dir_all(ready.roots.home.join(".dolphin-emu")).unwrap();
    let error = preflight_dolphin_launch(&ready.request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- spawn: fake executable receives exact argv ------------------------------------------------------

#[test]
fn fake_executable_receives_exact_argv() {
    let ready = build_ready_fixture("argv-capture");
    let capture_path = ready.fixture.path("argv-capture.txt");
    ready.fixture.write_executable(
        "bin/dolphin-emu",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let command = preflight(&ready).unwrap();
    let mut process = spawn_dolphin(command).expect("the fake script must spawn");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if process.poll().is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(
        lines,
        vec![
            "-u",
            ready.profile_root.to_str().unwrap(),
            "-e",
            ready.request.selected_content_path.to_str().unwrap()
        ]
    );
}

// --- spawn: successful spawn returns PID, clean exit reported -----------------------------------------

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let ready = build_ready_fixture("spawn-success");
    let command = preflight(&ready).unwrap();
    let mut process = spawn_dolphin(command).expect("the fake script must spawn");
    assert!(process.pid > 0);
    assert!(process.is_running() || process.poll().is_some());

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let report = loop {
        if let Some(report) = process.poll() {
            break report;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let status = report.status.as_ref().expect("wait() must succeed");
    assert!(status.success(), "the fake script exits 0");
}

// --- spawn: non-zero exit and bounded stderr reported -------------------------------------------------

#[test]
fn non_zero_exit_and_stderr_are_reported() {
    let ready = build_ready_fixture("spawn-nonzero");
    ready.fixture.write_executable(
        "bin/dolphin-emu",
        b"#!/bin/sh\necho 'synthetic dolphin failure' 1>&2\nexit 7\n",
    );
    let command = preflight(&ready).unwrap();
    let mut process = spawn_dolphin(command).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let report = loop {
        if let Some(report) = process.poll() {
            break report;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic dolphin failure"));
}

// --- spawn: shell metacharacters never interpreted -------------------------------------------------

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-dolphin-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&marker);
    let ready = build_ready_fixture("shell-metacharacters");
    let dangerous_name = format!("game; touch {} #.iso", marker.display());
    let dangerous_path = ready.fixture.write(
        &format!("games/{dangerous_name}"),
        &gamecube_iso_bytes(b"GALE01", 0),
    );
    let mut request = ready.request.clone();
    request.selected_content_path = dangerous_path;
    let command = preflight_dolphin_launch(&request, &ready.roots).unwrap();
    let mut process = spawn_dolphin(command).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if process.poll().is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !marker.exists(),
        "shell metacharacters in a content path must never be interpreted"
    );
    let _ = fs::remove_file(&marker);
}

// --- spawn failure reported -----------------------------------------------------------------------

#[test]
fn spawn_failure_is_reported() {
    let ready = build_ready_fixture("spawn-failure");
    let mut command = preflight(&ready).unwrap();
    command.executable = ready.fixture.path("bin/does-not-exist");
    let result = spawn_dolphin(command);
    assert!(matches!(result, Err(DolphinLaunchSpawnError::Spawn(_))));
}

#[test]
fn nkit_marker_in_ordinary_iso_refuses_native_launch_without_losing_game_id() {
    let ready = build_ready_fixture("nkit-refusal");
    let mut bytes = gamecube_iso_bytes(b"GALE01", 0);
    bytes.resize(0x2440, 0);
    bytes[0x200..0x208].copy_from_slice(b"NKIT v01");
    bytes[0x210..0x214].copy_from_slice(&0x57058000u32.to_be_bytes());
    fs::write(&ready.request.selected_content_path, &bytes).unwrap();
    let report = crate::game_identity::inspect_catalogued_game_identity(
        &ready.request.selected_content_path,
        Some("GameCube"),
    );
    assert_eq!(report.verified_dolphin_game_id(), Some("GALE01"));
    let error =
        preflight(&ready).expect_err("NKit marker is not ordinary ISO launch compatibility");
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::ContentFormatUnsupported
    );
    assert!(error.detail.contains("NKit"));
    assert_eq!(
        fs::read(&ready.request.selected_content_path).unwrap(),
        bytes
    );
}

#[test]
fn ordinary_iso_with_nkit_filename_keeps_existing_native_launch_path() {
    let mut ready = build_ready_fixture("ordinary-nkit-name");
    let renamed = ready
        .request
        .selected_content_path
        .with_file_name("ordinary.nkit.iso");
    let mut bytes = gamecube_iso_bytes(b"GALE01", 0);
    bytes.resize(0x2440, 0);
    fs::write(&renamed, bytes).unwrap();
    ready.request.selected_content_path = renamed;
    assert!(preflight(&ready).is_ok());
}

#[test]
fn nkit_raw_game_ids_overlapping_wrapper_signatures_are_refused_as_ambiguous() {
    for game_id in [b"WBFS01", b"CISO01"] {
        let mut ready = build_ready_fixture("nkit-overlapping-signatures");
        let mut bytes = gamecube_iso_bytes(game_id, 0);
        bytes.resize(0x2440, 0);
        bytes[0x200..0x208].copy_from_slice(b"NKIT v01");
        bytes[0x210..0x214].copy_from_slice(&0x57058000u32.to_be_bytes());
        fs::write(&ready.request.selected_content_path, &bytes).unwrap();
        ready.request.expected_game_id = std::str::from_utf8(game_id).unwrap().into();
        let identity = crate::game_identity::inspect_catalogued_game_identity(
            &ready.request.selected_content_path,
            Some("GameCube"),
        );
        assert_eq!(
            identity.verified_dolphin_game_id(),
            Some(ready.request.expected_game_id.as_str())
        );
        let error = preflight(&ready).unwrap_err();
        assert_eq!(
            error.kind,
            DolphinLaunchPreflightErrorKind::ContentFormatUnsupported
        );
        assert!(error.detail.contains("ambiguous"));
    }
}

#[test]
fn supported_wrappers_keep_existing_identity_and_native_preflight_contract() {
    for extension in ["rvz", "ciso"] {
        let mut ready = build_ready_fixture("nkit-ordinary-wrapper");
        let header = gamecube_iso_bytes(b"GALE01", 0);
        let mut bytes = if extension == "rvz" {
            let mut bytes = vec![0u8; 0x440];
            bytes[..4].copy_from_slice(b"RVZ\x01");
            bytes[0x48..0x4c].copy_from_slice(&1u32.to_be_bytes());
            bytes[0x58..0x78].copy_from_slice(&header[..0x20]);
            bytes
        } else {
            let mut bytes = vec![0u8; 0x8800];
            bytes[..4].copy_from_slice(b"CISO");
            bytes[4..8].copy_from_slice(&2048u32.to_le_bytes());
            bytes[8] = 1;
            bytes[0x8000..0x8020].copy_from_slice(&header[..0x20]);
            bytes
        };
        // This physical wrapper offset is not a logical disc-header offset.
        bytes[0x200..0x204].copy_from_slice(b"NKIT");
        let path = ready
            .fixture
            .write(&format!("games/ordinary.{extension}"), &bytes);
        ready.request.selected_content_path = path.clone();
        let identity =
            crate::game_identity::inspect_catalogued_game_identity(&path, Some("GameCube"));
        assert_eq!(identity.verified_dolphin_game_id(), Some("GALE01"));
        assert!(
            preflight(&ready).is_ok(),
            "ordinary {extension} must retain the existing launch contract"
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

fn build_flatpak_fixture(label: &str) -> ReadyFixture {
    let mut ready = build_ready_fixture(label);
    let app_id = crate::launch::installation_known::DOLPHIN.flatpak_ids[0];
    let profile = ready.roots.home.join(".var/app").join(app_id);
    let config = profile.join("config/dolphin-emu");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(profile.join("data/dolphin-emu")).unwrap();
    fs::write(config.join("Dolphin.ini"), b"[Core]\n").unwrap();
    ready.request.profile_id = format!("dolphin:{}", config.display());
    ready.request.expected_user_directory_mode = DolphinUserDirectoryMode::DefaultNative;
    let binary = ready
        .fixture
        .write_executable("bin/flatpak", b"#!/bin/sh\nexit 0\n");
    ready.roots.path_override = Some(vec![binary.parent().unwrap().to_path_buf()]);
    ready.request.expected_executable = binary;
    ready.fixture.write(
        &format!("data/flatpak/app/{app_id}/current/active/metadata"),
        b"[Application]\n[Context]\nfilesystems=host:ro;\n",
    );
    ready
}

#[test]
fn flatpak_preflight_revalidates_default_profile_and_preserves_owned_process_tracking() {
    let mut ready = build_flatpak_fixture("flatpak-lifecycle");
    let command = preflight(&ready).unwrap();
    assert_eq!(command.arguments[0], "run");
    assert!(command.arguments[1].to_str().unwrap().ends_with(":ro"));
    let mut launched = preflight_and_launch_dolphin(&ready.request, &ready.roots).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while launched.poll().is_none() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!launched.is_running());
    ready.roots.dolphin_emu_userpath_override = Some(ready.fixture.path("other-profile"));
    assert_eq!(
        preflight(&ready).unwrap_err().kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

/// Explicit opt-in QA: the caller must provide a private HOME/XDG session,
/// initialized Dolphin profile, and a disposable copy of authorised media.
/// Set EMUWIZ_TEST_REAL_ENVIRONMENT=1 only with those private environment
/// roots, so the global test constructor preserves the explicit QA session.
#[test]
#[ignore = "requires an installed Flatpak, graphical session and authorised disposable media"]
fn dolphin_flatpak_disposable_live() {
    let qa = PathBuf::from(std::env::var_os("EMUWIZ_DOLPHIN_QA_ROOT").expect("private QA root"));
    let roots = DolphinLocalDiscoveryRoots::from_environment().unwrap();
    assert_eq!(roots.home, qa.join("home"));
    assert!(roots.xdg_config_home.starts_with(&qa));
    assert!(roots.xdg_data_home.starts_with(&qa));
    let content = qa.join("media/game.rvz");
    let discovery = discover_dolphin_local_profiles(&roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| {
            profile.installation_type
                == crate::patch_manager::DolphinLocalInstallationType::FlatpakUser
        })
        .unwrap();
    let binding = resolve_dolphin_native_launch_binding(profile, &roots).unwrap();
    let request = DolphinLaunchRequest {
        selected_content_path: content,
        expected_game_id: std::env::var("EMUWIZ_DOLPHIN_QA_GAME_ID").unwrap(),
        profile_id: profile.profile_id.clone(),
        expected_executable: binding.executable,
        expected_user_directory_mode: binding.user_directory_mode,
    };
    let command = preflight_dolphin_launch(&request, &roots).unwrap();
    println!("LIVE command: {:?}", command.command_spec());
    let mut launched = preflight_and_launch_dolphin(&request, &roots).unwrap();
    fs::write(qa.join("launched.pid"), launched.pid.to_string()).unwrap();
    std::thread::sleep(Duration::from_secs(25));
    assert!(
        launched.is_running(),
        "Dolphin exited before the gameplay observation"
    );
    let outcome =
        launched.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::from_secs(10));
    println!("LIVE stop: {outcome:?}");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while launched.poll().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "exit report is still draining; close surviving Dolphin descendants normally"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("LIVE exit: {:?}", launched.poll().unwrap());
}

#[test]
fn flatpak_launch_keeps_lazy_data_initialization_and_refuses_unsafe_ancestry() {
    let ready = build_flatpak_fixture("flatpak-lazy-data");
    let data = ready
        .roots
        .home
        .join(".var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu");
    fs::remove_dir(&data).unwrap();
    fs::remove_dir(data.parent().unwrap()).unwrap();
    assert!(
        preflight(&ready).is_ok(),
        "sandbox may initialise its own missing data tree"
    );
    assert!(
        !data.exists(),
        "preflight must not create profile directories"
    );
    fs::create_dir_all(data.parent().unwrap()).unwrap();
    fs::write(&data, b"not a directory").unwrap();
    assert_eq!(
        preflight(&ready).unwrap_err().kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
    fs::remove_file(&data).unwrap();
    symlink(ready.fixture.path("missing-target"), &data).unwrap();
    assert_eq!(
        preflight(&ready).unwrap_err().kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
    fs::remove_file(&data).unwrap();
    fs::remove_dir(data.parent().unwrap()).unwrap();
    let elsewhere = ready.fixture.path("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, data.parent().unwrap()).unwrap();
    assert_eq!(
        preflight(&ready).unwrap_err().kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}
