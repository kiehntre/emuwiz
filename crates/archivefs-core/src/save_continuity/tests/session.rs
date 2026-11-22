use super::super::ContinuityError;
use super::super::session::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A process table a test can change: pid -> (start ticks, parent).
#[derive(Default)]
struct Table(RefCell<BTreeMap<u32, (u64, u32)>>);

impl Table {
    fn start(&self, pid: u32, ticks: u64, parent: u32) {
        self.0.borrow_mut().insert(pid, (ticks, parent));
    }
    fn exit(&self, pid: u32) {
        self.0.borrow_mut().remove(&pid);
    }
}

impl ProcessTable for Table {
    fn start_ticks(&self, pid: u32) -> Option<u64> {
        self.0.borrow().get(&pid).map(|(ticks, _)| *ticks)
    }
    fn executable(&self, pid: u32) -> Option<PathBuf> {
        Some(PathBuf::from(format!("/usr/bin/process-{pid}")))
    }
    fn descendants(&self, pid: u32) -> Vec<u32> {
        let table = self.0.borrow();
        let mut found = vec![pid];
        let mut index = 0;
        while index < found.len() {
            let current = found[index];
            for (child, (_, parent)) in table.iter() {
                if *parent == current && !found.contains(child) {
                    found.push(*child);
                }
            }
            index += 1;
        }
        found.remove(0);
        found
    }
}

fn journal() -> (tempfile::TempDir, SessionJournal) {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path(), "device-a").unwrap();
    (dir, journal)
}

fn game() -> SessionGame {
    SessionGame {
        library_id: Some(7),
        title: "Manhunt".into(),
        product_code: Some("SLUS-20827".into()),
        content_path: None,
    }
}

fn launch(through_launcher: bool) -> SessionLaunch {
    SessionLaunch {
        emulator: "ProbeEmu".into(),
        profile: "native".into(),
        core: None,
        emulator_version: None,
        through_launcher,
    }
}

fn mark(id: &str, sha: &str) -> SaveMark {
    SaveMark {
        save_id: id.into(),
        label: format!("save {id}"),
        content_sha256: sha.into(),
        last_changed_unix: None,
        before_launch: None,
        folder: None,
    }
}

#[test]
fn a_request_is_not_a_session_and_a_started_process_is_not_proof_of_play() {
    let (_dir, journal) = journal();
    let table = Table::default();
    let requested = journal
        .request(game(), launch(false), vec![mark("a", "1")])
        .unwrap();
    assert_eq!(requested.state, SessionState::Requested);
    assert_eq!(requested.play_evidence, PlayEvidence::None);
    assert_eq!(requested.status_line(), "Starting…");

    table.start(500, 111, 1);
    let started = journal
        .mark_started(&requested.session_id, 500, &table)
        .unwrap();
    assert_eq!(started.state, SessionState::Started);
    assert_eq!(
        started.play_evidence,
        PlayEvidence::None,
        "running is not playing"
    );
    assert_eq!(started.processes[0].start_ticks, 111);
    assert!(started.processes[0].launched_by_emuwiz);

    table.exit(500);
    // The save is byte-identical afterwards: the process ran, nothing more is claimed.
    let ended = journal
        .mark_ended(
            &requested.session_id,
            ExitObservation::Code(0),
            Some(&[mark("a", "1")]),
            &table,
        )
        .unwrap();
    assert_eq!(ended.state, SessionState::Ended);
    assert_eq!(ended.end, Some(SessionEnd::Clean));
    assert!(matches!(
        ended.play_evidence,
        PlayEvidence::ProcessRan { .. }
    ));
    assert!(ended.save_changes.is_empty());
    assert!(!ended.needs_attention());
}

#[test]
fn save_changes_are_what_differs_in_content_not_in_date() {
    let (_dir, journal) = journal();
    let table = Table::default();
    let session = journal
        .request(
            game(),
            launch(false),
            vec![mark("kept", "1"), mark("changed", "2"), mark("gone", "3")],
        )
        .unwrap();
    table.start(1, 1, 0);
    journal
        .mark_started(&session.session_id, 1, &table)
        .unwrap();
    table.exit(1);
    let after = [mark("kept", "1"), mark("changed", "2b"), mark("new", "9")];
    let ended = journal
        .mark_ended(
            &session.session_id,
            ExitObservation::Code(0),
            Some(&after),
            &table,
        )
        .unwrap();
    assert_eq!(
        ended.save_changes,
        vec![
            SaveChange::Modified {
                label: "save changed".into()
            },
            SaveChange::Created {
                label: "save new".into()
            },
            SaveChange::Missing {
                label: "save gone".into()
            },
        ]
    );
    assert!(matches!(
        ended.play_evidence,
        PlayEvidence::SaveChanged { .. }
    ));
    assert!(
        ended.needs_attention(),
        "a save that disappeared is worth a look"
    );
}

#[test]
fn crashes_and_error_exits_are_recorded_as_such() {
    for (exit, expected, abnormal) in [
        (
            ExitObservation::Signal {
                signal: 11,
                core_dumped: true,
            },
            SessionEnd::Signal {
                signal: 11,
                core_dumped: true,
            },
            true,
        ),
        (
            ExitObservation::Signal {
                signal: 15,
                core_dumped: false,
            },
            SessionEnd::Signal {
                signal: 15,
                core_dumped: false,
            },
            true,
        ),
        (
            ExitObservation::Code(3),
            SessionEnd::ExitCode { code: 3 },
            true,
        ),
        (
            ExitObservation::Unavailable,
            SessionEnd::StatusUnavailable,
            false,
        ),
    ] {
        let (_dir, journal) = journal();
        let table = Table::default();
        let session = journal.request(game(), launch(false), Vec::new()).unwrap();
        table.start(1, 1, 0);
        journal
            .mark_started(&session.session_id, 1, &table)
            .unwrap();
        table.exit(1);
        let ended = journal
            .mark_ended(&session.session_id, exit, None, &table)
            .unwrap();
        assert_eq!(ended.end, Some(expected));
        assert_eq!(ended.needs_attention(), abnormal);
    }
}

#[test]
fn a_launch_that_never_produced_a_process_is_settled_as_never_started() {
    let (_dir, journal) = journal();
    let table = Table::default();
    let session = journal.request(game(), launch(false), Vec::new()).unwrap();
    assert!(
        journal
            .reconcile(&table, session.requested_unix + 10)
            .is_empty(),
        "still within the grace period"
    );
    let settled = journal.reconcile(&table, session.requested_unix + START_GRACE_SECONDS + 1);
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].state, SessionState::NeverStarted);
    assert_eq!(settled[0].status_line(), "Did not start");
    // A failure reported by the launcher itself is recorded at once.
    let other = journal.request(game(), launch(false), Vec::new()).unwrap();
    let failed = journal
        .mark_never_started(&other.session_id, "the emulator could not be found")
        .unwrap();
    assert_eq!(failed.state, SessionState::NeverStarted);
}

#[test]
fn a_session_emuwiz_did_not_see_end_is_never_called_clean() {
    let (_dir, journal) = journal();
    let table = Table::default();
    let session = journal.request(game(), launch(false), Vec::new()).unwrap();
    table.start(700, 5000, 1);
    journal
        .mark_started(&session.session_id, 700, &table)
        .unwrap();

    assert!(journal.reconcile(&table, 0).is_empty(), "still running");
    // EmuWiz is restarted; meanwhile the emulator died and its id was
    // given to an unrelated process (different start time).
    table.exit(700);
    table.start(700, 9999, 1);
    let settled = journal.reconcile(&table, 0);
    assert_eq!(settled.len(), 1);
    let record = &settled[0];
    assert_eq!(record.state, SessionState::EndNotSeen);
    assert_eq!(record.end, None);
    assert!(record.needs_attention());
    assert!(record.status_line().contains("not known"));
    assert!(
        matches!(
            journal.mark_ended(&session.session_id, ExitObservation::Code(0), None, &table),
            Err(ContinuityError::Refused(_))
        ),
        "a later 'clean exit' for the recycled id is not accepted"
    );
}

#[test]
fn a_launcher_finishing_does_not_end_the_session_while_the_emulator_runs() {
    let (_dir, journal) = journal();
    let table = Table::default();
    let session = journal
        .request(game(), launch(true), vec![mark("a", "1")])
        .unwrap();
    table.start(100, 10, 1); // flatpak run
    journal
        .mark_started(&session.session_id, 100, &table)
        .unwrap();
    table.start(101, 11, 100); // bwrap
    table.start(102, 12, 101); // the emulator, started after the session was recorded
    let seen = journal
        .observe_running(&session.session_id, &table)
        .unwrap();
    assert_eq!(
        seen.processes.len(),
        3,
        "a look a moment later finds what the launcher started"
    );

    table.exit(100);
    let after_launcher = journal
        .mark_ended(
            &session.session_id,
            ExitObservation::Code(0),
            Some(&[mark("a", "2")]),
            &table,
        )
        .unwrap();
    assert_eq!(
        after_launcher.state,
        SessionState::Started,
        "the emulator is still running"
    );
    assert_eq!(
        after_launcher.processes.len(),
        3,
        "descendants found late are still tracked"
    );
    assert!(
        after_launcher.save_changes.is_empty(),
        "saves are not compared while it runs"
    );
    assert!(journal.reconcile(&table, 0).is_empty());

    table.exit(101);
    table.exit(102);
    let settled = journal.reconcile(&table, 50);
    assert_eq!(settled[0].state, SessionState::Ended);
    assert_eq!(
        settled[0].end,
        Some(SessionEnd::LauncherExitedOk),
        "only the launcher's result is known"
    );
}

#[test]
fn a_launcher_exit_code_zero_is_not_reported_as_the_emulators() {
    let (_dir, journal) = journal();
    let table = Table::default();
    let session = journal.request(game(), launch(true), Vec::new()).unwrap();
    table.start(1, 1, 0);
    journal
        .mark_started(&session.session_id, 1, &table)
        .unwrap();
    table.exit(1);
    let ended = journal
        .mark_ended(&session.session_id, ExitObservation::Code(0), None, &table)
        .unwrap();
    assert_eq!(ended.end, Some(SessionEnd::LauncherExitedOk));
}

#[test]
fn recent_sessions_come_newest_first_and_survive_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let ids: Vec<String> = {
        let journal = SessionJournal::open(dir.path(), "device-a").unwrap();
        (0..5)
            .map(|_| {
                journal
                    .request(game(), launch(false), Vec::new())
                    .unwrap()
                    .session_id
            })
            .collect()
    };
    let journal = SessionJournal::open(dir.path(), "device-a").unwrap();
    assert_eq!(journal.all().len(), 5);
    assert_eq!(journal.recent(2).len(), 2);
    let linked = journal.link_snapshot(&ids[0], "abc").unwrap();
    assert_eq!(linked.snapshots, vec!["abc".to_string()]);
    // A half-written record from a crash is skipped, not fatal.
    std::fs::write(
        dir.path().join("sessions/999999999999-broken.json"),
        b"{ not json",
    )
    .unwrap();
    assert_eq!(journal.all().len(), 5);
}

#[test]
fn the_real_process_table_knows_this_process_and_its_children() {
    let me = std::process::id();
    let ticks = ProcFs.start_ticks(me).expect("own start time");
    assert_eq!(ProcFs.start_ticks(me), Some(ticks));
    assert!(ProcFs.executable(me).is_some());
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    assert!(ProcFs.descendants(me).contains(&child.id()));
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(
        ProcFs.start_ticks(child.id()),
        None,
        "a reaped process is gone"
    );
}

#[test]
fn a_flatpak_app_scope_keeps_the_session_open_after_its_launcher_is_gone() {
    /// Processes 201 and 202 share an app scope; nobody is their parent any more.
    struct Scoped(Table);
    impl ProcessTable for Scoped {
        fn start_ticks(&self, pid: u32) -> Option<u64> {
            self.0.start_ticks(pid)
        }
        fn app_scope(&self, pid: u32) -> Option<String> {
            self.0
                .start_ticks(pid)
                .map(|_| "app-flatpak-org.duckstation.DuckStation-42.scope".to_string())
        }
        fn in_scope(&self, _: &str) -> Vec<u32> {
            self.0.0.borrow().keys().copied().collect()
        }
    }
    let (_dir, journal) = journal();
    let table = Scoped(Table::default());
    let session = journal.request(game(), launch(true), Vec::new()).unwrap();
    table.0.start(200, 1, 1);
    let started = journal
        .mark_started(&session.session_id, 200, &table)
        .unwrap();
    assert!(started.app_scope.is_some());
    // The launcher exits before EmuWiz ever looked again; the emulator
    // (a re-parented process in the same scope) appears afterwards.
    table.0.exit(200);
    table.0.start(202, 9, 1);
    let open = journal
        .mark_ended(&session.session_id, ExitObservation::Code(0), None, &table)
        .unwrap();
    assert_eq!(open.state, SessionState::Started);
    table.0.exit(202);
    assert_eq!(journal.reconcile(&table, 9)[0].state, SessionState::Ended);
}
