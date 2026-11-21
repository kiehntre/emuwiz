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
    let day = 20 + (t - T0 + 84_600) / 86_400; // T0 is 23:30 on the 20th
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
struct Machine(RefCell<BTreeMap<u32, Proc>>);

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
