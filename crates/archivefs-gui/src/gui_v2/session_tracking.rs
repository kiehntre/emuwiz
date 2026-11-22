//! Tells the save-continuity session journal about Blue's launches.
//!
//! Blue knows only "a launch is in progress" and, later, "it is over". The
//! real process, its start time and its exit status come from
//! [`archivefs_core::launch::process_log`], which every emulator route feeds.
//! This module joins the two and passes them to
//! [`LaunchTracker`](archivefs_core::save_continuity::tracker::LaunchTracker).
//!
//! **The saves are read before the emulator exists.** This module sets the
//! process log's before-start gate: the one function that creates every
//! emulator process calls it first and waits, and the gate returns only when
//! that emulator's saves have been read (or the read has been given up on,
//! in which case the session says save changes were not tracked, never
//! "changed" or "unchanged"). Nothing is inferred from file times. That read
//! has a thread of its own and needs only the profile's folders: it never
//! waits behind the session bookkeeping below, however long the history is.
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
use archivefs_core::save_continuity::session::{ProcFs, SessionGame, SessionState};
use archivefs_core::save_continuity::tracker::{
    Baseline, LaunchTracker, classify_started, saves_known_for,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The longest an emulator's start is held for the read of its saves. The
/// read takes a few hundredths of a second; if it has not finished by then
/// the game starts anyway and the session records that save changes were
/// not tracked.
const GATE_PATIENCE: Duration = Duration::from_secs(10);

const PENDING: u8 = 0;
const DONE: u8 = 1;
const ABANDONED: u8 = 2;

/// What the gate's read gave for one start: the saves and how long the
/// start was held (milliseconds), or why there is nothing.
type GateResult = Result<(Baseline, u64, u64), String>;

const NOT_READ: &str = "the saves could not be read before the emulator started";
const TOO_SLOW: &str = "reading the saves took longer than the start could be held";
const NOT_COVERED: &str = "EmuWiz does not know where this emulator keeps its saves";

/// How often an open session is looked at. The stored "last seen alive"
/// time is written at most once a minute and must never be more than ten
/// minutes old for a crash record to be tied to the emulator, so this has
/// to stay far below that.
pub(super) const LOOK_EVERY: Duration = Duration::from_secs(20);

/// How long to wait for the launcher's exit status to be written down after
/// Blue reports the launch over, before recording it as not available.
const EXIT_STATUS_PATIENCE: u32 = 15;
const SHORT_WAIT: Duration = Duration::from_millis(200);
/// Looks (a fifth of a second apart) to wait for the emulator itself when a
/// launch has so far started only something else.
const EMULATOR_PATIENCE: u32 = 10;

enum Message {
    Began {
        /// Processes started from this sequence on belong to this launch.
        after_sequence: u64,
        game: SessionGame,
        emulator: String,
    },
    Finished {
        failure: Option<String>,
        /// Processes started from this sequence on came after this launch
        /// was over and belong to a later one.
        until_sequence: u64,
    },
}

type OpenTracker = Box<dyn FnOnce() -> Result<LaunchTracker, ContinuityError> + Send>;
type StartedSince = fn(u64) -> Vec<StartedProcess>;

/// The one worker of this program, started with the first message.
fn worker() -> Option<Sender<Message>> {
    static WORKER: OnceLock<Mutex<Sender<Message>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            Mutex::new(start(
                Box::new(open_real_tracker),
                process_log::started_since,
                LOOK_EVERY,
            ))
        })
        .lock()
        .ok()
        .map(|sender| sender.clone())
}

/// What the gate read, by token, until the session of the started process
/// claims it.
static READS: Mutex<BTreeMap<u64, GateResult>> = Mutex::new(BTreeMap::new());

/// How many emulator starts are being held at the gate right now.
static STARTS_HELD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn claim_read(token: u64) -> Option<GateResult> {
    READS.lock().ok()?.remove(&token)
}

/// The before-start gate. Runs on whichever thread is about to create the
/// process, and returns only when the saves have been read or given up on.
fn gate(executable: &Path, arguments: &[String]) -> Option<u64> {
    use archivefs_core::save_continuity::discover::DiscoveryRoots;
    use archivefs_core::save_continuity::tracker::read_baseline;
    let launch = classify_started(executable, arguments);
    if !saves_known_for(&launch.emulator) {
        // Not an emulator whose saves are looked for: nothing to wait for.
        return None;
    }
    let roots = DiscoveryRoots::current()?;
    // Said while an emulator is held at the gate: its session waits for it
    // rather than take a helper of the same launch for the emulator.
    struct Held;
    impl Drop for Held {
        fn drop(&mut self) {
            STARTS_HELD.fetch_sub(1, Ordering::SeqCst);
        }
    }
    STARTS_HELD.fetch_add(1, Ordering::SeqCst);
    let _held = Held;
    Some(hold_for_read(GATE_PATIENCE, move |cancel| {
        read_baseline(&roots, &launch, Some(cancel))
            .map_err(|error| format!("{NOT_READ} ({error})"))
    }))
}

/// Runs `read` on a thread of its own and waits for it, at most `patience`.
/// Returns the token under which the outcome is kept: the read, if it
/// finished while the caller was still waiting, or else why there is none.
/// A read that finishes after the wait was given up is thrown away: by then
/// the emulator may exist.
fn hold_for_read(
    patience: Duration,
    read: impl FnOnce(&std::sync::atomic::AtomicBool) -> Result<Baseline, String> + Send + 'static,
) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    let began = Instant::now();
    let state = Arc::new(AtomicU8::new(PENDING));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (reply, done) = mpsc::channel::<()>();
    let reader = {
        let (state, cancel) = (Arc::clone(&state), Arc::clone(&cancel));
        std::thread::Builder::new()
            .name("save-baseline-before-start".to_string())
            .spawn(move || {
                let queued = began.elapsed().as_millis() as u64;
                let read = read(&cancel);
                let held = began.elapsed().as_millis() as u64;
                // Only a read that finished while the start was still held
                // counts. This is decided before the gate is released.
                let in_time = state
                    .compare_exchange(PENDING, DONE, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
                if in_time {
                    keep_read(token, read.map(|baseline| (baseline, held, queued)));
                }
                let _ = reply.send(());
            })
    };
    let why_not = match reader {
        Ok(_) => match done.recv_timeout(patience) {
            Ok(()) => None,
            Err(RecvTimeoutError::Timeout) => Some(TOO_SLOW.to_string()),
            // The reader ended without an answer.
            Err(RecvTimeoutError::Disconnected) => {
                Some(format!("{NOT_READ} (the read stopped unexpectedly)"))
            }
        },
        Err(_) => Some(format!("{NOT_READ} (the read could not be started)")),
    };
    if let Some(why_not) = why_not
        && state
            .compare_exchange(PENDING, ABANDONED, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        // Not waiting any longer; the reader is told to stop, and whatever
        // it still produces is not kept.
        cancel.store(true, Ordering::SeqCst);
        keep_read(token, Err(why_not));
    }
    token
}

fn keep_read(token: u64, read: GateResult) {
    if let Ok(mut reads) = READS.lock() {
        reads.insert(token, read);
        while reads.len() > 16 {
            // Reads no started process ever claimed (the start failed).
            reads.pop_first();
        }
    }
}

/// The interface thread's handle. Creating it sets the before-start gate.
pub(super) struct SessionTracking;

impl Default for SessionTracking {
    fn default() -> Self {
        // Blue's own tests drive the launch activity with made-up launches.
        // They must never open the real save-continuity folder or profile;
        // the worker is tested directly, with folders of its own.
        if !cfg!(test) {
            process_log::set_before_start(Arc::new(gate));
        }
        Self
    }
}

impl SessionTracking {
    /// A launch has just become active. `after_sequence` is the process
    /// log's next sequence number as it was **before** the launch could
    /// have started anything (the caller notes it while no launch is
    /// active): a launch can start its process before Blue first sees it
    /// as active.
    pub(super) fn launch_began(&mut self, after_sequence: u64, emulator: &str, game: SessionGame) {
        self.send(Message::Began {
            after_sequence,
            game,
            emulator: emulator.to_string(),
        });
    }

    /// Blue's launch activity is over (the process it started has ended, or
    /// none could be started).
    pub(super) fn launch_finished(&mut self, failure: Option<String>) {
        self.send(Message::Finished {
            failure,
            until_sequence: process_log::next_sequence(),
        });
    }

    fn send(&mut self, message: Message) {
        if cfg!(test) {
            drop(message);
            return;
        }
        // A worker that has gone away only means nothing is recorded.
        if let Some(worker) = worker() {
            let _ = worker.send(message);
        }
    }
}

/// The person's own save-continuity folder and profile.
///
/// Recording a launch creates the session journal's folder and this
/// installation's local id (a random number in `installation.json`, made
/// once and reused), and nothing
/// else: no snapshot store and no signing key. The key belongs to the
/// two-device feature and is created only when that is used.
fn open_real_tracker() -> Result<LaunchTracker, ContinuityError> {
    use archivefs_core::save_continuity::discover::DiscoveryRoots;
    let root = archivefs_core::save_continuity::default_root()
        .map_err(|error| ContinuityError::Io(error.to_string()))?;
    let device_id = archivefs_core::save_continuity::installation::open_or_create(&root)?;
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
    /// Set when the launch is over: only a process started before this
    /// belongs to it.
    until_sequence: Option<u64>,
    /// The started process has been passed to the tracker.
    started: bool,
    /// Which process of the log the session is bound to, once it is.
    bound: Option<u64>,
    /// Looks spent waiting for a process the gate held to appear, while
    /// only helpers of the launch had been started.
    waited_for_emulator: u32,
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
            // Until a launch's process has been seen, look often: the
            // process log is in memory and the look costs nothing, and the
            // session's start should be noted when it happens, not at the
            // next slow look.
            let awaiting_process = sessions
                .iter()
                .any(|session| !session.started && session.over.is_none());
            let wait = if waiting_for_status || awaiting_process {
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
        // Everything already sent is taken in before any launch is followed:
        // a launch that is already over must be known to be over (and where
        // its processes end) before a later launch's process is looked at.
        let mut next = message;
        while let Some(message) = next {
            take(tracker, &mut sessions, message);
            next = receiver.try_recv().ok();
        }
        sessions.retain_mut(|session| follow(tracker, session, started_since));
        if sessions.iter().any(|session| session.launcher_done) {
            // Ends the sessions whose emulator has gone since.
            tracker.settle();
        }
    }
}

/// Notes one message.
fn take(tracker: &LaunchTracker, sessions: &mut Vec<Open>, message: Message) {
    match message {
        Message::Began {
            after_sequence,
            game,
            emulator,
        } => {
            if let Ok(record) = tracker.begin_gated(game, &emulator) {
                sessions.push(Open {
                    session_id: record.session_id,
                    after_sequence,
                    until_sequence: None,
                    started: false,
                    bound: None,
                    waited_for_emulator: 0,
                    over: None,
                    launcher_done: false,
                    waited_for_status: 0,
                });
            }
        }
        Message::Finished {
            failure,
            until_sequence,
        } => {
            // The launch that is over is the oldest one not yet over:
            // launches do not overlap, and messages arrive in order.
            if let Some(session) = sessions.iter_mut().find(|session| session.over.is_none()) {
                session.over = Some(failure);
                session.until_sequence = Some(until_sequence);
            }
        }
    }
}

/// One step for one launch. Returns whether to keep following it.
fn follow(tracker: &LaunchTracker, session: &mut Open, started_since: StartedSince) -> bool {
    // The first process started inside this launch's own span. Never one
    // started after the launch was over: that is a later launch's.
    // Of those, one the gate held (an emulator whose saves were read for
    // it) before any helper the launch may also have started.
    let in_span: Vec<StartedProcess> = started_since(session.after_sequence)
        .into_iter()
        .filter(|process| {
            session
                .until_sequence
                .is_none_or(|until| process.sequence < until)
        })
        .collect();
    // Once bound, a session stays with its process.
    let process = match session.bound {
        Some(sequence) => in_span.iter().find(|process| process.sequence == sequence),
        None => {
            let held = in_span.iter().find(|process| process.gate_token.is_some());
            if STARTS_HELD.load(Ordering::SeqCst) > 0 {
                // An emulator is at the gate: the short wait below starts
                // over once it has been let through.
                session.waited_for_emulator = 0;
            }
            if held.is_none()
                && !in_span.is_empty()
                && session.over.is_none()
                && (session.waited_for_emulator < EMULATOR_PATIENCE
                    || STARTS_HELD.load(Ordering::SeqCst) > 0)
            {
                // Only a helper so far: give the emulator a moment.
                session.waited_for_emulator += 1;
                return true;
            }
            held.or(in_span.first())
        }
    }
    .cloned();
    if !session.started
        && let Some(process) = &process
    {
        session.started = true;
        session.bound = Some(process.sequence);
        // What the gate read for this very start, if anything.
        let read = match process.gate_token.and_then(claim_read) {
            Some(read) => read,
            None if !saves_known_for(
                &classify_started(&process.executable, &process.arguments).emulator,
            ) =>
            {
                Err(NOT_COVERED.to_string())
            }
            None => Err(NOT_READ.to_string()),
        };
        if tracker
            .process_started_gated(&session.session_id, process, read, &ProcFs)
            .is_err()
        {
            // The record may already say "started" (the process was noted
            // before the slower look at the saves failed). Keep following
            // it then, so its end is still seen.
            return tracker
                .look_again(&session.session_id)
                .is_ok_and(|record| record.state == SessionState::Started);
        }
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
        Ok(_) => false,
        // Could not be recorded as ended now. Keep looking: the session is
        // then settled as soon as its processes are seen gone.
        Err(_) => {
            session.launcher_done = true;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::process_log::ExitSummary;
    use archivefs_core::save_continuity::discover::DiscoveryRoots;
    use archivefs_core::save_continuity::session::{
        SessionEnd, SessionJournal, SessionLaunch, SessionRecord,
    };
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
            gate_token: None,
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
        worker
            .send(Message::Finished {
                failure: None,
                until_sequence: u64::MAX,
            })
            .unwrap();
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
                until_sequence: 9_000_002,
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
    fn a_failed_launch_never_adopts_the_next_launchs_process() {
        // Launch 1 fails before starting anything. The person launches
        // again at once; that launch starts a process. All three messages
        // are waiting while the worker is still busy with the first.
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
        std::fs::create_dir_all(&home).unwrap();
        const SEQUENCE: u64 = 9_000_005;
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
            gate_token: None,
        });
        let worker = tracking(&root, &home, Duration::from_millis(40));
        let named = |title: &str| SessionGame {
            title: title.into(),
            ..Default::default()
        };
        worker
            .send(Message::Began {
                after_sequence: SEQUENCE,
                game: named("First, failed"),
                emulator: "PCSX2".into(),
            })
            .unwrap();
        worker
            .send(Message::Finished {
                failure: Some("the BIOS is missing".into()),
                // Over before the process with this number was started.
                until_sequence: SEQUENCE,
            })
            .unwrap();
        worker
            .send(Message::Began {
                after_sequence: SEQUENCE,
                game: named("Second, running"),
                emulator: "PCSX2".into(),
            })
            .unwrap();
        until("both launches are settled as they really were", || {
            let all = sessions(&root);
            all.len() == 2
                && all
                    .iter()
                    .all(|session| session.state != SessionState::Requested)
        });
        let all = sessions(&root);
        let by = |title: &str| all.iter().find(|s| s.game.title == title).unwrap();
        assert_eq!(by("First, failed").state, SessionState::NeverStarted);
        assert_eq!(by("First, failed").process_id, None);
        assert_eq!(by("Second, running").state, SessionState::Started);
        assert_eq!(by("Second, running").process_id, Some(child.id()));
        child.kill().unwrap();
        child.wait().unwrap();
        drop(worker);
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
            worker
                .send(Message::Finished {
                    failure: None,
                    until_sequence: u64::MAX,
                })
                .unwrap();
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
            gate_token: None,
        });
        until("the worker noticed the process by itself", || {
            sessions(&root)[0].state == SessionState::Started
        });
        child.kill().unwrap();
        child.wait().unwrap();
        drop(worker);
    }
    fn home_with_saves(home: &Path) -> PathBuf {
        let config = home.join(".config/retroarch");
        std::fs::create_dir_all(config.join("saves")).unwrap();
        std::fs::write(
            config.join("retroarch.cfg"),
            "savefile_directory = \"default\"\n",
        )
        .unwrap();
        std::fs::write(config.join("saves/Played (USA).srm"), [1u8; 8192]).unwrap();
        std::fs::write(config.join("saves/Untouched (USA).srm"), [2u8; 8192]).unwrap();
        config.join("saves")
    }

    fn retroarch() -> SessionLaunch {
        classify_started(Path::new("/nowhere/retroarch"), &[])
    }

    fn read_of(
        home: &Path,
    ) -> impl FnOnce(&std::sync::atomic::AtomicBool) -> Result<Baseline, String> + Send + 'static
    {
        use archivefs_core::save_continuity::tracker::read_baseline;
        let roots = DiscoveryRoots::under_home(home);
        move |cancel| {
            read_baseline(&roots, &retroarch(), Some(cancel)).map_err(|error| error.to_string())
        }
    }

    /// Runs one launch through the worker: `gate` is what happens before
    /// the "emulator" starts (it gets the home folder and gives the token,
    /// or none), and the "emulator" writes one save and makes another once
    /// it has been started.
    fn gated_launch(sequence: u64, gate: impl FnOnce(&Path) -> Option<u64>) -> SessionRecord {
        let dir = tempfile::tempdir().unwrap();
        let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
        let saves = home_with_saves(&home);
        let worker = tracking(&root, &home, Duration::from_millis(50));
        let gate_token = gate(&home);
        // Only now does the emulator exist, and write.
        std::fs::write(saves.join("Played (USA).srm"), [9u8; 8192]).unwrap();
        std::fs::write(saves.join("Brand New (USA).srm"), [7u8; 8192]).unwrap();
        worker
            .send(Message::Began {
                after_sequence: sequence,
                game: game(),
                emulator: "RetroArch".into(),
            })
            .unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut process = StartedProcess {
            sequence,
            pid: child.id(),
            start_ticks: ticks(child.id()),
            executable: PathBuf::from("/nowhere/retroarch"),
            arguments: Vec::new(),
            exit: None,
            gate_token,
        };
        report(process.clone());
        until("the process is recorded", || {
            sessions(&root)
                .first()
                .is_some_and(|record| record.state == SessionState::Started)
        });
        child.kill().unwrap();
        child.wait().unwrap();
        process.exit = Some(ExitSummary::Code(0));
        report(process);
        worker
            .send(Message::Finished {
                failure: None,
                until_sequence: u64::MAX,
            })
            .unwrap();
        until("the end is recorded", || {
            sessions(&root)[0].state == SessionState::Ended
        });
        sessions(&root).remove(0)
    }

    #[test]
    fn saves_read_while_the_start_is_held_are_the_starting_content() {
        use archivefs_core::save_continuity::session::{SaveChange, SaveTracking};
        let ended = gated_launch(9_000_010, |home| {
            Some(hold_for_read(GATE_PATIENCE, read_of(home)))
        });
        assert!(
            matches!(ended.save_tracking, Some(SaveTracking::Established { .. })),
            "{:?}",
            ended.save_tracking
        );
        // Both saves as they were before the "emulator" wrote.
        assert_eq!(ended.saves_before.len(), 2);
        let mut changes = ended.save_changes.clone();
        changes.sort_by_key(|change| format!("{change:?}"));
        assert_eq!(
            changes,
            vec![
                SaveChange::Created {
                    label: "Brand New (USA).srm".into()
                },
                SaveChange::Modified {
                    label: "Played (USA).srm".into()
                },
            ]
        );
    }

    fn untracked(ended: &SessionRecord, reason: &str) {
        use archivefs_core::save_continuity::session::SaveTracking;
        match &ended.save_tracking {
            Some(SaveTracking::Unavailable { reason: given }) => {
                assert!(given.starts_with(reason), "{given}")
            }
            other => panic!("{other:?}"),
        }
        // Nothing is said about any save, changed or unchanged.
        assert!(ended.saves_before.is_empty());
        assert!(ended.saves_after.is_none());
        assert!(ended.save_changes.is_empty());
    }

    #[test]
    fn without_a_read_before_the_start_nothing_is_said_about_the_saves() {
        // The gate gave no token at all.
        untracked(&gated_launch(9_000_011, |_| None), NOT_READ);

        // The read failed.
        untracked(
            &gated_launch(9_000_013, |_| {
                Some(hold_for_read(GATE_PATIENCE, |_| {
                    Err(format!("{NOT_READ} (made to fail)"))
                }))
            }),
            NOT_READ,
        );
    }

    #[test]
    fn a_read_still_running_when_the_wait_is_over_is_thrown_away() {
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ended = {
            let finished = Arc::clone(&finished);
            gated_launch(9_000_012, move |home| {
                let read = read_of(home);
                let began = Instant::now();
                // The read takes longer than the start is held for.
                let token = hold_for_read(Duration::from_millis(100), move |cancel| {
                    std::thread::sleep(Duration::from_millis(3000));
                    let read = read(&std::sync::atomic::AtomicBool::new(false));
                    assert!(cancel.load(Ordering::SeqCst), "the reader was told to stop");
                    finished.store(true, Ordering::SeqCst);
                    read
                });
                // The start was released at the end of the wait, not of the read.
                assert!(began.elapsed() < Duration::from_millis(2500));
                Some(token)
            })
        };
        untracked(&ended, TOO_SLOW);
        // The late read did complete, with a real baseline, and was not used.
        until("the late read finishes", || finished.load(Ordering::SeqCst));
        std::thread::sleep(Duration::from_millis(50));
    }

    #[test]
    fn the_read_before_a_start_does_not_wait_for_session_bookkeeping() {
        // A worker that is busy for a long time (its journal takes seconds
        // to open) does not delay the read: the read has its own thread.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        home_with_saves(&home);
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            work(
                receiver,
                Box::new(|| {
                    std::thread::sleep(Duration::from_secs(3));
                    Err(ContinuityError::Refused("no folder".into()))
                }),
                started_since,
                Duration::from_millis(50),
            )
        });
        sender
            .send(Message::Began {
                after_sequence: 9_000_014,
                game: game(),
                emulator: "RetroArch".into(),
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let began = Instant::now();
        let token = hold_for_read(GATE_PATIENCE, read_of(&home));
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "the start was held"
        );
        assert!(matches!(claim_read(token), Some(Ok(_))));
    }

    #[test]
    fn a_read_that_dies_releases_the_start_at_once_and_claims_nothing() {
        let began = Instant::now();
        let token = hold_for_read(GATE_PATIENCE, |_| panic!("made to fail"));
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the start was held"
        );
        match claim_read(token) {
            Some(Err(reason)) => assert!(reason.starts_with(NOT_READ), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
}
