//! The player's confirmed second stop (issue #528): exactly one more SIGTERM
//! to the same owned child, only after a first, never a third.
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }

    /// A child that counts every SIGTERM and never exits on its own, like
    /// RetroArch whose own shutdown has stalled; it ends only on release.
    fn spawn_counting_child(&self) -> WatchedProcess {
        let path = self.path("emulator.sh");
        let body = format!(
            "#!/bin/sh\ntrap 'printf x >> \"{count}\"' TERM\n: > '{ready}'\nwhile [ ! -e '{release}' ]; do sleep 0.01; done\nexit 1\n",
            count = self.path("count").display(),
            ready = self.path("ready").display(),
            release = self.path("release").display(),
        );
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let process = spawn_watched_process(&PreparedProcessCommand {
            executable: path,
            arguments: Vec::new(),
            working_directory: None,
        })
        .unwrap();
        self.wait_for("ready");
        process
    }

    fn wait_for(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.path(name).exists() {
            assert!(Instant::now() < deadline, "{name} never appeared");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Signals counted so far, after giving any pending trap time to run.
    fn signals(&self) -> usize {
        std::thread::sleep(Duration::from_millis(150));
        std::fs::read(self.path("count")).map_or(0, |count| count.len())
    }

    fn release(&self, process: &mut WatchedProcess) {
        std::fs::write(self.path("release"), b"").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while process.poll().is_none() {
            assert!(Instant::now() < deadline, "the test process did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn fixture() -> Fixture {
    Fixture {
        dir: tempfile::tempdir().unwrap(),
    }
}

#[test]
fn a_second_stop_is_never_the_first_signal() {
    let f = fixture();
    let mut process = f.spawn_counting_child();
    assert_eq!(
        process.request_second_stop(Duration::ZERO),
        GracefulStopOutcome::Refused(GracefulStopRefusal::NoEarlierStopRequest)
    );
    assert_eq!(f.signals(), 0);
    f.release(&mut process);
}

#[test]
fn after_a_first_stop_the_second_sends_exactly_one_more_signal_and_never_a_third() {
    let f = fixture();
    let mut process = f.spawn_counting_child();
    assert_eq!(
        process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO),
        GracefulStopOutcome::StillRunning {
            request_already_pending: false
        }
    );
    // Repeating the first request still only observes.
    process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO);
    assert_eq!(f.signals(), 1);

    assert_eq!(
        process.request_second_stop(Duration::ZERO),
        GracefulStopOutcome::StillRunning {
            request_already_pending: false
        }
    );
    assert_eq!(
        process.request_second_stop(Duration::ZERO),
        GracefulStopOutcome::StillRunning {
            request_already_pending: true
        }
    );
    process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO);
    assert_eq!(
        f.signals(),
        2,
        "one first and one second request, never a third"
    );

    f.release(&mut process);
    assert_eq!(
        process.request_second_stop(Duration::ZERO),
        GracefulStopOutcome::AlreadyExited
    );
    assert_eq!(f.signals(), 2);
}

#[test]
fn a_child_that_exits_on_the_second_signal_is_reported_as_exited_after_the_request() {
    let f = fixture();
    // RetroArch's own rule: the first termination signal asks it to close,
    // a second while it is closing makes it exit at once with status 1.
    let path = f.path("emulator.sh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nn=0\ntrap 'n=$((n+1)); [ $n -ge 2 ] && exit 1' TERM\n: > '{}'\nwhile :; do sleep 0.01; done\n",
            f.path("ready").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut process = spawn_watched_process(&PreparedProcessCommand {
        executable: path,
        arguments: Vec::new(),
        working_directory: None,
    })
    .unwrap();
    f.wait_for("ready");
    assert!(matches!(
        process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::from_millis(300)),
        GracefulStopOutcome::StillRunning { .. }
    ));
    assert_eq!(
        process.request_second_stop(Duration::from_secs(20)),
        GracefulStopOutcome::ExitedAfterRequest
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        process
            .exit_report()
            .unwrap()
            .status
            .as_ref()
            .unwrap()
            .code(),
        Some(1)
    );
}
