//! Disposable children only; no installed Cemu, game, profile or guessed PID.
use super::*;
use crate::launch::cemu_command::CemuCommandSelection;
use crate::launch::process_spawn::{
    GracefulStopOutcome as Outcome, GracefulStopRefusal, GracefulStopScope as Scope,
};
use std::time::{Duration, Instant};

const PROBE: &str = r#"
import pathlib, signal, sys, time
root = pathlib.Path(sys.argv[1])
ignore = sys.argv[2] == 'ignore'
running = True
terms = 0
def term(signum, frame):
    global running, terms
    terms += 1
    (root / 'terms').write_text(str(terms))
    sys.stderr.write('graceful TERM\n')
    sys.stderr.flush()
    if not ignore:
        running = False
signal.signal(signal.SIGTERM, term)
(root / 'ready').write_text('ready')
deadline = time.monotonic() + 15
while running and not (root / 'release').exists() and time.monotonic() < deadline:
    time.sleep(0.005)
signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGTERM})
"#;

fn until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "synthetic child missed checkpoint"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Fixture {
    root: tempfile::TempDir,
    process: LaunchedCemuProcess,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("probe.py");
        fs::write(&script, PROBE).unwrap();
        let command = CemuCommand {
            executable: "/usr/bin/python3".into(),
            arguments: vec![
                script.into_os_string(),
                root.path().as_os_str().into(),
                mode.into(),
            ],
            working_directory: None,
            selection: CemuCommandSelection {
                profile_id: "synthetic-cemu".into(),
                platform_id: "WiiU".into(),
                content_path: root.path().join("title"),
                rpx_path: root.path().join("title/code/game.rpx"),
                title_id: Some("0005000010101A00".into()),
            },
        };
        let process = spawn_cemu(command).unwrap();
        until(|| root.path().join("ready").exists());
        Self { root, process }
    }

    fn stop(&mut self) -> Outcome {
        self.process
            .request_graceful_stop(Scope::LaunchedChild, Duration::ZERO)
    }

    fn release(&self) {
        fs::write(self.root.path().join("release"), b"release").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.release();
        until(|| self.process.poll().is_some());
    }
}

#[test]
fn cemu_owned_child_stops_normally_and_keeps_exit_diagnostics() {
    let mut fixture = Fixture::new("normal");
    assert!(matches!(
        fixture.stop(),
        Outcome::StillRunning { .. } | Outcome::ExitedAfterRequest
    ));
    until(|| fixture.process.poll().is_some());
    let report = fixture.process.poll().unwrap();
    assert!(report.status.as_ref().unwrap().success());
    assert_eq!(report.stderr, b"graceful TERM\n");
    assert_eq!(fixture.stop(), Outcome::AlreadyExited);
}

#[test]
fn cemu_repeated_stop_is_one_signal_without_escalation() {
    let mut fixture = Fixture::new("ignore");
    assert_eq!(
        fixture.stop(),
        Outcome::StillRunning {
            request_already_pending: false
        }
    );
    until(|| fs::read_to_string(fixture.root.path().join("terms")).is_ok_and(|s| s == "1"));
    assert_eq!(
        fixture.stop(),
        Outcome::StillRunning {
            request_already_pending: true
        }
    );
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("terms")).unwrap(),
        "1"
    );
    assert!(fixture.process.poll().is_none());
    fixture.release();
    until(|| fixture.process.poll().is_some());
    assert!(
        fixture
            .process
            .poll()
            .unwrap()
            .status
            .as_ref()
            .unwrap()
            .success()
    );
}

#[test]
fn cemu_normal_exit_then_stop_is_a_no_op() {
    let mut fixture = Fixture::new("normal");
    fixture.release();
    until(|| fixture.process.poll().is_some());
    assert_eq!(fixture.stop(), Outcome::AlreadyExited);
    assert!(!fixture.root.path().join("terms").exists());
}

#[test]
fn cemu_pid_ownership_mismatch_refuses_without_signalling_either_child() {
    let mut owned = Fixture::new("normal");
    let mut foreign = Fixture::new("normal");
    let real_pid = owned.process.pid;
    owned.process.pid = foreign.process.pid;
    assert!(matches!(
        owned.stop(),
        Outcome::Refused(GracefulStopRefusal::OwnershipUnavailable { .. })
    ));
    assert!(owned.process.poll().is_none());
    assert!(foreign.process.poll().is_none());
    assert!(!owned.root.path().join("terms").exists());
    assert!(!foreign.root.path().join("terms").exists());
    owned.process.pid = real_pid;
}

#[test]
fn cemu_stale_pid_cannot_target_a_live_foreign_process() {
    let mut stale = Fixture::new("normal");
    stale.release();
    until(|| stale.process.poll().is_some());
    let mut foreign = Fixture::new("normal");
    stale.process.pid = foreign.process.pid;
    assert!(matches!(
        stale.stop(),
        Outcome::Refused(GracefulStopRefusal::OwnershipUnavailable { .. })
    ));
    assert!(foreign.process.poll().is_none());
    assert!(!foreign.root.path().join("terms").exists());
}

#[test]
fn cemu_whole_process_tree_stop_is_refused() {
    let mut fixture = Fixture::new("normal");
    assert_eq!(
        fixture
            .process
            .request_graceful_stop(Scope::ProcessTree, Duration::ZERO),
        Outcome::Refused(GracefulStopRefusal::ProcessTreeNotSupervised)
    );
    assert!(fixture.process.poll().is_none());
    assert!(!fixture.root.path().join("terms").exists());
}
