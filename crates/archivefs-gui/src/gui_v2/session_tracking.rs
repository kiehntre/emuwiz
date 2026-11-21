//! Tells the save-continuity session journal about Blue's launches.
//!
//! Blue knows only "a launch is in progress" and, later, "it is over". The
//! real process, its start time and its exit status come from
//! [`archivefs_core::launch::process_log`], which every emulator route feeds.
//! This module joins the two and passes them to
//! [`LaunchTracker`](archivefs_core::save_continuity::tracker::LaunchTracker).
//!
//! Everything slow (reading saves to mark their content) happens on one
//! worker thread; the interface thread only sends a message. While a session
//! is open the worker looks at it every [`LOOK_EVERY`], with its own timer,
//! not Blue's frames: that is what keeps "the emulator was last seen alive"
//! current through a long game, which the crash-record lookup depends on.
//!
//! This only records. It never copies, restores or changes a save, and a
//! failure here never affects a launch: the session is simply not recorded.

use archivefs_core::launch::process_log::{self, StartedProcess};
use archivefs_core::save_continuity::ContinuityError;
use archivefs_core::save_continuity::session::{SessionGame, SessionState};
use archivefs_core::save_continuity::tracker::LaunchTracker;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

/// How often an open session is looked at. The stored "last seen alive"
/// time is written at most once a minute and must never be more than ten
/// minutes old for a crash record to be tied to the emulator, so this has
/// to stay far below that.
pub(super) const LOOK_EVERY: Duration = Duration::from_secs(20);

/// How long to wait for the launcher's exit status to be written down after
/// Blue reports the launch over, before recording it as not available.
const EXIT_STATUS_PATIENCE: u32 = 15;
const SHORT_WAIT: Duration = Duration::from_millis(200);

enum Message {
    Began {
        /// Processes started from this sequence on belong to this launch.
        after_sequence: u64,
        game: SessionGame,
        emulator: String,
    },
    Finished {
        failure: Option<String>,
    },
}

type OpenTracker = Box<dyn FnOnce() -> Result<LaunchTracker, ContinuityError> + Send>;
type StartedSince = fn(u64) -> Vec<StartedProcess>;

/// The interface thread's handle. Cheap; the worker starts with the first
/// launch.
#[derive(Default)]
pub(super) struct SessionTracking {
    worker: Option<Sender<Message>>,
}

impl SessionTracking {
    /// A launch has just become active.
    pub(super) fn launch_began(&mut self, emulator: &str, game: SessionGame) {
        self.send(Message::Began {
            after_sequence: process_log::next_sequence(),
            game,
            emulator: emulator.to_string(),
        });
    }

    /// Blue's launch activity is over (the process it started has ended, or
    /// none could be started).
    pub(super) fn launch_finished(&mut self, failure: Option<String>) {
        self.send(Message::Finished { failure });
    }

    fn send(&mut self, message: Message) {
        // Blue's own tests drive the launch activity with made-up launches.
        // They must never open the real save-continuity folder or profile;
        // the worker is tested directly, with folders of its own.
        if cfg!(test) {
            drop(message);
            return;
        }
        let worker = self.worker.get_or_insert_with(|| {
            start(
                Box::new(open_real_tracker),
                process_log::started_since,
                LOOK_EVERY,
            )
        });
        // A worker that has gone away only means nothing is recorded.
        if worker.send(message).is_err() {
            self.worker = None;
        }
    }
}

/// The person's own save-continuity folder and profile.
///
/// Recording a launch creates the session journal's folder and nothing
/// else: no snapshot store, and no device identity or signing key. Those
/// belong to snapshots and to the two-device feature and are created only
/// when those are used. If an identity already exists its id is used.
fn open_real_tracker() -> Result<LaunchTracker, ContinuityError> {
    use archivefs_core::save_continuity::device::LocalDevice;
    use archivefs_core::save_continuity::discover::DiscoveryRoots;
    use archivefs_core::save_continuity::session::UNPAIRED_DEVICE_ID;
    let root = archivefs_core::save_continuity::default_root()
        .map_err(|error| ContinuityError::Io(error.to_string()))?;
    let device_id = LocalDevice::open_existing(&root)
        .ok()
        .flatten()
        .map_or_else(
            || UNPAIRED_DEVICE_ID.to_string(),
            |device| device.identity.device_id,
        );
    let roots = DiscoveryRoots::current()
        .ok_or_else(|| ContinuityError::Refused("HOME is not set".to_string()))?;
    LaunchTracker::open(&root, roots, &device_id)
}

fn start(open: OpenTracker, started_since: StartedSince, look_every: Duration) -> Sender<Message> {
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("save-continuity-sessions".to_string())
        .spawn(move || work(receiver, open, started_since, look_every));
    // If the thread cannot be started the receiver is dropped with the
    // closure and every send fails quietly.
    drop(spawned);
    sender
}

/// One launch the worker is following.
struct Open {
    session_id: String,
    after_sequence: u64,
    /// The started process has been passed to the tracker.
    started: bool,
    /// Blue reported the launch over.
    over: Option<Option<String>>,
    /// The launcher's exit has been passed on; the emulator may live on.
    launcher_done: bool,
    waited_for_status: u32,
}

fn work(
    receiver: Receiver<Message>,
    open: OpenTracker,
    started_since: StartedSince,
    look_every: Duration,
) {
    let mut open = Some(open);
    let mut tracker: Option<LaunchTracker> = None;
    let mut sessions: Vec<Open> = Vec::new();
    loop {
        let waiting_for_status = sessions
            .iter()
            .any(|session| session.over.is_some() && !session.launcher_done);
        let message = if sessions.is_empty() {
            match receiver.recv() {
                Ok(message) => Some(message),
                Err(_) => return,
            }
        } else {
            let wait = if waiting_for_status {
                SHORT_WAIT.min(look_every)
            } else {
                look_every
            };
            match receiver.recv_timeout(wait) {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => None,
                // Blue is closing. What is still open is settled by the
                // next run of EmuWiz, from what was last written.
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        if tracker.is_none()
            && let Some(open) = open.take()
        {
            // Without a tracker nothing is recorded; launches are unaffected.
            tracker = open().ok();
        }
        let Some(tracker) = tracker.as_ref() else {
            // Opening failed once; keep draining so senders never block.
            continue;
        };
        match message {
            Some(Message::Began {
                after_sequence,
                game,
                emulator,
            }) => {
                if let Ok(record) = tracker.begin(game, &emulator) {
                    sessions.push(Open {
                        session_id: record.session_id,
                        after_sequence,
                        started: false,
                        over: None,
                        launcher_done: false,
                        waited_for_status: 0,
                    });
                }
            }
            Some(Message::Finished { failure }) => {
                // The launch that is over is the newest one not yet over.
                if let Some(session) = sessions
                    .iter_mut()
                    .rev()
                    .find(|session| session.over.is_none())
                {
                    session.over = Some(failure);
                }
            }
            None => {}
        }
        sessions.retain_mut(|session| follow(tracker, session, started_since));
        if sessions.iter().any(|session| session.launcher_done) {
            // Ends the sessions whose emulator has gone since.
            tracker.settle();
        }
    }
}

/// One step for one launch. Returns whether to keep following it.
fn follow(tracker: &LaunchTracker, session: &mut Open, started_since: StartedSince) -> bool {
    let process = started_since(session.after_sequence).into_iter().next();
    if !session.started
        && let Some(process) = &process
    {
        if tracker
            .process_started(&session.session_id, process)
            .is_err()
        {
            return false;
        }
        session.started = true;
    }
    if session.launcher_done {
        // The launcher is gone; the emulator it started may still be
        // running. Keep noting that it is alive until it is not.
        return tracker
            .look_again(&session.session_id)
            .is_ok_and(|record| record.state == SessionState::Started);
    }
    let Some(failure) = &session.over else {
        if session.started {
            let _ = tracker.look_again(&session.session_id);
        }
        return true;
    };
    if !session.started {
        // No process was ever started for this launch.
        let _ = tracker.finished(&session.session_id, None, failure.as_deref());
        return false;
    }
    let exit = process.and_then(|process| process.exit);
    if exit.is_none() && session.waited_for_status < EXIT_STATUS_PATIENCE {
        // The watcher thread writes the status a moment after Blue sees the
        // launch end. Wait for it rather than record "not available".
        session.waited_for_status += 1;
        return true;
    }
    match tracker.finished(&session.session_id, exit, failure.as_deref()) {
        Ok(record) if record.state == SessionState::Started => {
            session.launcher_done = true;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::process_log::ExitSummary;
    use archivefs_core::save_continuity::discover::DiscoveryRoots;
    use archivefs_core::save_continuity::session::{SessionEnd, SessionJournal, SessionRecord};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Instant;

    /// What the launch layer would report, set by each test. Sequence
    /// numbers keep the tests apart: each uses its own.
    static STARTED: Mutex<Vec<StartedProcess>> = Mutex::new(Vec::new());

    fn started_since(sequence: u64) -> Vec<StartedProcess> {
        STARTED
            .lock()
            .unwrap()
            .iter()
            .filter(|process| process.sequence == sequence)
            .cloned()
            .collect()
    }

    fn report(process: StartedProcess) {
        let mut started = STARTED.lock().unwrap();
        started.retain(|known| known.sequence != process.sequence);
        started.push(process);
    }

    fn tracking(root: &Path, home: &Path, look_every: Duration) -> Sender<Message> {
        let (root, home) = (root.to_path_buf(), home.to_path_buf());
        start(
            Box::new(move || LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev")),
            started_since,
            look_every,
        )
    }

    fn sessions(root: &Path) -> Vec<SessionRecord> {
        SessionJournal::open(root, "dev").unwrap().all()
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn ticks(pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(19)?
            .parse()
            .ok()
    }

    fn game() -> SessionGame {
        SessionGame {
            library_id: Some(7),
            title: "Test Game".into(),
            content_path: Some(PathBuf::from("/games/test.iso")),
            ..Default::default()
        }
    }

    #[test]
    fn a_launch_is_recorded_from_request_to_end_without_the_interface_doing_the_work() {
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
        std::fs::create_dir_all(&home).unwrap();
        let worker = tracking(&root, &home, Duration::from_millis(50));
        const SEQUENCE: u64 = 9_000_001;
        worker
            .send(Message::Began {
                after_sequence: SEQUENCE,
                game: game(),
                emulator: "PCSX2".into(),
            })
            .unwrap();
        until("the request is recorded", || sessions(&root).len() == 1);
        assert_eq!(sessions(&root)[0].state, SessionState::Requested);
        assert_eq!(sessions(&root)[0].game.title, "Test Game");

        // The launch layer starts a real process.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut process = StartedProcess {
            sequence: SEQUENCE,
            pid: child.id(),
            start_ticks: ticks(child.id()),
            executable: PathBuf::from("/usr/bin/sleep"),
            arguments: vec!["30".into()],
            exit: None,
        };
        report(process.clone());
        until("the process is recorded", || {
            sessions(&root)[0].state == SessionState::Started
        });
        assert_eq!(sessions(&root)[0].process_id, Some(child.id()));

        // It ends; Blue reports the launch over a moment before the status
        // is written down. The worker waits for the status.
        child.kill().unwrap();
        child.wait().unwrap();
        worker.send(Message::Finished { failure: None }).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(sessions(&root)[0].state, SessionState::Started);
        process.exit = Some(ExitSummary::Signal {
            signal: 9,
            core_dumped: false,
        });
        report(process);
        until("the end is recorded", || {
            sessions(&root)[0].state == SessionState::Ended
        });
        let ended = &sessions(&root)[0];
        // `sleep` is not an emulator EmuWiz knows: it is treated as a
        // launcher, so its end says nothing about any emulator.
        assert!(
            matches!(ended.end, Some(SessionEnd::LauncherEndedOtherwise { .. })),
            "{:?}",
            ended.end
        );
    }

    #[test]
    fn a_launch_that_never_started_a_process_is_recorded_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
        std::fs::create_dir_all(&home).unwrap();
        let worker = tracking(&root, &home, Duration::from_millis(50));
        worker
            .send(Message::Began {
                after_sequence: 9_000_002,
                game: game(),
                emulator: "RetroArch".into(),
            })
            .unwrap();
        worker
            .send(Message::Finished {
                failure: Some("the core is missing".into()),
            })
            .unwrap();
        until("the failed launch is recorded", || {
            sessions(&root)
                .first()
                .is_some_and(|session| session.state == SessionState::NeverStarted)
        });
        assert!(
            sessions(&root)[0]
                .notes
                .iter()
                .any(|note| note.contains("the core is missing")),
            "{:?}",
            sessions(&root)[0].notes
        );
    }

    #[test]
    fn a_tracker_that_cannot_be_opened_never_blocks_or_fails_a_launch() {
        let worker = start(
            Box::new(|| Err(ContinuityError::Refused("no folder".into()))),
            started_since,
            Duration::from_millis(20),
        );
        for _ in 0..50 {
            worker
                .send(Message::Began {
                    after_sequence: 9_000_003,
                    game: game(),
                    emulator: "PCSX2".into(),
                })
                .unwrap();
            worker.send(Message::Finished { failure: None }).unwrap();
        }
    }

    #[test]
    fn an_open_session_is_looked_at_on_the_workers_own_timer() {
        // The look interval is what keeps a long session's "still alive"
        // time current; it must stay far below the ten-minute window and
        // must not depend on the interface drawing frames.
        use archivefs_core::save_continuity::session::{
            CRASH_RECORD_WINDOW_SECONDS, LAST_ALIVE_WRITE_SECONDS,
        };
        assert!(
            (LOOK_EVERY.as_secs() as i64 + LAST_ALIVE_WRITE_SECONDS) * 3
                <= CRASH_RECORD_WINDOW_SECONDS
        );
        // With no message at all after the start, a process the launcher
        // starts later is still picked up: the worker woke by itself.
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
        std::fs::create_dir_all(&home).unwrap();
        let worker = tracking(&root, &home, Duration::from_millis(40));
        const SEQUENCE: u64 = 9_000_004;
        worker
            .send(Message::Began {
                after_sequence: SEQUENCE,
                game: game(),
                emulator: "PCSX2".into(),
            })
            .unwrap();
        until("the request is recorded", || sessions(&root).len() == 1);
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        report(StartedProcess {
            sequence: SEQUENCE,
            pid: child.id(),
            start_ticks: ticks(child.id()),
            executable: PathBuf::from("/usr/bin/sleep"),
            arguments: vec!["30".into()],
            exit: None,
        });
        until("the worker noticed the process by itself", || {
            sessions(&root)[0].state == SessionState::Started
        });
        child.kill().unwrap();
        child.wait().unwrap();
        drop(worker);
    }
}
