//! Whose exit is it? Flatpak sandboxes, wrappers, launchers that die first,
//! EmuWiz closing first, and somebody else's emulator running alongside.
//!
//! The process trees here are the ones `scripts/qa/flatpak_exit_probe.py`
//! recorded from real launches: started process (bwrap, leader of the app
//! scope) -> bwrap -> emulator binary.

use super::super::session::*;
use super::super::status::{Advisory, PrecedingSession};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

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
        self.0.borrow().get(&pid).map(|process| process.ticks)
    }
    fn executable(&self, pid: u32) -> Option<PathBuf> {
        self.0
            .borrow()
            .get(&pid)
            .map(|process| PathBuf::from(process.exe))
    }
    fn parent(&self, pid: u32) -> Option<u32> {
        self.0.borrow().get(&pid).map(|process| process.parent)
    }
    fn name(&self, pid: u32) -> Option<String> {
        self.0
            .borrow()
            .get(&pid)
            .map(|process| process.name.to_string())
    }
    fn app_scope(&self, pid: u32) -> Option<String> {
        self.0
            .borrow()
            .get(&pid)
            .and_then(|process| process.scope)
            .map(str::to_string)
    }
    fn in_scope(&self, scope: &str) -> Vec<u32> {
        self.0
            .borrow()
            .iter()
            .filter(|(_, process)| process.scope == Some(scope))
            .map(|(pid, _)| *pid)
            .collect()
    }
    fn descendants(&self, pid: u32) -> Vec<u32> {
        let table = self.0.borrow();
        let mut found = vec![pid];
        let mut index = 0;
        while index < found.len() {
            let current = found[index];
            for (child, process) in table.iter() {
                if process.parent == current && !found.contains(child) {
                    found.push(*child);
                }
            }
            index += 1;
        }
        found.remove(0);
        found
    }
}

const SCOPE: &str = "app-flatpak-test.probe.ProbeEmu-100.scope";
const SAVE: &str = "save-1";

fn mark(content: &str) -> SaveMark {
    SaveMark {
        save_id: SAVE.into(),
        label: SAVE.into(),
        content_sha256: content.into(),
        last_changed_unix: None,
        before_launch: None,
        folder: None,
        path: None,
    }
}

struct Case {
    _dir: tempfile::TempDir,
    journal: SessionJournal,
    machine: Machine,
    session: String,
}

/// A Flatpak launch of the made-up, trusted-in-tests emulator, with the
/// process tree really observed for Flatpak emulators: 100 (bwrap, scope leader)
/// -> 101 (bwrap) -> 102 (/app/bin/pcsx2-qt).
fn flatpak_probe() -> Case {
    let case = launch("ProbeEmu", "flatpak", true);
    case.machine
        .start(100, 1, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    case.machine
        .start(102, 101, "probeemu", "/app/bin/probeemu", Some(SCOPE));
    case
}

fn launch(emulator: &str, profile: &str, through_launcher: bool) -> Case {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path(), "dev").unwrap();
    let launch = SessionLaunch {
        emulator: emulator.into(),
        profile: profile.into(),
        through_launcher,
        ..Default::default()
    };
    let session = journal
        .request(SessionGame::default(), launch, vec![mark("v1")])
        .unwrap()
        .session_id;
    Case {
        _dir: dir,
        journal,
        machine: Machine::default(),
        session,
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
            "ProbeEmu",
            "flatpak",
            &[mark("v2")],
        )
    }
}

#[test]
fn a_normal_flatpak_shutdown_is_a_confirmed_clean_exit() {
    let case = flatpak_probe();
    let seen = case.look();
    assert_eq!(
        seen.sandbox_main.as_ref().map(|main| main.pid),
        Some(102),
        "the emulator inside was identified"
    );
    // The emulator closes; the sandbox relays its status 0 and exits.
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.state, SessionState::Ended);
    assert_eq!(ended.end, Some(SessionEnd::Clean));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::SandboxRelay));
    let preceding = case.preceding();
    assert!(preceding.clean(), "{preceding:?}");
    assert_eq!(preceding.advisory(), Advisory::None);
}

#[test]
fn an_exit_code_from_a_sandbox_is_never_turned_into_a_signal() {
    // 139 and 134 are what a sandbox returns when its command dies of
    // SIGSEGV or SIGABRT, and also just numbers a program may return. Even
    // for an emulator whose exit codes are believed, they stay exit codes:
    // an abnormal exit status, with no claim about a fatal signal.
    for code in [139, 134, 143, 1] {
        let case = flatpak_probe();
        case.look();
        case.machine.exit(&[102, 101, 100]);
        let ended = case.end(ExitObservation::Code(code));
        assert_eq!(ended.end, Some(SessionEnd::ExitCode { code }), "{code}");
        assert_eq!(ended.exit_evidence, Some(ExitEvidence::SandboxRelay));
        assert!(
            !ended.status_line().to_lowercase().contains("signal"),
            "{}",
            ended.status_line()
        );
        let preceding = case.preceding();
        assert!(
            matches!(preceding, PrecedingSession::ErrorExit { code: c, .. } if c == code),
            "{code}: {preceding:?}"
        );
        assert!(!matches!(
            preceding,
            PrecedingSession::FatalSignal { .. } | PrecedingSession::ForcedStop { .. }
        ));
        let warning = preceding.warning().unwrap();
        assert!(
            warning.starts_with("Abnormal exit detected") && !warning.contains("signal"),
            "{warning}"
        );
    }
}

#[test]
fn a_forced_stop_and_an_error_exit_inside_the_sandbox_are_confirmed_as_such() {
    let case = flatpak_probe();
    case.look();
    case.machine.exit(&[102, 101, 100]);
    case.end(ExitObservation::Code(143)); // an exit code; never read as "signal 15"
    assert!(matches!(
        case.preceding(),
        PrecedingSession::ErrorExit { code: 143, .. }
    ));

    let case = flatpak_probe();
    case.look();
    case.machine.exit(&[102, 101, 100]);
    case.end(ExitObservation::Code(1)); // PCSX2 exits 1 when asked to terminate
    let preceding = case.preceding();
    assert!(
        matches!(preceding, PrecedingSession::ErrorExit { code: 1, .. }),
        "{preceding:?}"
    );
    assert_eq!(preceding.advisory(), Advisory::Warning);
}

#[test]
fn a_launcher_returning_success_is_not_a_clean_exit_when_the_emulator_was_never_identified() {
    // The session was over before EmuWiz ever saw the emulator process, or
    // the app is one whose relay was never checked.
    let case = flatpak_probe();
    case.machine.exit(&[102, 101, 100]); // no look() happened in between
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.end, Some(SessionEnd::LauncherExitedOk));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));
    let preceding = case.preceding();
    assert!(
        matches!(preceding, PrecedingSession::ExitNotSeen { .. }),
        "{preceding:?}"
    );
    assert_eq!(
        preceding.advisory(),
        Advisory::Caution,
        "uncertainty, not a warning"
    );
    assert_eq!(preceding.warning(), None);

    // An app that is not on the checked list, even with a perfect tree.
    let case = launch("Dolphin", "flatpak", true);
    case.machine.start(
        100,
        1,
        "bwrap",
        "/usr/bin/bwrap",
        Some("app-flatpak-org.DolphinEmu.dolphin-emu-100.scope"),
    );
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", None);
    case.machine
        .start(102, 101, "dolphin-emu", "/app/bin/dolphin-emu", None);
    assert!(case.look().sandbox_main.is_none());
    case.machine.exit(&[102, 101, 100]);
    assert_eq!(
        case.end(ExitObservation::Code(0)).end,
        Some(SessionEnd::LauncherExitedOk)
    );
}

#[test]
fn a_launcher_ending_badly_is_not_a_crash() {
    // Nothing was identified inside, and the launcher returns 139.
    let case = flatpak_probe();
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(139));
    assert!(matches!(
        ended.end,
        Some(SessionEnd::LauncherEndedOtherwise { .. })
    ));
    assert!(!ended.end.as_ref().unwrap().abnormal());
    assert!(!ended.needs_attention());
    let preceding = case.preceding();
    assert!(
        matches!(preceding, PrecedingSession::ExitNotSeen { .. }),
        "{preceding:?}"
    );
    assert_eq!(preceding.advisory(), Advisory::Caution);

    // The launcher itself is killed while the emulator carries on (observed
    // for real: the emulator was still alive two seconds later).
    let case = flatpak_probe();
    case.look();
    case.machine.exit(&[100]);
    let open = case.end(ExitObservation::Signal {
        signal: 9,
        core_dumped: false,
    });
    assert_eq!(
        open.state,
        SessionState::Started,
        "the emulator is still running"
    );
    assert!(case.journal.reconcile(&case.machine, 0).is_empty());
    // Later the emulator goes away too. How is not known: not clean, not forced.
    case.machine.exit(&[101, 102]);
    let settled = case.journal.reconcile(&case.machine, 5);
    assert_eq!(settled[0].state, SessionState::Ended);
    assert!(
        matches!(
            settled[0].end,
            Some(SessionEnd::LauncherEndedOtherwise { .. })
        ),
        "{:?}",
        settled[0].end
    );
    assert_eq!(settled[0].exit_evidence, Some(ExitEvidence::LauncherOnly));
    let preceding = preceding_session(
        &case.journal.all(),
        "dev",
        "ProbeEmu",
        "flatpak",
        &[mark("v1")],
    );
    assert!(
        matches!(preceding, PrecedingSession::ExitNotSeen { .. }),
        "{preceding:?}"
    );
}

#[test]
fn a_wrapper_that_does_not_hand_over_cleanly_gives_no_answer() {
    // A shell script stays between EmuWiz and the sandbox: whatever status
    // it returns is the script's.
    let case = launch("ProbeEmu", "flatpak", true);
    case.machine.start(90, 1, "bash", "/usr/bin/bash", None);
    case.journal
        .mark_started(&case.session, 90, &case.machine)
        .unwrap();
    case.machine
        .start(100, 90, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    case.machine
        .start(102, 101, "probeemu", "/app/bin/probeemu", Some(SCOPE));
    // The process EmuWiz started is a shell, not the sandbox. Whatever the
    // script returns is the script's: here the emulator dies of a segfault
    // mid-save and the script (`flatpak run ... ; true`) still exits 0.
    assert!(
        case.look().sandbox_main.is_none(),
        "a script between EmuWiz and the sandbox: no relayed status is believed"
    );
    case.machine.exit(&[102, 101, 100, 90]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.end, Some(SessionEnd::LauncherExitedOk), "never Clean");
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));
    let preceding = case.preceding();
    assert!(!preceding.clean(), "{preceding:?}");
    assert!(matches!(preceding, PrecedingSession::ExitNotSeen { .. }));
    // ...but with a second shell inside the sandbox in front of the emulator:
    let case = launch("ProbeEmu", "flatpak", true);
    case.machine
        .start(100, 1, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
    case.machine
        .start(102, 101, "sh", "/usr/bin/sh", Some(SCOPE));
    case.machine
        .start(103, 102, "probeemu", "/app/bin/probeemu", Some(SCOPE));
    assert!(
        case.look().sandbox_main.is_none(),
        "a shell between the sandbox and the emulator"
    );
    case.machine.exit(&[103, 102, 101, 100]);
    assert_eq!(
        case.end(ExitObservation::Code(0)).end,
        Some(SessionEnd::LauncherExitedOk)
    );
    assert!(matches!(
        case.preceding(),
        PrecedingSession::ExitNotSeen { .. }
    ));
}

#[test]
fn a_native_wrapper_script_exit_is_the_wrappers_only() {
    // through_launcher with a native profile: never relayed.
    let case = launch("PCSX2", "native", true);
    case.machine.start(50, 1, "bash", "/usr/bin/bash", None);
    case.journal
        .mark_started(&case.session, 50, &case.machine)
        .unwrap();
    case.machine
        .start(51, 50, "pcsx2-qt", "/usr/bin/pcsx2-qt", None);
    assert!(case.look().sandbox_main.is_none());
    case.machine.exit(&[51, 50]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.end, Some(SessionEnd::LauncherExitedOk));
}

#[test]
fn emuwiz_closing_first_leaves_plain_uncertainty() {
    let case = flatpak_probe();
    case.look();
    // EmuWiz exits here: nobody calls mark_ended. The emulator closes later.
    case.machine.exit(&[102, 101, 100]);
    // Next start of EmuWiz:
    let settled = case.journal.reconcile(&case.machine, 0);
    assert_eq!(settled[0].state, SessionState::EndNotSeen);
    assert_eq!(settled[0].end, None);
    let preceding = preceding_session(
        &case.journal.all(),
        "dev",
        "ProbeEmu",
        "flatpak",
        &[mark("v1")],
    );
    assert!(
        matches!(preceding, PrecedingSession::ExitNotSeen { .. }),
        "{preceding:?}"
    );
    assert_eq!(preceding.advisory(), Advisory::Caution);
    assert!(preceding.uncertainty().unwrap().starts_with("Not known"));
}

#[test]
fn somebody_elses_emulator_does_not_change_this_session() {
    let case = flatpak_probe();
    // Another PCSX2, started outside EmuWiz, in its own scope, with its own tree.
    let other = "app-flatpak-test.probe.ProbeEmu-900.scope";
    case.machine
        .start(900, 1, "bwrap", "/usr/bin/bwrap", Some(other));
    case.machine
        .start(901, 900, "bwrap", "/usr/bin/bwrap", Some(other));
    case.machine
        .start(902, 901, "probeemu", "/app/bin/probeemu", Some(other));
    let seen = case.look();
    assert_eq!(seen.sandbox_main.as_ref().map(|main| main.pid), Some(102));
    assert!(
        seen.processes.iter().all(|process| process.pid < 900),
        "the other instance is not tracked"
    );

    // Ours closes normally while theirs keeps running: ours is over, cleanly.
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.state, SessionState::Ended);
    assert_eq!(ended.end, Some(SessionEnd::Clean));

    // And theirs crashing later changes nothing recorded for ours.
    case.machine.exit(&[902, 901, 900]);
    assert!(case.journal.reconcile(&case.machine, 9).is_empty());
    assert!(case.preceding().clean());
}

#[test]
fn a_native_emulator_started_directly_reports_its_own_exit() {
    let case = launch("ProbeEmu", "native", false);
    case.machine
        .start(70, 1, "duckstation-qt", "/opt/duckstation-qt", None);
    case.journal
        .mark_started(&case.session, 70, &case.machine)
        .unwrap();
    case.machine.exit(&[70]);
    let ended = case.end(ExitObservation::Signal {
        signal: 11,
        core_dumped: true,
    });
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::EmulatorProcess));
    assert_eq!(
        ended.end,
        Some(SessionEnd::Signal {
            signal: 11,
            core_dumped: true
        })
    );
}

#[test]
fn copy_safety_does_not_depend_on_how_the_emulator_ended() {
    use super::super::store::CaptureBoundary;
    use super::*;
    // Exit unknown, emulator closed: the copy is a verified safe point, and
    // the record keeps "unknown" beside it.
    let bench = Bench::new();
    let file = bench.file("card.ps2", b"bytes");
    let mut ask = request("dev", &[&file], 1);
    ask.preceding_session = PrecedingSession::ExitNotSeen {
        session_id: "s".into(),
        detail: "only the launcher's end was seen".into(),
    };
    let manifest = bench
        .store
        .capture(&ask, &CLOSED)
        .unwrap()
        .manifest()
        .clone();
    assert_eq!(
        manifest.body.boundary,
        CaptureBoundary::EmulatorClosedObserved
    );
    assert!(
        !manifest.body.clean_recovery_point(),
        "unknown exit is never promoted to a clean recovery point"
    );
    assert_eq!(
        super::super::workflow::warning_of(&manifest),
        None,
        "no crash warning for not knowing"
    );
    assert!(
        super::super::workflow::uncertainty_of(&manifest)
            .unwrap()
            .starts_with("Not known")
    );
    assert!(
        super::super::workflow::status_lines(&manifest)[2].starts_with("Game data not checked")
    );
}

#[test]
fn no_real_emulators_flatpak_exit_is_believed_until_demonstrated_from_inside_a_game() {
    // Every real emulator probed so far, each with a perfect process tree.
    // What was observed with no game running is preliminary evidence only.
    let real: [(&str, &'static str, &'static str, &'static str); 4] = [
        (
            "PCSX2",
            "pcsx2-qt",
            "/app/bin/pcsx2-qt",
            "app-flatpak-net.pcsx2.PCSX2-100.scope",
        ),
        (
            "RetroArch",
            "retroarch",
            "/app/bin/retroarch",
            "app-flatpak-org.libretro.RetroArch-100.scope",
        ),
        (
            "RPCS3",
            "rpcs3",
            "/app/bin/rpcs3",
            "app-flatpak-net.rpcs3.RPCS3-100.scope",
        ),
        (
            "PPSSPP",
            "PPSSPPSDL",
            "/app/bin/PPSSPPSDL",
            "app-flatpak-org.ppsspp.PPSSPP-100.scope",
        ),
    ];
    for (emulator, binary, exe, scope) in real {
        assert!(
            !exit_status_demonstrated(emulator),
            "{emulator} is not on the demonstrated list"
        );
        assert!(
            PRELIMINARY_EXIT_EVIDENCE
                .iter()
                .any(|(name, _)| *name == emulator),
            "{emulator}: what was observed is kept as preliminary evidence"
        );
        for code in [0, 1, 134, 139, 143] {
            if code == 0 && relayed_zero_believed(emulator) {
                // Its own test: `a_relayed_zero_from_the_tracked_pcsx2_process_is_a_normal_close`.
                continue;
            }
            let case = launch(emulator, "flatpak", true);
            case.machine
                .start(100, 1, "bwrap", "/usr/bin/bwrap", Some(scope));
            case.journal
                .mark_started(&case.session, 100, &case.machine)
                .unwrap();
            case.machine
                .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(scope));
            case.machine.start(102, 101, binary, exe, Some(scope));
            // The emulator's own process inside the sandbox is identified
            // (so that evidence about exactly that process can be tied to
            // the launch), but that does not make its exit status believed.
            assert_eq!(
                case.look().sandbox_main.map(|main| main.pid),
                Some(102),
                "{emulator}: the emulator inside the sandbox is identified"
            );
            case.machine.exit(&[102, 101, 100]);
            let ended = case.end(ExitObservation::Code(code));
            assert_eq!(
                ended.exit_evidence,
                Some(ExitEvidence::LauncherOnly),
                "{emulator} {code}"
            );
            assert!(
                !ended.end.as_ref().unwrap().abnormal(),
                "{emulator} {code}: never abnormal"
            );
            assert_ne!(
                ended.end,
                Some(SessionEnd::Clean),
                "{emulator} {code}: never clean"
            );
            assert!(!ended.needs_attention(), "{emulator} {code}");
            let preceding = preceding_session(
                &case.journal.all(),
                "dev",
                emulator,
                "flatpak",
                &[mark("v2")],
            );
            assert!(
                matches!(preceding, PrecedingSession::ExitNotSeen { .. }),
                "{emulator} {code}: {preceding:?}"
            );
            assert_eq!(preceding.advisory(), Advisory::Caution);
            assert_eq!(preceding.warning(), None);
        }
    }
    // The shipped build believes no emulator at all; only the made-up test
    // emulator is on the list, and only in test builds.
    assert_eq!(EXIT_STATUS_DEMONSTRATED.len(), 1);
    assert_eq!(EXIT_STATUS_DEMONSTRATED[0].0, "ProbeEmu");
}

/// A Flatpak launch of PCSX2 with the process tree really observed:
/// 100 (bwrap, the process EmuWiz started) -> 101 (bwrap) -> 102 (pcsx2-qt).
fn flatpak_pcsx2() -> Case {
    const PCSX2_SCOPE: &str = "app-flatpak-net.pcsx2.PCSX2-100.scope";
    let case = launch("PCSX2", "flatpak", true);
    case.machine
        .start(100, 1, "bwrap", "/usr/bin/bwrap", Some(PCSX2_SCOPE));
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(PCSX2_SCOPE));
    case.machine
        .start(102, 101, "pcsx2-qt", "/app/bin/pcsx2-qt", Some(PCSX2_SCOPE));
    case
}

#[test]
fn a_relayed_zero_from_the_tracked_pcsx2_process_is_a_normal_close() {
    // Owner decision 2026-11-23. The emulator's own process was identified
    // inside the sandbox, it is gone, and the sandbox relayed 0.
    assert!(relayed_zero_believed("PCSX2"));
    assert!(!exit_status_demonstrated("PCSX2"));
    let case = flatpak_pcsx2();
    assert_eq!(case.look().sandbox_main.map(|main| main.pid), Some(102));
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.state, SessionState::Ended);
    assert_eq!(ended.end, Some(SessionEnd::Clean));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::SandboxRelay));
    assert_eq!(
        ended.observed_status,
        Some(ObservedStatus::ExitCode { code: 0 })
    );
    assert!(!ended.needs_attention());
    // How the process ended says nothing about a save: the save changes are
    // whatever the marks say, here one changed mark and no claim beyond it.
    assert!(!ended.status_line().to_lowercase().contains("saved"));
}

#[test]
fn any_other_relayed_status_of_pcsx2_stays_the_launchers_alone() {
    // Nothing but 0 was ever shown to mean anything for PCSX2 (a close at
    // its first-run window returned 1). Non-zero is neither clean nor
    // abnormal: exactly what was recorded before the decision.
    for code in [1, 2, 134, 139, 143, 255] {
        let case = flatpak_pcsx2();
        case.look();
        case.machine.exit(&[102, 101, 100]);
        let ended = case.end(ExitObservation::Code(code));
        assert_eq!(
            ended.exit_evidence,
            Some(ExitEvidence::LauncherOnly),
            "{code}"
        );
        assert_ne!(ended.end, Some(SessionEnd::Clean), "{code}");
        assert!(!ended.end.as_ref().unwrap().abnormal(), "{code}");
        assert!(!ended.needs_attention(), "{code}");
    }
    // A signal that ended the process EmuWiz started is not a relayed status.
    let case = flatpak_pcsx2();
    case.look();
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Signal {
        signal: 9,
        core_dumped: false,
    });
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));
    assert_ne!(ended.end, Some(SessionEnd::Clean));
}

#[test]
fn a_zero_that_is_only_the_launchers_is_not_a_normal_close_of_pcsx2() {
    const PCSX2_SCOPE: &str = "app-flatpak-net.pcsx2.PCSX2-100.scope";
    // 1. The emulator's process was never seen: nothing to attribute 0 to.
    let case = launch("PCSX2", "flatpak", true);
    case.machine
        .start(100, 1, "bwrap", "/usr/bin/bwrap", Some(PCSX2_SCOPE));
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    assert!(case.look().sandbox_main.is_none());
    case.machine.exit(&[100]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.end, Some(SessionEnd::LauncherExitedOk));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));

    // 2. A shell sits between the sandbox and the emulator: the status that
    //    comes back may be the shell's.
    let case = launch("PCSX2", "flatpak", true);
    case.machine
        .start(100, 1, "bwrap", "/usr/bin/bwrap", Some(PCSX2_SCOPE));
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(PCSX2_SCOPE));
    case.machine
        .start(102, 101, "sh", "/usr/bin/sh", Some(PCSX2_SCOPE));
    case.machine
        .start(103, 102, "pcsx2-qt", "/app/bin/pcsx2-qt", Some(PCSX2_SCOPE));
    assert!(case.look().sandbox_main.is_none());
    case.machine.exit(&[103, 102, 101, 100]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.end, Some(SessionEnd::LauncherExitedOk));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));

    // 3. The process EmuWiz started is a script, not the sandbox.
    let case = launch("PCSX2", "flatpak", true);
    case.machine
        .start(100, 1, "sh", "/usr/bin/sh", Some(PCSX2_SCOPE));
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(PCSX2_SCOPE));
    case.machine
        .start(102, 101, "pcsx2-qt", "/app/bin/pcsx2-qt", Some(PCSX2_SCOPE));
    assert!(case.look().sandbox_main.is_none());
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.end, Some(SessionEnd::LauncherExitedOk));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));

    // 4. The launcher returned 0 while the identified emulator is still
    //    running: that 0 is not the emulator's.
    let case = flatpak_pcsx2();
    assert_eq!(case.look().sandbox_main.map(|main| main.pid), Some(102));
    case.machine.exit(&[100]);
    let ended = case.end(ExitObservation::Code(0));
    assert_ne!(ended.end, Some(SessionEnd::Clean));
    assert_ne!(ended.exit_evidence, Some(ExitEvidence::SandboxRelay));

    // 5. A native PCSX2 behind a launcher is not a checked sandbox route.
    let case = launch("PCSX2", "native", true);
    case.machine.start(100, 1, "sh", "/usr/bin/sh", None);
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine.exit(&[100]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));

    // 6. No other real emulator gains anything from the rule.
    for other in ["RPCS3", "PPSSPP", "RetroArch", "DuckStation"] {
        assert!(!relayed_zero_believed(other), "{other}");
    }
}

// ---- Regression tests for the independent review of 2026-11-21 ----

#[test]
fn a_signal_read_directly_from_the_emulators_own_process_warns_for_any_emulator() {
    // EmuWiz started the emulator itself and the kernel reports the signal
    // that ended it. That is an observation, not an interpretation, so it
    // warns whether or not the emulator's exit codes are believed.
    for emulator in [
        "PCSX2",
        "RPCS3",
        "RetroArch",
        "PPSSPP",
        "DuckStation",
        "SomeOtherEmulator",
        "ProbeEmu",
    ] {
        for (exit, fatal) in [
            (
                ExitObservation::Signal {
                    signal: 11,
                    core_dumped: true,
                },
                true,
            ),
            (
                ExitObservation::Signal {
                    signal: 11,
                    core_dumped: false,
                },
                true,
            ),
            (
                ExitObservation::Signal {
                    signal: 6,
                    core_dumped: false,
                },
                true,
            ),
            (
                ExitObservation::Signal {
                    signal: 9,
                    core_dumped: false,
                },
                false,
            ),
            (
                ExitObservation::Signal {
                    signal: 15,
                    core_dumped: false,
                },
                false,
            ),
        ] {
            let case = launch(emulator, "native", false);
            case.machine.start(70, 1, "emu", "/usr/bin/emu", None);
            case.journal
                .mark_started(&case.session, 70, &case.machine)
                .unwrap();
            case.machine.exit(&[70]);
            let ended = case.end(exit);
            assert!(
                matches!(ended.end, Some(SessionEnd::Signal { .. })),
                "{emulator} {exit:?}: {:?}",
                ended.end
            );
            assert_eq!(ended.exit_evidence, Some(ExitEvidence::EmulatorProcess));
            assert!(ended.needs_attention(), "{emulator} {exit:?}");
            let preceding = preceding_session(
                &case.journal.all(),
                "dev",
                emulator,
                "native",
                &[mark("v2")],
            );
            assert_eq!(
                matches!(preceding, PrecedingSession::FatalSignal { .. }),
                fatal,
                "{emulator} {exit:?}: {preceding:?}"
            );
            assert_eq!(
                matches!(preceding, PrecedingSession::ForcedStop { .. }),
                !fatal,
                "{emulator} {exit:?}: {preceding:?}"
            );
            assert_eq!(preceding.advisory(), Advisory::Warning);
            let warning = preceding.warning().unwrap();
            assert!(
                warning.starts_with("Abnormal termination detected"),
                "{warning}"
            );
            // What it must not say.
            let lower = warning.to_lowercase();
            assert!(!lower.contains("crash"), "{warning}");
            assert!(
                lower.contains("does not show that") && lower.contains("save is damaged"),
                "{warning}"
            );
        }
    }
}

#[test]
fn a_non_zero_exit_code_of_an_undemonstrated_emulator_stays_unknown() {
    // A code needs interpreting, and nothing has shown what these emulators
    // return on an ordinary close from inside a game.
    for emulator in [
        "PCSX2",
        "RPCS3",
        "RetroArch",
        "PPSSPP",
        "DuckStation",
        "SomeOtherEmulator",
    ] {
        for code in [1, 2, 134, 139, 255] {
            let case = launch(emulator, "native", false);
            case.machine.start(70, 1, "emu", "/usr/bin/emu", None);
            case.journal
                .mark_started(&case.session, 70, &case.machine)
                .unwrap();
            case.machine.exit(&[70]);
            let ended = case.end(ExitObservation::Code(code));
            assert!(
                matches!(ended.end, Some(SessionEnd::ExitUnsettled { .. })),
                "{emulator} {code}: {:?}",
                ended.end
            );
            assert!(!ended.needs_attention(), "{emulator} {code}");
            let preceding = preceding_session(
                &case.journal.all(),
                "dev",
                emulator,
                "native",
                &[mark("v2")],
            );
            assert!(
                matches!(preceding, PrecedingSession::ExitNotSeen { .. }),
                "{emulator} {code}: {preceding:?}"
            );
            assert_eq!(preceding.advisory(), Advisory::Caution);
            assert_eq!(
                preceding.warning(),
                None,
                "no warning from a code that cannot be interpreted"
            );
        }
        // An exit status of 0 from the emulator's own process is still 0.
        let case = launch(emulator, "native", false);
        case.machine.start(70, 1, "emu", "/usr/bin/emu", None);
        case.journal
            .mark_started(&case.session, 70, &case.machine)
            .unwrap();
        case.machine.exit(&[70]);
        let end = case.end(ExitObservation::Code(0)).end;
        if emulator == "RetroArch" {
            // RetroArch's statuses mean something only from inside a running
            // game (owner decision 2026-11-21). Nothing showed that a game
            // ran here, so even 0 claims nothing.
            assert!(
                matches!(end, Some(SessionEnd::ExitUnsettled { ref detail }) if detail.contains("status 0")),
                "{end:?}"
            );
        } else {
            assert_eq!(end, Some(SessionEnd::Clean), "{emulator}");
        }
    }
}

#[test]
fn a_signal_that_ended_only_a_launcher_or_wrapper_is_not_a_fatal_signal_of_the_emulator() {
    // The kernel does report a signal here, but for the launcher: whatever
    // happened to the emulator behind it was not observed.
    for (emulator, profile) in [
        ("PCSX2", "flatpak"),
        ("RetroArch", "flatpak"),
        ("ProbeEmu", "flatpak"),
        ("PCSX2", "native"),
    ] {
        for signal in [11, 6, 9] {
            let case = launch(emulator, profile, true);
            case.machine.start(50, 1, "bash", "/usr/bin/bash", None);
            case.journal
                .mark_started(&case.session, 50, &case.machine)
                .unwrap();
            case.machine.exit(&[50]);
            let ended = case.end(ExitObservation::Signal {
                signal,
                core_dumped: signal != 9,
            });
            assert!(
                matches!(ended.end, Some(SessionEnd::LauncherEndedOtherwise { .. })),
                "{emulator} {signal}: {:?}",
                ended.end
            );
            assert!(!ended.needs_attention());
            let preceding =
                preceding_session(&case.journal.all(), "dev", emulator, profile, &[mark("v2")]);
            assert!(
                matches!(preceding, PrecedingSession::ExitNotSeen { .. }),
                "{emulator} {signal}: {preceding:?}"
            );
            assert_eq!(preceding.warning(), None);
        }
    }
}

#[test]
fn a_real_exit_status_completes_a_session_that_was_settled_a_moment_too_early() {
    let case = launch("ProbeEmu", "native", false);
    case.machine
        .start(70, 1, "duckstation-qt", "/opt/duckstation-qt", None);
    case.journal
        .mark_started(&case.session, 70, &case.machine)
        .unwrap();
    case.machine.exit(&[70]);
    // Another EmuWiz command settles open sessions while this one is still
    // comparing saves.
    let settled = case
        .journal
        .reconcile(&case.machine, crate::save_continuity::now_unix());
    assert_eq!(settled[0].state, SessionState::EndNotSeen);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.state, SessionState::Ended);
    assert_eq!(ended.end, Some(SessionEnd::Clean));
    assert!(
        !ended
            .notes
            .iter()
            .any(|note| note.contains("did not see it end"))
    );
    // But not twice, and not for a session whose id was reused by a live process.
    assert!(
        case.journal
            .mark_ended(&case.session, ExitObservation::Code(0), None, &case.machine)
            .is_err()
    );
    let other = launch("ProbeEmu", "native", false);
    other
        .machine
        .start(70, 1, "duckstation-qt", "/opt/duckstation-qt", None);
    other
        .journal
        .mark_started(&other.session, 70, &other.machine)
        .unwrap();
    other.machine.exit(&[70]);
    other
        .journal
        .reconcile(&other.machine, crate::save_continuity::now_unix());
    other
        .machine
        .start(70, 1, "something-else", "/usr/bin/something-else", None); // same pid, same fake start time
    assert!(
        other
            .journal
            .mark_ended(
                &other.session,
                ExitObservation::Code(0),
                None,
                &other.machine
            )
            .is_err()
    );
}

#[test]
fn a_session_settled_long_ago_is_never_rewritten_by_a_late_exit_status() {
    let case = launch("ProbeEmu", "native", false);
    case.machine.start(70, 1, "probeemu", "/opt/probeemu", None);
    case.journal
        .mark_started(&case.session, 70, &case.machine)
        .unwrap();
    case.machine.exit(&[70]);
    // Settled at a time far in the past.
    let settled = case.journal.reconcile(&case.machine, 1_000);
    assert_eq!(settled[0].state, SessionState::EndNotSeen);
    assert!(
        case.journal
            .mark_ended(&case.session, ExitObservation::Code(0), None, &case.machine)
            .is_err()
    );
    assert_eq!(
        case.journal.get(&case.session).unwrap().state,
        SessionState::EndNotSeen
    );
}

// ---- RetroArch: demonstrated from inside a running game only ----

const RA_SCOPE: &str = "app-flatpak-org.libretro.RetroArch-100.scope";

/// A RetroArch session. `game` and `core` say whether a game with a core
/// was launched.
fn retroarch(profile: &str, through_launcher: bool, game: bool, core: Option<&str>) -> Case {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path(), "dev").unwrap();
    let launch = SessionLaunch {
        emulator: "RetroArch".into(),
        profile: profile.into(),
        core: core.map(str::to_string),
        through_launcher,
        ..Default::default()
    };
    let played = SessionGame {
        title: "Test Game".into(),
        content_path: game.then(|| PathBuf::from("/games/test.gb")),
        ..Default::default()
    };
    let session = journal
        .request(played, launch, vec![mark("v1")])
        .unwrap()
        .session_id;
    Case {
        _dir: dir,
        journal,
        machine: Machine::default(),
        session,
    }
}

fn retroarch_preceding(case: &Case, profile: &str) -> PrecedingSession {
    preceding_session(
        &case.journal.all(),
        "dev",
        "RetroArch",
        profile,
        &[mark("v2")],
    )
}

const CORE: Option<&str> = Some("gambatte_libretro.so");
const SEEN: &str = "test: the game was observed running";

/// A Flatpak RetroArch session with the real process tree, ended with
/// `code`. `seen` records evidence that the game was observed running.
fn retroarch_flatpak_ends(
    game: bool,
    core: Option<&str>,
    seen: bool,
    code: i32,
) -> (Case, SessionRecord) {
    let case = retroarch("flatpak", true, game, core);
    case.machine
        .start(100, 1, "bwrap", "/usr/bin/bwrap", Some(RA_SCOPE));
    case.journal
        .mark_started(&case.session, 100, &case.machine)
        .unwrap();
    case.machine
        .start(101, 100, "bwrap", "/usr/bin/bwrap", Some(RA_SCOPE));
    case.machine
        .start(102, 101, "retroarch", "/app/bin/retroarch", Some(RA_SCOPE));
    case.look();
    if seen {
        case.journal
            .record_game_seen_running(&case.session, SEEN)
            .unwrap();
    }
    case.machine.exit(&[102, 101, 100]);
    let ended = case.end(ExitObservation::Code(code));
    (case, ended)
}

/// A native RetroArch started directly by EmuWiz, ended with `code`.
fn retroarch_native_ends(
    game: bool,
    core: Option<&str>,
    seen: bool,
    code: i32,
) -> (Case, SessionRecord) {
    let case = retroarch("native", false, game, core);
    case.machine
        .start(70, 1, "retroarch", "/usr/bin/retroarch", None);
    case.journal
        .mark_started(&case.session, 70, &case.machine)
        .unwrap();
    if seen {
        case.journal
            .record_game_seen_running(&case.session, SEEN)
            .unwrap();
    }
    case.machine.exit(&[70]);
    let ended = case.end(ExitObservation::Code(code));
    (case, ended)
}

/// Nothing is claimed: not clean, not abnormal, no warning, never a signal.
fn nothing_is_claimed(case: &Case, ended: &SessionRecord, profile: &str, label: &str) {
    assert_ne!(ended.end, Some(SessionEnd::Clean), "{label}");
    assert!(!ended.end.as_ref().unwrap().abnormal(), "{label}");
    assert!(
        !matches!(ended.end, Some(SessionEnd::Signal { .. })),
        "{label}"
    );
    let preceding = retroarch_preceding(case, profile);
    assert!(!preceding.clean(), "{label}: {preceding:?}");
    assert_eq!(preceding.warning(), None, "{label}");
    assert_eq!(preceding.advisory(), Advisory::Caution, "{label}");
    assert!(
        !matches!(preceding, PrecedingSession::FatalSignal { .. }),
        "{label}"
    );
}

#[test]
fn a_genuinely_observed_retroarch_game_session_reports_its_exit() {
    // Native, started directly by EmuWiz, game seen running.
    let (case, ended) = retroarch_native_ends(true, CORE, true, 0);
    assert_eq!(ended.end, Some(SessionEnd::Clean));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::EmulatorProcess));
    assert_eq!(ended.game_seen_running.as_ref().unwrap().what, SEEN);
    assert!(retroarch_preceding(&case, "native").clean());

    let (case, ended) = retroarch_native_ends(true, CORE, true, 139);
    // An exit code stays an exit code: never a signal, never "crash".
    assert_eq!(ended.end, Some(SessionEnd::ExitCode { code: 139 }));
    let preceding = retroarch_preceding(&case, "native");
    assert!(
        matches!(preceding, PrecedingSession::ErrorExit { code: 139, .. }),
        "{preceding:?}"
    );
    let warning = preceding.warning().unwrap();
    assert!(warning.starts_with("Abnormal exit detected"), "{warning}");
    let lower = warning.to_lowercase();
    assert!(
        !lower.contains("crash") && !lower.contains("signal") && !lower.contains("damaged"),
        "{warning}"
    );

    // Flatpak: the sandbox relays the emulator's status.
    let (case, ended) = retroarch_flatpak_ends(true, CORE, true, 0);
    assert_eq!(ended.end, Some(SessionEnd::Clean));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::SandboxRelay));
    assert!(retroarch_preceding(&case, "flatpak").clean());
    let (case, ended) = retroarch_flatpak_ends(true, CORE, true, 139);
    assert_eq!(ended.end, Some(SessionEnd::ExitCode { code: 139 }));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::SandboxRelay));
    assert_eq!(
        retroarch_preceding(&case, "flatpak").advisory(),
        Advisory::Warning
    );
}

#[test]
fn a_game_that_was_asked_for_but_never_seen_running_leaves_the_exit_unknown() {
    // The game and core were requested. Whether the core loaded the game
    // (or failed and left RetroArch at its menu) was not observed. That is
    // every real session today: nothing in EmuWiz can observe gameplay.
    for code in [0, 139, 134, 1] {
        let label = format!("requested, not seen, {code}");
        let (case, ended) = retroarch_native_ends(true, CORE, false, code);
        assert_eq!(ended.game_seen_running, None);
        // The observed number is kept as diagnostic evidence.
        match &ended.end {
            Some(SessionEnd::ExitUnsettled { detail }) => {
                assert!(detail.contains(&format!("status {code}")), "{detail}");
                assert!(
                    detail.contains("not verified that the game actually ran"),
                    "{detail}"
                );
            }
            other => panic!("{label}: {other:?}"),
        }
        nothing_is_claimed(&case, &ended, "native", &label);

        let (case, ended) = retroarch_flatpak_ends(true, CORE, false, code);
        assert_eq!(
            ended.exit_evidence,
            Some(ExitEvidence::LauncherOnly),
            "{label}"
        );
        if code != 0 {
            assert_eq!(
                ended.end,
                Some(SessionEnd::LauncherEndedOtherwise {
                    detail: format!("exit code {code}")
                }),
                "{label}"
            );
        }
        nothing_is_claimed(&case, &ended, "flatpak", &label);
    }
}

#[test]
fn a_menu_only_retroarch_session_leaves_the_exit_unknown_including_the_unexplained_139() {
    // No game, no core, or an empty core name.
    for (game, core) in [
        (false, None),
        (true, None),
        (false, CORE),
        (true, Some("  ")),
    ] {
        for code in [0, 139, 1] {
            let label = format!("{game} {core:?} {code}");
            let (case, ended) = retroarch_native_ends(game, core, false, code);
            assert!(
                matches!(ended.end, Some(SessionEnd::ExitUnsettled { .. })),
                "{label}: {:?}",
                ended.end
            );
            nothing_is_claimed(&case, &ended, "native", &label);
            let (case, ended) = retroarch_flatpak_ends(game, core, false, code);
            assert_eq!(
                ended.exit_evidence,
                Some(ExitEvidence::LauncherOnly),
                "{label}"
            );
            nothing_is_claimed(&case, &ended, "flatpak", &label);
        }
    }
}

#[test]
fn uncertain_retroarch_sessions_stay_unknown() {
    // Evidence can only be recorded for a running session, and has to say
    // what it is.
    let case = retroarch("native", false, true, CORE);
    assert!(
        case.journal
            .record_game_seen_running(&case.session, SEEN)
            .is_err(),
        "not started yet"
    );
    case.machine
        .start(70, 1, "retroarch", "/usr/bin/retroarch", None);
    case.journal
        .mark_started(&case.session, 70, &case.machine)
        .unwrap();
    assert!(
        case.journal
            .record_game_seen_running(&case.session, "  ")
            .is_err()
    );
    case.machine.exit(&[70]);
    let ended = case.end(ExitObservation::Code(139));
    assert!(
        case.journal
            .record_game_seen_running(&case.session, SEEN)
            .is_err(),
        "already over: evidence cannot be added afterwards"
    );
    nothing_is_claimed(&case, &ended, "native", "evidence refused");

    // A wrapper is still only a wrapper, even with the game seen running:
    // a shell between EmuWiz and the sandbox...
    let case = retroarch("flatpak", true, true, CORE);
    case.machine.start(90, 1, "bash", "/usr/bin/bash", None);
    case.journal
        .mark_started(&case.session, 90, &case.machine)
        .unwrap();
    case.machine
        .start(100, 90, "bwrap", "/usr/bin/bwrap", Some(RA_SCOPE));
    case.machine
        .start(102, 100, "retroarch", "/app/bin/retroarch", Some(RA_SCOPE));
    case.look();
    case.journal
        .record_game_seen_running(&case.session, SEEN)
        .unwrap();
    case.machine.exit(&[102, 100, 90]);
    let ended = case.end(ExitObservation::Code(0));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));
    nothing_is_claimed(&case, &ended, "flatpak", "shell wrapper");
    // ...and a native wrapper script.
    let case = retroarch("native", true, true, CORE);
    case.machine.start(90, 1, "bash", "/usr/bin/bash", None);
    case.journal
        .mark_started(&case.session, 90, &case.machine)
        .unwrap();
    case.machine
        .start(91, 90, "retroarch", "/usr/bin/retroarch", None);
    case.look();
    case.journal
        .record_game_seen_running(&case.session, SEEN)
        .unwrap();
    case.machine.exit(&[91, 90]);
    let ended = case.end(ExitObservation::Code(139));
    assert_eq!(ended.exit_evidence, Some(ExitEvidence::LauncherOnly));
    nothing_is_claimed(&case, &ended, "native", "script wrapper");

    // A signal the kernel reported for RetroArch's own process is a
    // different, direct kind of evidence and needs no gameplay proof.
    let case = retroarch("native", false, false, None);
    case.machine
        .start(70, 1, "retroarch", "/usr/bin/retroarch", None);
    case.journal
        .mark_started(&case.session, 70, &case.machine)
        .unwrap();
    case.machine.exit(&[70]);
    let ended = case.end(ExitObservation::Signal {
        signal: 11,
        core_dumped: true,
    });
    assert_eq!(
        ended.end,
        Some(SessionEnd::Signal {
            signal: 11,
            core_dumped: true
        })
    );
    assert_eq!(
        retroarch_preceding(&case, "native").advisory(),
        Advisory::Warning
    );
}

#[test]
fn seeing_a_game_run_does_not_make_any_other_emulator_believed() {
    for (emulator, binary, exe) in [
        ("PPSSPP", "PPSSPPSDL", "/app/bin/PPSSPPSDL"),
        ("PCSX2", "pcsx2-qt", "/app/bin/pcsx2-qt"),
        ("RPCS3", "rpcs3", "/app/bin/rpcs3"),
    ] {
        for code in [0, 1, 134, 139] {
            if code == 0 && relayed_zero_believed(emulator) {
                // A relayed 0 of PCSX2 is believed by its own rule, whether
                // or not a game was seen: see
                // `a_relayed_zero_from_the_tracked_pcsx2_process_is_a_normal_close`.
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let journal = SessionJournal::open(dir.path(), "dev").unwrap();
            let launch = SessionLaunch {
                emulator: emulator.into(),
                profile: "flatpak".into(),
                core: Some("a_core".into()),
                through_launcher: true,
                ..Default::default()
            };
            let played = SessionGame {
                content_path: Some(PathBuf::from("/games/test.iso")),
                ..Default::default()
            };
            let session = journal
                .request(played, launch, vec![mark("v1")])
                .unwrap()
                .session_id;
            let machine = Machine::default();
            machine.start(100, 1, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
            journal.mark_started(&session, 100, &machine).unwrap();
            machine.start(101, 100, "bwrap", "/usr/bin/bwrap", Some(SCOPE));
            machine.start(102, 101, binary, exe, Some(SCOPE));
            journal.observe_running(&session, &machine).unwrap();
            journal.record_game_seen_running(&session, SEEN).unwrap();
            machine.exit(&[102, 101, 100]);
            let after = [mark("v2")];
            let ended = journal
                .mark_ended(
                    &session,
                    ExitObservation::Code(code),
                    Some(&after),
                    &machine,
                )
                .unwrap();
            let label = format!("{emulator} {code}");
            assert_eq!(
                ended.exit_evidence,
                Some(ExitEvidence::LauncherOnly),
                "{label}"
            );
            assert_ne!(ended.end, Some(SessionEnd::Clean), "{label}");
            assert!(!ended.end.as_ref().unwrap().abnormal(), "{label}");
        }
    }
    // The lists themselves.
    assert_eq!(EXIT_STATUS_DEMONSTRATED_IN_GAME, ["RetroArch"]);
    for emulator in [
        "RetroArch",
        "PPSSPP",
        "PCSX2",
        "RPCS3",
        "DuckStation",
        "Dolphin",
    ] {
        assert!(!exit_status_demonstrated(emulator), "{emulator}");
    }
    // An emulator on neither list keeps the older rule unchanged: its own
    // exit 0, directly observed, is a normal close.
    let case = launch("DuckStation", "native", false);
    case.machine
        .start(70, 1, "duckstation-qt", "/opt/duckstation-qt", None);
    case.journal
        .mark_started(&case.session, 70, &case.machine)
        .unwrap();
    case.machine.exit(&[70]);
    assert_eq!(
        case.end(ExitObservation::Code(0)).end,
        Some(SessionEnd::Clean)
    );
}
