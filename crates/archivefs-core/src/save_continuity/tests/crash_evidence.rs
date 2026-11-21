//! The read-only crash-record lookup, against synthetic log files, and its
//! use when a sandboxed emulator's session is settled.

use super::super::crash_evidence::*;
use super::super::session::*;
use super::super::status::{Advisory, PrecedingSession};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// The synthetic logs are written in UTC, so this is the whole conversion.
fn utc(time: &LocalTime) -> Option<i64> {
    let (y, m, d) = (
        i64::from(time.year),
        i64::from(time.month),
        i64::from(time.day),
    );
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(
        days * 86_400
            + i64::from(time.hour) * 3600
            + i64::from(time.minute) * 60
            + i64::from(time.second),
    )
}

/// 2026-11-20 23:30:00 UTC.
const T0: i64 = 1_795_217_400;

fn stamp(offset: i64) -> String {
    let t = T0 + offset;
    let secs = t.rem_euclid(86_400);
    // T0 is 23:30 on the 20th; days before it count down (Euclidean division).
    let day = 20 + (t - T0 + 84_600).div_euclid(86_400);
    format!(
        "2026-11-{day:02} {:02}:{:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn crashed(offset: i64, pid: u32) -> String {
    format!(
        "ERROR: apport (pid 999) {},940: host pid {pid} crashed in a container without apport support\n",
        stamp(offset)
    )
}

fn other(offset: i64) -> String {
    format!(
        "ERROR: apport (pid 999) {},100: environment: environ({{'LC_CTYPE': 'C.UTF-8'}})\n",
        stamp(offset)
    )
}

struct Files {
    dir: tempfile::TempDir,
}

impl Files {
    fn new(pattern: &str, rotated: Option<&str>, current: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("core_pattern"), pattern).unwrap();
        // As on the development machine: the kernel waits for the handler.
        fs::write(dir.path().join("core_pipe_limit"), "10\n").unwrap();
        if let Some(text) = rotated {
            fs::write(dir.path().join("apport.log.1"), text).unwrap();
        }
        if let Some(text) = current {
            fs::write(dir.path().join("apport.log"), text).unwrap();
        }
        Self { dir }
    }

    fn source(&self) -> ApportLog {
        ApportLog {
            core_pattern: self.dir.path().join("core_pattern"),
            core_pipe_limit: self.dir.path().join("core_pipe_limit"),
            logs: vec![
                self.dir.path().join("apport.log.1"),
                self.dir.path().join("apport.log"),
            ],
            to_unix: utc,
        }
    }
}

const APPORT: &str = "|/usr/share/apport/apport -p%p -s%s -c%c -d%d -P%P -u%u -g%g -F%F -- %E\n";

/// The emulator: pid 4242, alive from T0+100 to T0+400.
fn emulator() -> ProcessLifetime {
    ProcessLifetime {
        pid: 4242,
        started_unix: T0 + 100,
        gone_by_unix: T0 + 400,
    }
}

#[test]
fn the_time_arithmetic_of_this_test_file_is_right() {
    assert_eq!(
        utc(&LocalTime {
            year: 2026,
            month: 11,
            day: 20,
            hour: 23,
            minute: 30,
            second: 0
        }),
        Some(T0)
    );
    assert_eq!(stamp(0), "2026-11-20 23:30:00");
    assert_eq!(stamp(1800), "2026-11-21 00:00:00");
    assert_eq!(stamp(-60), "2026-11-20 23:29:00");
    assert_eq!(stamp(-86_000), "2026-11-19 23:36:40");
    assert_eq!(stamp(40_000), "2026-11-21 10:36:40");
}

#[test]
fn a_record_for_exactly_this_process_inside_its_lifetime_is_a_confirmed_fatal_termination() {
    let log = other(0) + &crashed(250, 4242) + &other(500);
    let files = Files::new(APPORT, None, Some(&log));
    let found = files.source().lookup(&emulator());
    assert_eq!(
        found,
        CrashEvidence::FatalTerminationRecorded {
            recorded_at: stamp(250),
            source: "the system's crash handler log".to_string()
        }
    );
    assert!(found.confirmed());
    // At the very edges of the lifetime (the handler writes a moment after).
    for offset in [100 - LIFETIME_SLACK_SECONDS, 400 + LIFETIME_SLACK_SECONDS] {
        let log = other(0) + &crashed(offset, 4242);
        assert!(
            Files::new(APPORT, None, Some(&log))
                .source()
                .lookup(&emulator())
                .confirmed(),
            "{offset}"
        );
    }
    // In the rotated file.
    let files = Files::new(
        APPORT,
        Some(&(other(0) + &crashed(250, 4242))),
        Some(&other(600)),
    );
    assert!(files.source().lookup(&emulator()).confirmed());
}

#[test]
fn another_processs_record_is_not_this_process() {
    // Other things died in the same sandbox at the same time: a helper, a
    // shell, another emulator. None of them has this process id.
    let log = other(0)
        + &crashed(240, 4241)
        + &crashed(250, 4243)
        + &crashed(260, 42420)
        + &crashed(270, 424);
    let files = Files::new(APPORT, None, Some(&log));
    assert_eq!(
        files.source().lookup(&emulator()),
        CrashEvidence::NoRecord {
            source: "the system's crash handler log".to_string()
        }
    );
}

#[test]
fn a_stale_record_or_a_reused_process_id_is_not_this_process() {
    // The same id, but before this process existed: an earlier process.
    let before = other(0) + &crashed(50, 4242);
    assert!(
        !Files::new(APPORT, None, Some(&before))
            .source()
            .lookup(&emulator())
            .confirmed()
    );
    // The same id, long after this process was seen gone: a later process.
    let after = other(0) + &crashed(4000, 4242);
    assert!(
        !Files::new(APPORT, None, Some(&after))
            .source()
            .lookup(&emulator())
            .confirmed()
    );
    // Yesterday's record with this id in the rotated file.
    let rotated = crashed(-80_000, 4242);
    let files = Files::new(APPORT, Some(&rotated), Some(&other(0)));
    assert_eq!(
        files.source().lookup(&emulator()),
        CrashEvidence::NoRecord {
            source: "the system's crash handler log".to_string()
        }
    );
    // Two records with this id inside the lifetime: one process ends once,
    // so neither is believed.
    let twice = other(0) + &crashed(200, 4242) + &crashed(300, 4242);
    assert!(matches!(
        Files::new(APPORT, None, Some(&twice))
            .source()
            .lookup(&emulator()),
        CrashEvidence::Unavailable { .. }
    ));
}

#[test]
fn an_unavailable_source_is_unknown_never_an_error_and_never_no_record() {
    let unavailable = |files: Files| {
        matches!(
            files.source().lookup(&emulator()),
            CrashEvidence::Unavailable { .. }
        )
    };
    let log = other(0) + &crashed(250, 4242);
    // Another crash handler (most other distributions), even with an old
    // apport log lying around that names this very process.
    assert!(unavailable(Files::new(
        "|/usr/lib/systemd/systemd-coredump %P %u %g %s %t %c %h\n",
        None,
        Some(&log)
    )));
    assert!(unavailable(Files::new("core\n", None, Some(&log))));
    // No core_pattern file at all (not Linux, or hidden).
    let files = Files::new(APPORT, None, Some(&log));
    fs::remove_file(files.dir.path().join("core_pattern")).unwrap();
    assert!(unavailable(files));
    // No log files.
    assert!(unavailable(Files::new(APPORT, None, None)));
    // A log this user may not read.
    if unsafe { libc::geteuid() } != 0 {
        use std::os::unix::fs::PermissionsExt;
        let files = Files::new(APPORT, None, Some(&log));
        let path = files.dir.path().join("apport.log");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        assert!(unavailable(files));
    }
    // A log that does not reach back to when the process started: an entry
    // may have been rotated away, so "no record" cannot be said.
    assert!(unavailable(Files::new(APPORT, None, Some(&other(300)))));
    assert!(unavailable(Files::new(APPORT, None, Some(""))));
    // A lifetime that makes no sense.
    let files = Files::new(APPORT, None, Some(&log));
    let backwards = ProcessLifetime {
        pid: 4242,
        started_unix: T0 + 400,
        gone_by_unix: T0 + 100,
    };
    assert!(matches!(
        files.source().lookup(&backwards),
        CrashEvidence::Unavailable { .. }
    ));
}

#[test]
fn malformed_evidence_is_unknown_not_guessed_at() {
    let unavailable = |log: String| {
        matches!(
            Files::new(APPORT, None, Some(&log))
                .source()
                .lookup(&emulator()),
            CrashEvidence::Unavailable { .. }
        )
    };
    // The line names this process but its time cannot be read.
    assert!(unavailable(
        other(0)
            + "ERROR: apport (pid 9) 2026-11-2X 23:34:10,1: host pid 4242 crashed in a container\n"
    ));
    assert!(unavailable(
        other(0)
            + "ERROR: apport (pid 9) 2026-13-45 99:99:99,1: host pid 4242 crashed in a container\n"
    ));
    assert!(unavailable(
        other(0) + "garbage host pid 4242 crashed somewhere\n"
    ));
    // Malformed lines about other things are skipped and change nothing.
    let noisy = other(0)
        + "Traceback (most recent call last):\n"
        + "  File \"/usr/share/apport/apport\", line 820, in main\n"
        + "ERROR: apport (pid 9) not-a-time: host pid 7 crashed\n"
        + "\u{0}\u{1}binary junk\n"
        + &crashed(250, 4242);
    assert!(
        Files::new(APPORT, None, Some(&noisy))
            .source()
            .lookup(&emulator())
            .confirmed()
    );
    // A pid that merely starts with the same digits, or text that is not
    // the handler's own "host pid N crashed" form, is not a match.
    let near = other(0)
        + "ERROR: apport (pid 4242) 2026-11-20 23:34:10,1: called for pid 77, signal 11\n"
        + "ERROR: apport (pid 9) 2026-11-20 23:34:10,1: executable was host pid 4242 crashed-looking text\n";
    assert_eq!(
        Files::new(APPORT, None, Some(&near))
            .source()
            .lookup(&emulator()),
        CrashEvidence::NoRecord {
            source: "the system's crash handler log".to_string()
        }
    );
}

// ---- Used when a session is settled ----

#[derive(Clone)]
struct Proc {
    ticks: u64,
    parent: u32,
    name: &'static str,
    exe: &'static str,
    scope: Option<&'static str>,
}

#[derive(Default)]
struct Machine(RefCell<BTreeMap<u32, Proc>>, Cell<Option<(u64, u64)>>);

impl Machine {
    fn start(
        &self,
        pid: u32,
        parent: u32,
        name: &'static str,
        exe: &'static str,
        scope: Option<&'static str>,
    ) {
        self.0.borrow_mut().insert(
            pid,
            Proc {
                ticks: u64::from(pid) * 10,
                parent,
                name,
                exe,
                scope,
            },
        );
    }
    fn exit(&self, pids: &[u32]) {
        for pid in pids {
            self.0.borrow_mut().remove(pid);
        }
    }
}

impl ProcessTable for Machine {
    fn process_ids_issued(&self) -> Option<(u64, u64)> {
        self.1.get()
    }
    fn start_ticks(&self, pid: u32) -> Option<u64> {
        self.0.borrow().get(&pid).map(|p| p.ticks)
    }
    fn executable(&self, pid: u32) -> Option<PathBuf> {
        self.0.borrow().get(&pid).map(|p| PathBuf::from(p.exe))
    }
    fn parent(&self, pid: u32) -> Option<u32> {
        self.0.borrow().get(&pid).map(|p| p.parent)
    }
    fn name(&self, pid: u32) -> Option<String> {
        self.0.borrow().get(&pid).map(|p| p.name.to_string())
    }
    fn app_scope(&self, pid: u32) -> Option<String> {
        self.0
            .borrow()
            .get(&pid)
            .and_then(|p| p.scope)
            .map(str::to_string)
    }
    fn in_scope(&self, scope: &str) -> Vec<u32> {
        self.0
            .borrow()
            .iter()
            .filter(|(_, p)| p.scope == Some(scope))
            .map(|(pid, _)| *pid)
            .collect()
    }
    fn descendants(&self, pid: u32) -> Vec<u32> {
        let table = self.0.borrow();
        let mut found = vec![pid];
        let mut index = 0;
        while index < found.len() {
            let current = found[index];
            for (child, p) in table.iter() {
                if p.parent == current && !found.contains(child) {
                    found.push(*child);
                }
            }
            index += 1;
        }
        found.remove(0);
        found
    }
}

/// A crash source that gives one answer and remembers what it was asked.
struct Answers {
    answer: CrashEvidence,
    asked: std::rc::Rc<RefCell<Vec<ProcessLifetime>>>,
}

impl CrashSource for Answers {
    fn lookup(&self, process: &ProcessLifetime) -> CrashEvidence {
        self.asked.borrow_mut().push(*process);
        self.answer.clone()
    }
}

struct Case {
    _dir: tempfile::TempDir,
    journal: SessionJournal,
    machine: Machine,
    session: String,
    asked: std::rc::Rc<RefCell<Vec<ProcessLifetime>>>,
    emulator: &'static str,
}

const SCOPE: &str = "app-flatpak-net.pcsx2.PCSX2-100.scope";

fn recorded() -> CrashEvidence {
    CrashEvidence::FatalTerminationRecorded {
        recorded_at: "2026-11-20 23:39:17".into(),
        source: "test".into(),
    }
}

/// A Flatpak launch with the real tree: 100 (bwrap, scope leader) -> 101
/// (bwrap) -> 102 (the emulator's own binary).
fn flatpak(
    emulator: &'static str,
    binary: &'static str,
    exe: &'static str,
    answer: CrashEvidence,
) -> Case {
    let dir = tempfile::tempdir().unwrap();
    let asked = std::rc::Rc::new(RefCell::new(Vec::new()));
    let journal = SessionJournal::open(dir.path(), "dev")
        .unwrap()
        .with_crash_source(Box::new(Answers {
            answer,
            asked: asked.clone(),
        }));
    let launch = SessionLaunch {
        emulator: emulator.into(),
        profile: "flatpak".into(),
        through_launcher: true,
        ..Default::default()
    };
    let session = journal
        .request(SessionGame::default(), launch, vec![mark("v1")])
        .unwrap()
        .session_id;
    let case = Case {
        _dir: dir,
        journal,
        machine: Machine::default(),
        session,
        asked,
        emulator,
    };
    case.machine
        .start(100, 1, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    case.machine.start(102, 101, binary, exe, Some(SCOPE));
    case
}

fn mark(content: &str) -> SaveMark {
    SaveMark {
        save_id: "save-1".into(),
        label: "save-1".into(),
        content_sha256: content.into(),
    }
}

impl Case {
    fn look(&self) -> SessionRecord {
        self.journal
            .observe_running(&self.session, &self.machine)
            .unwrap()
    }
    fn end(&self, exit: ExitObservation) -> SessionRecord {
        self.journal
            .mark_ended(&self.session, exit, Some(&[mark("v2")]), &self.machine)
            .unwrap()
    }
    fn preceding(&self) -> PrecedingSession {
        preceding_session(
            &self.journal.all(),
            "dev",
            self.emulator,
            "flatpak",
            &[mark("v2")],
        )
    }
}

#[test]
fn a_crash_record_for_the_sandboxed_emulator_is_a_confirmed_fatal_termination_whatever_number_came_back()
 {
    // PCSX2 and RPCS3 are not on the demonstrated list: their exit status
    // alone is never believed. The record is independent evidence.
    for (emulator, binary, exe) in [
        ("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt"),
        ("RPCS3", "rpcs3", "/app/bin/rpcs3"),
        ("PPSSPP", "PPSSPPSDL", "/app/bin/PPSSPPSDL"),
        ("RetroArch", "retroarch", "/app/bin/retroarch"),
    ] {
        for code in [134, 139, 1, 0] {
            let case = flatpak(emulator, binary, exe, recorded());
            assert_eq!(case.look().sandbox_main.map(|main| main.pid), Some(102));
            case.machine.exit(&[102, 101, 100]);
            let ended = case.end(ExitObservation::Code(code));
            assert_eq!(
                ended.end,
                Some(SessionEnd::FatalTerminationRecorded {
                    recorded_at: "2026-11-20 23:39:17".into()
                }),
                "{emulator} {code}"
            );
            assert_eq!(ended.exit_evidence, Some(ExitEvidence::CrashRecord));
            assert!(ended.needs_attention());
            // Asked about exactly the emulator's process, with its lifetime.
            let asked = case.asked.borrow();
            assert_eq!(asked.len(), 1);
            assert_eq!(asked[0].pid, 102);
            assert!(asked[0].started_unix > 0 && asked[0].started_unix <= asked[0].gone_by_unix);
            drop(asked);
            let preceding = case.preceding();
            assert!(
                matches!(preceding, PrecedingSession::FatalTerminationRecorded { .. }),
                "{preceding:?}"
            );
            assert_eq!(preceding.advisory(), Advisory::Warning);
            let warning = preceding.warning().unwrap();
            assert!(
                warning.starts_with("Abnormal termination detected"),
                "{warning}"
            );
            let lower = warning.to_lowercase();
            // No invented signal, no claim about the game or the save.
            assert!(
                !lower.contains("crash recorded")
                    && !lower.contains("sigsegv")
                    && !lower.contains("signal 11")
                    && !lower.contains("signal 6"),
                "{warning}"
            );
            assert!(
                lower.contains("which signal") && lower.contains("is not recorded"),
                "{warning}"
            );
            assert!(
                lower.contains("does not show that") && lower.contains("save is damaged"),
                "{warning}"
            );
        }
    }
}

#[test]
fn without_a_record_or_without_a_source_the_answer_stays_unknown_and_nothing_fails() {
    for answer in [
        CrashEvidence::NoRecord {
            source: "test".into(),
        },
        CrashEvidence::unavailable("this system does not use the crash handler EmuWiz can read"),
    ] {
        for code in [134, 139, 1] {
            let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", answer.clone());
            case.look();
            case.machine.exit(&[102, 101, 100]);
            let ended = case.end(ExitObservation::Code(code));
            // 139 or 134 alone is never a fatal signal.
            assert!(
                matches!(ended.end, Some(SessionEnd::LauncherEndedOtherwise { .. })),
                "{answer:?} {code}: {:?}",
                ended.end
            );
            assert_eq!(ended.crash_evidence, Some(answer.clone()));
            assert!(!ended.needs_attention());
            let preceding = case.preceding();
            assert!(
                matches!(preceding, PrecedingSession::ExitNotSeen { .. }),
                "{preceding:?}"
            );
            assert_eq!(preceding.warning(), None);
        }
    }
}

#[test]
fn the_source_is_not_asked_when_the_emulator_was_not_identified_or_is_still_alive() {
    // Never looked at while it ran: no emulator process to ask about.
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(139));
    assert!(case.asked.borrow().is_empty());
    assert!(matches!(
        ended.end,
        Some(SessionEnd::LauncherEndedOtherwise { .. })
    ));

    // The launcher is gone but the emulator carries on (RPCS3 after a fault
    // it handled): the session stays open and nothing is asked yet.
    let case = flatpak("RPCS3", "rpcs3", "/app/bin/rpcs3", recorded());
    case.look();
    case.machine.exit(&[100]);
    let open = case.end(ExitObservation::Code(134));
    assert_eq!(open.state, SessionState::Started);
    assert!(case.asked.borrow().is_empty());

    // A shell between EmuWiz and the sandbox: the emulator inside is not
    // identified, so no record can be tied to this launch.
    let dir = tempfile::tempdir().unwrap();
    let asked = std::rc::Rc::new(RefCell::new(Vec::new()));
    let journal = SessionJournal::open(dir.path(), "dev")
        .unwrap()
        .with_crash_source(Box::new(Answers {
            answer: recorded(),
            asked: asked.clone(),
        }));
    let launch = SessionLaunch {
        emulator: "PCSX2".into(),
        profile: "flatpak".into(),
        through_launcher: true,
        ..Default::default()
    };
    let session = journal
        .request(SessionGame::default(), launch, vec![mark("v1")])
        .unwrap()
        .session_id;
    let machine = Machine::default();
    machine.start(90, 1, "bash", "/usr/bin/bash", None);
    journal.mark_started(&session, 90, &machine).unwrap();
    machine.start(100, 90, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    machine.start(102, 100, "pcsx2-qt", "/app/bin/pcsx2-qt", Some(SCOPE));
    journal.observe_running(&session, &machine).unwrap();
    machine.exit(&[102, 100, 90]);
    let ended = journal
        .mark_ended(&session, ExitObservation::Code(139), None, &machine)
        .unwrap();
    assert!(asked.borrow().is_empty());
    assert!(!ended.needs_attention());
}

#[test]
fn a_directly_started_emulator_does_not_use_the_record_and_keeps_its_own_observation() {
    let dir = tempfile::tempdir().unwrap();
    let asked = std::rc::Rc::new(RefCell::new(Vec::new()));
    let journal = SessionJournal::open(dir.path(), "dev")
        .unwrap()
        .with_crash_source(Box::new(Answers {
            answer: recorded(),
            asked: asked.clone(),
        }));
    let launch = SessionLaunch {
        emulator: "DuckStation".into(),
        profile: "native".into(),
        through_launcher: false,
        ..Default::default()
    };
    let session = journal
        .request(SessionGame::default(), launch, vec![mark("v1")])
        .unwrap()
        .session_id;
    let machine = Machine::default();
    machine.start(70, 1, "duckstation-qt", "/opt/duckstation-qt", None);
    journal.mark_started(&session, 70, &machine).unwrap();
    machine.exit(&[70]);
    let ended = journal
        .mark_ended(
            &session,
            ExitObservation::Signal {
                signal: 11,
                core_dumped: true,
            },
            None,
            &machine,
        )
        .unwrap();
    assert_eq!(
        ended.end,
        Some(SessionEnd::Signal {
            signal: 11,
            core_dumped: true
        })
    );
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::EmulatorProcess));
    assert!(
        asked.borrow().is_empty(),
        "the kernel's own report needs no second source"
    );
}

#[test]
fn a_session_emuwiz_did_not_see_end_can_still_be_confirmed_but_only_while_the_process_id_still_means_that_process()
 {
    // EmuWiz closed first; the emulator then died; EmuWiz looks again soon.
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.look();
    case.machine.exit(&[102, 101, 100]);
    let soon = super::super::now_unix() + 30;
    let settled = case.journal.reconcile(&case.machine, soon);
    assert_eq!(settled[0].state, SessionState::Ended);
    assert!(matches!(
        settled[0].end,
        Some(SessionEnd::FatalTerminationRecorded { .. })
    ));
    assert_eq!(case.asked.borrow().len(), 1);

    // The same, but nobody looked for hours: the id may have been reused by
    // then, so the source is not even asked and the end stays "not seen".
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.look();
    case.machine.exit(&[102, 101, 100]);
    let much_later = super::super::now_unix() + CRASH_RECORD_WINDOW_SECONDS + 3600;
    let settled = case.journal.reconcile(&case.machine, much_later);
    assert_eq!(settled[0].state, SessionState::EndNotSeen);
    assert!(case.asked.borrow().is_empty());
    assert!(matches!(
        settled[0].crash_evidence,
        Some(CrashEvidence::Unavailable { .. })
    ));
    let _ = Cell::new(0); // (keeps the import used on every configuration)
}

// ---- Timing refinements (owner decision 2026-11-21) ----

const NO_RECORD: &str = "the system's crash handler log";

#[test]
fn a_record_near_but_outside_the_lifetime_is_unknown_and_a_clearly_stale_one_is_not_this_launch() {
    let answer = |offset: i64| {
        Files::new(APPORT, None, Some(&(other(-5000) + &crashed(offset, 4242))))
            .source()
            .lookup(&emulator())
    };
    // Lifetime: T0+100 .. T0+400, with LIFETIME_SLACK_SECONDS either side.
    // Just before it started, or just after it was seen gone: timing unclear.
    for offset in [
        100 - LIFETIME_SLACK_SECONDS - 1,
        60,
        100 - LIFETIME_SLACK_SECONDS - UNCLEAR_TIMING_SECONDS,
        400 + LIFETIME_SLACK_SECONDS + 1,
        450,
        400 + LIFETIME_SLACK_SECONDS + UNCLEAR_TIMING_SECONDS,
    ] {
        let found = answer(offset);
        assert!(
            matches!(found, CrashEvidence::Unavailable { ref why } if why.contains("close to, but not inside")),
            "{offset}: {found:?}"
        );
    }
    // Clearly another process with this id: long before, or long after.
    for offset in [
        100 - LIFETIME_SLACK_SECONDS - UNCLEAR_TIMING_SECONDS - 1,
        -3000,
        400 + LIFETIME_SLACK_SECONDS + UNCLEAR_TIMING_SECONDS + 1,
        40_000,
    ] {
        assert_eq!(
            answer(offset),
            CrashEvidence::NoRecord {
                source: NO_RECORD.to_string()
            },
            "{offset}"
        );
    }
    // A genuine record inside the lifetime is still believed when a stale
    // one for the same id also exists far away...
    let both = other(-5000) + &crashed(-3000, 4242) + &crashed(250, 4242);
    assert!(
        Files::new(APPORT, None, Some(&both))
            .source()
            .lookup(&emulator())
            .confirmed()
    );
    // ...but not when a second one sits just outside: that is unclear.
    let unclear = other(-5000) + &crashed(250, 4242) + &crashed(450, 4242);
    assert!(matches!(
        Files::new(APPORT, None, Some(&unclear))
            .source()
            .lookup(&emulator()),
        CrashEvidence::Unavailable { .. }
    ));
}

#[test]
fn a_record_written_late_in_a_long_dying_process_is_matched_by_when_the_process_was_really_gone() {
    // The emulator caught the first fault and hung; it was finally ended 40
    // seconds later, and the handler wrote its line then. What EmuWiz tracks
    // is the process's lifetime up to when it was seen gone, so the late
    // line is inside it.
    let hung = ProcessLifetime {
        pid: 4242,
        started_unix: T0 + 100,
        gone_by_unix: T0 + 440,
    };
    let log = other(0) + &crashed(439, 4242);
    assert!(
        Files::new(APPORT, None, Some(&log))
            .source()
            .lookup(&hung)
            .confirmed()
    );
    // Had EmuWiz (wrongly) thought it gone at the time of the first fault,
    // the same line would be near-but-outside: unknown, not a rejection.
    let too_early = ProcessLifetime {
        pid: 4242,
        started_unix: T0 + 100,
        gone_by_unix: T0 + 400,
    };
    assert!(matches!(
        Files::new(APPORT, None, Some(&log))
            .source()
            .lookup(&too_early),
        CrashEvidence::Unavailable { .. }
    ));
}

#[test]
fn when_the_kernel_does_not_wait_for_the_handler_no_record_establishes_nothing() {
    let quiet = other(0) + &other(500);
    let with_limit = |log: &str, limit: Option<&str>| {
        let files = Files::new(APPORT, None, Some(log));
        let path = files.dir.path().join("core_pipe_limit");
        match limit {
            Some(text) => fs::write(&path, text).unwrap(),
            None => fs::remove_file(&path).unwrap(),
        }
        files.source().lookup(&emulator())
    };
    for limit in [Some("0\n"), Some("not a number"), None] {
        let found = with_limit(&quiet, limit);
        assert!(
            matches!(found, CrashEvidence::Unavailable { ref why } if why.contains("may not have been written yet")),
            "{limit:?}: {found:?}"
        );
        // A record that IS there still counts: it cannot be un-written.
        assert!(
            with_limit(&(other(0) + &crashed(250, 4242)), limit).confirmed(),
            "{limit:?}"
        );
    }
    // With a kernel that waits, the same quiet log is a real "no record".
    assert_eq!(
        Files::new(APPORT, None, Some(&quiet))
            .source()
            .lookup(&emulator()),
        CrashEvidence::NoRecord {
            source: NO_RECORD.to_string()
        }
    );
}

#[test]
fn a_reused_process_id_is_never_attributed_to_this_launch() {
    // The same host pid three times over two days: an old process, this
    // launch's emulator, and a later unrelated process. Only a record inside
    // this launch's lifetime is its own.
    let old_and_new = other(-90_000) + &crashed(-86_000, 4242) + &other(0) + &crashed(20_000, 4242);
    assert_eq!(
        Files::new(APPORT, None, Some(&old_and_new))
            .source()
            .lookup(&emulator()),
        CrashEvidence::NoRecord {
            source: NO_RECORD.to_string()
        },
        "neither the earlier nor the later process is this one"
    );
    let with_ours = old_and_new + &crashed(399, 4242);
    assert_eq!(
        Files::new(APPORT, None, Some(&with_ours))
            .source()
            .lookup(&emulator()),
        CrashEvidence::FatalTerminationRecorded {
            recorded_at: stamp(399),
            source: NO_RECORD.to_string()
        }
    );
}

// ---- Fourth review (2026-11-21) ----

#[test]
fn a_wall_clock_time_is_used_only_when_exactly_one_reading_of_it_is_real() {
    // An ordinary time: one reading.
    assert_eq!(one_reading(Some(100), None), Some(100));
    assert_eq!(one_reading(None, Some(100)), Some(100));
    // A zone without daylight saving: both readings agree.
    assert_eq!(one_reading(Some(100), Some(100)), Some(100));
    // The hour that repeats when the clocks go back: two instants an hour
    // apart, and the log does not say which. Not placed.
    assert_eq!(one_reading(Some(3600), Some(0)), None);
    // The hour skipped when they go forward, or an impossible date.
    assert_eq!(one_reading(None, None), None);
}

#[test]
fn the_real_conversion_agrees_with_the_systems_own_clock_or_declines() {
    // Take real instants, ask the C library for their local wall-clock time
    // (what the handler writes), and convert back.
    let now = super::super::now_unix();
    let zone_set = std::env::var_os("TZ").is_some_and(|zone| !zone.is_empty());
    for instant in [now, now - 86_400 * 30, now - 86_400 * 200, T0] {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let clock = instant as libc::time_t;
        assert!(!unsafe { libc::localtime_r(&clock, &mut tm) }.is_null());
        let local = LocalTime {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
            second: tm.tm_sec as u32,
        };
        match local_to_unix_for_tests(&local) {
            // Never a different instant.
            Some(found) => assert_eq!(found, instant, "{local:?}"),
            // Declining is allowed (a repeated hour, or TZ set for this
            // process); a wrong answer is not.
            None => assert!(zone_set || tm.tm_hour <= 3, "{local:?}"),
        }
    }
    // Dates that do not exist are refused, not normalised into real ones.
    for (month, day, hour) in [(2, 30, 12), (13, 1, 12), (11, 31, 12), (11, 20, 25)] {
        let impossible = LocalTime {
            year: 2026,
            month,
            day,
            hour,
            minute: 0,
            second: 0,
        };
        assert_eq!(local_to_unix_for_tests(&impossible), None, "{impossible:?}");
    }
}

#[test]
fn a_time_that_cannot_be_placed_is_unknown_for_this_process_and_ignored_for_others() {
    fn never(_: &LocalTime) -> Option<i64> {
        None
    }
    fn not_at_250(time: &LocalTime) -> Option<i64> {
        utc(time).filter(|at| *at != T0 + 250)
    }
    let log = other(0) + &crashed(250, 4242);
    let files = Files::new(APPORT, None, Some(&log));
    let mut source = files.source();
    source.to_unix = not_at_250;
    let found = source.lookup(&emulator());
    assert!(
        matches!(found, CrashEvidence::Unavailable { ref why } if why.contains("a time EmuWiz cannot place")),
        "{found:?}"
    );
    // Another process's unplaceable line changes nothing.
    let log = other(0) + &crashed(250, 7) + &crashed(260, 4242);
    let files = Files::new(APPORT, None, Some(&log));
    let mut source = files.source();
    source.to_unix = not_at_250;
    assert!(source.lookup(&emulator()).confirmed());
    // Nothing can be placed at all: the log then covers nothing.
    let files = Files::new(APPORT, None, Some(&(other(0) + &other(500))));
    let mut source = files.source();
    source.to_unix = never;
    assert!(matches!(
        source.lookup(&emulator()),
        CrashEvidence::Unavailable { .. }
    ));
}

#[test]
fn an_oversized_log_or_one_with_binary_bytes_is_handled_without_guessing() {
    // Larger than the limit: not read, not partly believed.
    let files = Files::new(APPORT, None, Some(&(other(0) + &crashed(250, 4242))));
    let path = files.dir.path().join("apport.log");
    let mut big = fs::read(&path).unwrap();
    big.resize(8 * 1024 * 1024 + 1, b'\n');
    fs::write(&path, &big).unwrap();
    assert!(matches!(
        files.source().lookup(&emulator()),
        CrashEvidence::Unavailable { .. }
    ));
    // Real non-UTF-8 bytes on other lines do not hide or create a record.
    let files = Files::new(APPORT, None, None);
    let mut bytes = other(0).into_bytes();
    bytes.extend_from_slice(b"\xff\xfe\x80 junk \xc3\x28\n");
    bytes.extend_from_slice(crashed(250, 4242).as_bytes());
    fs::write(files.dir.path().join("apport.log"), &bytes).unwrap();
    assert!(files.source().lookup(&emulator()).confirmed());
    let mut bytes = other(0).into_bytes();
    bytes.extend_from_slice(b"\xff\xfe\x80 junk\n");
    bytes.extend_from_slice(other(500).as_bytes());
    fs::write(files.dir.path().join("apport.log"), &bytes).unwrap();
    assert_eq!(
        files.source().lookup(&emulator()),
        CrashEvidence::NoRecord {
            source: NO_RECORD.to_string()
        }
    );
}

#[test]
fn a_line_inside_another_programs_quoted_command_line_is_not_the_handlers_word() {
    // A program that crashes with line breaks in its arguments gets them
    // written into the log verbatim. It can make a line that looks exactly
    // like the handler's.
    let forged = other(0)
        + &format!(
            "ERROR: apport (pid 9) {},100: executable: /tmp/x (command line \"/tmp/x\n",
            stamp(240)
        )
        + &crashed(250, 4242)
        + "tail\")\n"
        + &other(500);
    let found = Files::new(APPORT, None, Some(&forged))
        .source()
        .lookup(&emulator());
    assert!(
        matches!(found, CrashEvidence::Unavailable { ref why } if why.contains("quoted command line")),
        "{found:?}"
    );
    // The same shape naming another process changes nothing for this one.
    let unrelated = other(0)
        + &format!(
            "ERROR: apport (pid 9) {},100: executable: /tmp/x (command line \"/tmp/x\n",
            stamp(240)
        )
        + &crashed(245, 7)
        + "tail\")\n"
        + &crashed(250, 4242);
    assert!(
        Files::new(APPORT, None, Some(&unrelated))
            .source()
            .lookup(&emulator())
            .confirmed()
    );
    // An ordinary one-line command line (the usual case) closes itself.
    let usual = other(0)
        + &format!(
            "ERROR: apport (pid 9) {},100: executable: /app/bin/pcsx2-qt (command line \"pcsx2-qt -batch\")\n",
            stamp(249)
        )
        + &crashed(250, 4242);
    assert!(
        Files::new(APPORT, None, Some(&usual))
            .source()
            .lookup(&emulator())
            .confirmed()
    );
}

/// Moves a session's recorded times back, as if it had started that long ago.
fn age(case: &Case, seconds: i64) {
    fn shift(value: &mut serde_json::Value, seconds: i64) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, inner) in map.iter_mut() {
                    let timed = [
                        "first_seen_unix",
                        "sandbox_main_last_alive_unix",
                        "started_unix",
                    ];
                    match inner.as_i64() {
                        Some(at) if timed.contains(&key.as_str()) && at > 0 => {
                            *inner = (at - seconds).into()
                        }
                        _ => shift(inner, seconds),
                    }
                }
            }
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(|item| shift(item, seconds))
            }
            _ => {}
        }
    }
    let dir = case._dir.path().join("sessions");
    let mut changed = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        shift(&mut value, seconds);
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        changed += 1;
    }
    assert_eq!(changed, 1);
}

#[test]
fn a_long_session_that_emuwiz_kept_watching_is_still_asked_about() {
    // Half an hour of play, with EmuWiz looking from time to time.
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    let first = case.look();
    age(&case, 1800);
    let aged = case.journal.all().remove(0);
    assert_eq!(
        aged.sandbox_main_last_alive_unix,
        first.sandbox_main_last_alive_unix.map(|at| at - 1800)
    );
    // The next look finds it alive and that is written down, not just held.
    case.look();
    let stored = case.journal.all().remove(0);
    let now = super::super::now_unix();
    assert!(
        (now - stored.sandbox_main_last_alive_unix.unwrap()).abs() <= 2,
        "{stored:?}"
    );
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(139));
    assert!(
        matches!(ended.end, Some(SessionEnd::FatalTerminationRecorded { .. })),
        "{:?}",
        ended.crash_evidence
    );
    // Asked with the real bounds: from when the emulator was first seen
    // (half an hour ago) to now.
    let asked = case.asked.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].pid, 102);
    assert_eq!(
        asked[0].started_unix,
        aged.sandbox_main.as_ref().unwrap().first_seen_unix
    );
    assert!((asked[0].started_unix - (now - 1800)).abs() <= 2);
    assert!((asked[0].gone_by_unix - now).abs() <= 2);
    drop(asked);

    // A look less than a minute after the last write is not written again
    // (it is a look, not a write, on every frame).
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.look();
    age(&case, LAST_ALIVE_WRITE_SECONDS - 20);
    let before = case.journal.all().remove(0).sandbox_main_last_alive_unix;
    case.look();
    assert_eq!(
        case.journal.all().remove(0).sandbox_main_last_alive_unix,
        before
    );

    // The same half hour with nobody looking after the first seconds: the
    // id may mean something else by now, so the source is not asked.
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.look();
    age(&case, 1800);
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(139));
    assert!(case.asked.borrow().is_empty());
    assert!(matches!(
        ended.crash_evidence,
        Some(CrashEvidence::Unavailable { .. })
    ));
    assert!(!ended.needs_attention());
}

#[test]
fn a_record_is_not_tied_to_the_launch_when_the_process_id_may_have_come_round() {
    // Inside the ten-minute window, but the system has since started more
    // processes than half its id range: the id may belong to another.
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.machine.1.set(Some((1_000, 32_768)));
    case.look();
    case.machine.exit(&[102, 101, 100]);
    case.machine.1.set(Some((1_000 + 16_384, 32_768)));
    let settled = case
        .journal
        .reconcile(&case.machine, super::super::now_unix() + 300);
    assert!(case.asked.borrow().is_empty());
    assert_eq!(settled[0].state, SessionState::EndNotSeen);
    assert!(
        matches!(settled[0].crash_evidence, Some(CrashEvidence::Unavailable { ref why }) if why.contains("may have been given to another")),
        "{:?}",
        settled[0].crash_evidence
    );
    // Fewer than that: asked as usual.
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.machine.1.set(Some((1_000, 32_768)));
    case.look();
    case.machine.exit(&[102, 101, 100]);
    case.machine.1.set(Some((1_000 + 900, 32_768)));
    let settled = case
        .journal
        .reconcile(&case.machine, super::super::now_unix() + 300);
    assert_eq!(case.asked.borrow().len(), 1);
    assert!(matches!(
        settled[0].end,
        Some(SessionEnd::FatalTerminationRecorded { .. })
    ));
}

#[test]
fn a_record_found_when_emuwiz_was_not_running_says_the_save_was_not_compared() {
    // Settled later by a look, not at the moment it ended: nobody compared
    // the saves then.
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.look();
    case.machine.exit(&[102, 101, 100]);
    case.journal
        .reconcile(&case.machine, super::super::now_unix() + 30);
    let current = [mark("v1")];
    let preceding = preceding_session(&case.journal.all(), "dev", "PCSX2", "flatpak", &current);
    assert_eq!(
        preceding,
        PrecedingSession::FatalTerminationRecorded {
            session_id: case.session.clone(),
            saves_compared_afterwards: false
        }
    );
    let warning = preceding.warning().unwrap();
    assert!(
        warning.contains("was not compared then and may also have changed since"),
        "{warning}"
    );
    // Seen at the time, with the saves compared: no such caveat.
    let case = flatpak("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt", recorded());
    case.look();
    case.machine.exit(&[102, 101, 100]);
    case.end(ExitObservation::Code(134));
    let warning = case.preceding().warning().unwrap();
    assert!(!warning.contains("was not compared"), "{warning}");
    // And if the save has changed since a compared session, the session no
    // longer speaks for it at all.
    let changed = preceding_session(
        &case.journal.all(),
        "dev",
        "PCSX2",
        "flatpak",
        &[mark("v3")],
    );
    assert!(
        matches!(changed, PrecedingSession::Unknown { .. }),
        "{changed:?}"
    );
}

#[test]
fn a_snapshot_status_written_before_the_rename_can_still_be_read() {
    let old: PrecedingSession =
        serde_json::from_str(r#"{"outcome":"crashed","session_id":"s","signal":11}"#).unwrap();
    assert_eq!(
        old,
        PrecedingSession::FatalSignal {
            session_id: "s".into(),
            signal: 11
        }
    );
    let no_field: PrecedingSession =
        serde_json::from_str(r#"{"outcome":"fatal_termination_recorded","session_id":"s"}"#)
            .unwrap();
    assert_eq!(
        no_field,
        PrecedingSession::FatalTerminationRecorded {
            session_id: "s".into(),
            saves_compared_afterwards: true
        }
    );
}
