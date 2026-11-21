//! Crash-awareness: what a snapshot says about the session before it, and
//! that "copied safely" is never shown as "the save is good".

use super::super::model::{AssociationStrength, DiscoveredSave, InUse, SaveFormat, SaveIdentity};
use super::super::session::*;
use super::super::status::*;
use super::super::store::*;
use super::super::workflow::{
    advisory_of, status_lines, structure_of, timelines, uncertainty_of, warning_of,
};
use super::*;
use std::cell::Cell;

/// One process, alive or not.
struct Alive(Cell<bool>);

impl ProcessTable for Alive {
    fn start_ticks(&self, _: u32) -> Option<u64> {
        self.0.get().then_some(7)
    }
}

const DEVICE: &str = "dev";

fn mark(id: &str, content: &str) -> SaveMark {
    SaveMark {
        save_id: id.to_string(),
        label: id.to_string(),
        content_sha256: content.to_string(),
    }
}

struct World {
    _dir: tempfile::TempDir,
    journal: SessionJournal,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path(), DEVICE).unwrap();
        Self { _dir: dir, journal }
    }

    /// Records one whole session of `emulator` that saw `before`, ended with
    /// `exit`, and (when `after` is given) compared the saves afterwards.
    fn session(
        &self,
        emulator: &str,
        profile: &str,
        before: &[SaveMark],
        exit: ExitObservation,
        after: Option<&[SaveMark]>,
    ) -> SessionRecord {
        let table = Alive(Cell::new(true));
        let launch = SessionLaunch {
            emulator: emulator.to_string(),
            profile: profile.to_string(),
            ..Default::default()
        };
        let game = SessionGame {
            title: "Synthetic".into(),
            ..Default::default()
        };
        let session = self.journal.request(game, launch, before.to_vec()).unwrap();
        self.journal
            .mark_started(&session.session_id, 100, &table)
            .unwrap();
        table.0.set(false);
        self.journal
            .mark_ended(&session.session_id, exit, after, &table)
            .unwrap()
    }

    fn preceding(&self, emulator: &str, profile: &str, current: &[SaveMark]) -> PrecedingSession {
        preceding_session(&self.journal.all(), DEVICE, emulator, profile, current)
    }
}

fn capture(bench: &Bench, file: &Path, time: i64, preceding: PrecedingSession) -> CaptureOutcome {
    let mut ask = request(DEVICE, &[file], time);
    ask.preceding_session = preceding;
    bench.store.capture(&ask, &CLOSED).unwrap()
}

#[test]
fn after_a_clean_exit_the_snapshot_says_so_and_still_does_not_call_the_save_good() {
    let world = World::new();
    let now = [mark("save-1", "v2")];
    let session = world.session(
        "PCSX2",
        "native",
        &[mark("save-1", "v1")],
        ExitObservation::Code(0),
        Some(&now),
    );
    let preceding = world.preceding("PCSX2", "native", &now);
    assert_eq!(
        preceding,
        PrecedingSession::CleanExit {
            session_id: session.session_id
        }
    );
    assert_eq!(preceding.advisory(), Advisory::None);
    assert_eq!(preceding.warning(), None);

    let bench = Bench::new();
    let file = bench.file("card.ps2", b"progress");
    let manifest = capture(&bench, &file, 10, preceding).manifest().clone();
    assert!(manifest.body.clean_recovery_point());
    assert_eq!(warning_of(&manifest), None);
    // Three separate statuses and the context, never one "verified".
    let lines = status_lines(&manifest);
    assert_eq!(lines.len(), 4);
    assert!(lines[0].starts_with("Copy: Taken with the emulator closed"));
    assert!(lines[1].starts_with("Structure not checked"));
    assert!(
        lines[2].starts_with("Game data not checked"),
        "{}",
        lines[2]
    );
    assert!(lines[3].contains("closed normally"));
    assert!(matches!(
        manifest.body.game_integrity,
        Some(GameIntegrity::NotChecked { .. })
    ));
    let text = format!("{lines:?}").to_lowercase();
    assert!(
        !text.contains("known good")
            && !text.contains("known-good")
            && !text.contains("save is valid")
    );
}

#[test]
fn a_crash_during_a_write_is_copied_faithfully_kept_and_loudly_labelled() {
    let world = World::new();
    let bench = Bench::new();
    let file = bench.file("card.ps2", b"a complete save, 24 bytes");

    // A clean session first, and a recovery point from it.
    let v1 = [mark("save-1", "v1")];
    world.session("PCSX2", "native", &v1, ExitObservation::Code(0), Some(&v1));
    let good = capture(&bench, &file, 10, world.preceding("PCSX2", "native", &v1))
        .manifest()
        .clone();
    assert!(good.body.clean_recovery_point());

    // The emulator dies with a segmentation fault half-way through writing.
    fs::write(&file, b"a compl").unwrap();
    let crashed = world.session(
        "PCSX2",
        "native",
        &v1,
        ExitObservation::Signal {
            signal: 11,
            core_dumped: true,
        },
        None, // nothing was compared: there was no orderly end
    );
    let preceding = world.preceding("PCSX2", "native", &[mark("save-1", "torn")]);
    assert_eq!(
        preceding,
        PrecedingSession::Crashed {
            session_id: crashed.session_id,
            signal: 11
        }
    );

    // The emulator is closed now, so the *copy* is safe. That is all it is.
    let suspect = capture(&bench, &file, 20, preceding).manifest().clone();
    assert_eq!(
        suspect.body.boundary,
        CaptureBoundary::EmulatorClosedObserved
    );
    assert!(!suspect.body.clean_recovery_point());
    assert_eq!(advisory_of(&suspect), Advisory::Warning);
    let warning = warning_of(&suspect).unwrap();
    assert!(
        warning.starts_with("WARNING: the emulator crashed (signal 11)"),
        "{warning}"
    );
    assert!(warning.contains("half-written"));

    // Kept, not discarded; recoverable for inspection; clearly labelled.
    assert!(bench.store.verify(&suspect.snapshot_id).unwrap().intact());
    let destination = bench.dir.path().join("inspect");
    let plan = bench
        .store
        .plan_recovery(&suspect.snapshot_id, &destination)
        .unwrap();
    assert!(plan.blockers.is_empty());
    assert!(
        plan.notes
            .iter()
            .any(|note| note.starts_with("WARNING: the emulator crashed"))
    );
    assert!(
        plan.notes
            .iter()
            .any(|note| note.contains("does not make it a good save"))
    );
    bench
        .store
        .recover_to_new(&suspect.snapshot_id, &destination)
        .unwrap();
    assert_eq!(fs::read(destination.join("card.ps2")).unwrap(), b"a compl");

    // The earlier recovery point is untouched and still the clean one.
    let still = bench.store.get(&good.snapshot_id).unwrap();
    assert_eq!(still, good);
    assert!(bench.store.verify(&good.snapshot_id).unwrap().intact());
    assert_eq!(
        bench.store.read_object(&good.body.files[0].sha256).unwrap(),
        b"a complete save, 24 bytes"
    );
    let timeline = &timelines(&bench.store)[0];
    assert_eq!(timeline.entries.len(), 2);
    assert_eq!(timeline.entries[0].warning, None);
    assert_eq!(timeline.entries[0].advisory, Advisory::None);
    assert_eq!(timeline.entries[1].advisory, Advisory::Warning);
}

#[test]
fn a_forced_stop_an_error_exit_and_a_crash_are_told_apart() {
    for (exit, expect_crash, expect_forced) in [
        (
            ExitObservation::Signal {
                signal: 9,
                core_dumped: false,
            },
            false,
            true,
        ),
        (
            ExitObservation::Signal {
                signal: 15,
                core_dumped: false,
            },
            false,
            true,
        ),
        (
            ExitObservation::Signal {
                signal: 6,
                core_dumped: false,
            },
            true,
            false,
        ),
        (
            ExitObservation::Signal {
                signal: 15,
                core_dumped: true,
            },
            true,
            false,
        ),
    ] {
        let world = World::new();
        let now = [mark("save-1", "v1")];
        world.session("RetroArch", "native", &now, exit, None);
        let preceding = world.preceding("RetroArch", "native", &now);
        assert_eq!(
            matches!(preceding, PrecedingSession::Crashed { .. }),
            expect_crash,
            "{exit:?}"
        );
        assert_eq!(
            matches!(preceding, PrecedingSession::ForcedStop { .. }),
            expect_forced,
            "{exit:?}"
        );
        assert_eq!(preceding.advisory(), Advisory::Warning);
        assert!(preceding.warning().unwrap().starts_with("WARNING"));
    }
    let world = World::new();
    let now = [mark("save-1", "v1")];
    world.session(
        "RetroArch",
        "native",
        &now,
        ExitObservation::Code(3),
        Some(&now),
    );
    let preceding = world.preceding("RetroArch", "native", &now);
    assert!(matches!(
        preceding,
        PrecedingSession::ErrorExit { code: 3, .. }
    ));
    assert_eq!(preceding.advisory(), Advisory::Warning);
}

#[test]
fn an_exit_emuwiz_did_not_see_is_a_warning_not_a_clean_exit() {
    // EmuWiz was closed when the emulator stopped.
    let world = World::new();
    let now = [mark("save-1", "v1")];
    let table = Alive(Cell::new(true));
    let launch = SessionLaunch {
        emulator: "DuckStation".into(),
        profile: "native".into(),
        ..Default::default()
    };
    let session = world
        .journal
        .request(SessionGame::default(), launch, now.to_vec())
        .unwrap();
    world
        .journal
        .mark_started(&session.session_id, 5, &table)
        .unwrap();
    table.0.set(false);
    world.journal.reconcile(&table, 0);
    let unseen = world.preceding("DuckStation", "native", &now);
    assert!(
        matches!(unseen, PrecedingSession::ExitNotSeen { .. }),
        "{unseen:?}"
    );
    // Not knowing how it ended is uncertainty, not a crash warning.
    assert_eq!(unseen.advisory(), Advisory::Caution);
    assert_eq!(unseen.warning(), None);
    assert!(unseen.uncertainty().unwrap().starts_with("Not known"));

    // Only a launcher's exit was seen (flatpak run): its 0 is not the emulator's.
    let world = World::new();
    let table = Alive(Cell::new(true));
    let launch = SessionLaunch {
        emulator: "RPCS3".into(),
        profile: "flatpak".into(),
        through_launcher: true,
        ..Default::default()
    };
    let session = world
        .journal
        .request(SessionGame::default(), launch, now.to_vec())
        .unwrap();
    world
        .journal
        .mark_started(&session.session_id, 5, &table)
        .unwrap();
    table.0.set(false);
    world
        .journal
        .mark_ended(
            &session.session_id,
            ExitObservation::Code(0),
            Some(&now),
            &table,
        )
        .unwrap();
    let launcher = world.preceding("RPCS3", "flatpak", &now);
    assert!(
        matches!(launcher, PrecedingSession::ExitNotSeen { .. }),
        "{launcher:?}"
    );

    // An unreadable exit status is not a clean one either.
    let world = World::new();
    world.session(
        "PPSSPP",
        "flatpak",
        &now,
        ExitObservation::Unavailable,
        Some(&now),
    );
    assert!(matches!(
        world.preceding("PPSSPP", "flatpak", &now),
        PrecedingSession::ExitNotSeen { .. }
    ));
}

#[test]
fn stale_session_information_is_never_used() {
    let v1 = [mark("save-1", "v1")];

    // The save changed after the last recorded session ended: something
    // wrote it that EmuWiz did not see.
    let world = World::new();
    world.session("PCSX2", "native", &v1, ExitObservation::Code(0), Some(&v1));
    let changed_since = world.preceding("PCSX2", "native", &[mark("save-1", "v2-from-somewhere")]);
    assert!(
        matches!(changed_since, PrecedingSession::Unknown { ref why } if why.contains("changed since")),
        "{changed_since:?}"
    );

    // A clean session recorded this save, then a newer session of the same
    // emulator ran without recording it. The older "clean" no longer applies.
    let world = World::new();
    world.session("PCSX2", "native", &v1, ExitObservation::Code(0), Some(&v1));
    let other = [mark("another-save", "x")];
    world.session(
        "PCSX2",
        "native",
        &other,
        ExitObservation::Signal {
            signal: 11,
            core_dumped: false,
        },
        None,
    );
    let superseded = world.preceding("PCSX2", "native", &v1);
    assert!(
        matches!(superseded, PrecedingSession::Unknown { ref why } if why.contains("did not record this save")),
        "{superseded:?}"
    );

    // A normal exit where the saves were never compared afterwards.
    let world = World::new();
    world.session("PCSX2", "native", &v1, ExitObservation::Code(0), None);
    assert!(matches!(
        world.preceding("PCSX2", "native", &v1),
        PrecedingSession::Unknown { .. }
    ));

    // A session still recorded as open.
    let world = World::new();
    let launch = SessionLaunch {
        emulator: "PCSX2".into(),
        profile: "native".into(),
        ..Default::default()
    };
    world
        .journal
        .request(SessionGame::default(), launch, v1.to_vec())
        .unwrap();
    assert!(
        matches!(world.preceding("PCSX2", "native", &v1), PrecedingSession::Unknown { ref why } if why.contains("still recorded as open"))
    );

    // No sessions at all.
    let world = World::new();
    let none = world.preceding("PCSX2", "native", &v1);
    assert!(matches!(none, PrecedingSession::Unknown { .. }));
    assert_eq!(none.advisory(), Advisory::Caution);
    assert_eq!(none.warning(), None);
    assert!(
        none.uncertainty()
            .unwrap()
            .contains("nothing rules one out")
    );
}

#[test]
fn a_session_for_a_different_emulator_install_device_or_save_says_nothing() {
    let world = World::new();
    let v1 = [mark("save-1", "v1")];
    world.session("PCSX2", "native", &v1, ExitObservation::Code(0), Some(&v1));
    assert!(world.preceding("PCSX2", "native", &v1).clean());

    for (emulator, profile, current) in [
        ("RetroArch", "native", &v1[..]),                 // another emulator
        ("PCSX2", "flatpak", &v1[..]),                    // the other install of the same emulator
        ("PCSX2", "native", &[mark("save-2", "v1")][..]), // another save, same content
        ("PCSX2", "native", &[][..]),                     // a save with no identity
    ] {
        let preceding = world.preceding(emulator, profile, current);
        assert!(
            matches!(preceding, PrecedingSession::Unknown { .. }),
            "{emulator} {profile}: {preceding:?}"
        );
    }
    // Another device's session, even if it is in the list.
    let foreign = preceding_session(&world.journal.all(), "other-device", "PCSX2", "native", &v1);
    assert!(matches!(foreign, PrecedingSession::Unknown { .. }));
    // Two saves on one card: the session must have recorded both.
    let both = [mark("save-1", "v1"), mark("save-9", "v1")];
    assert!(matches!(
        world.preceding("PCSX2", "native", &both),
        PrecedingSession::Unknown { .. }
    ));

    // A launch that never produced a process wrote nothing: it is skipped.
    let launch = SessionLaunch {
        emulator: "PCSX2".into(),
        profile: "native".into(),
        ..Default::default()
    };
    let failed = world
        .journal
        .request(SessionGame::default(), launch, v1.to_vec())
        .unwrap();
    world
        .journal
        .mark_never_started(&failed.session_id, "emulator not found")
        .unwrap();
    assert!(world.preceding("PCSX2", "native", &v1).clean());
}

#[test]
fn the_same_bytes_after_a_crash_are_a_separate_snapshot_and_nothing_is_replaced() {
    let bench = Bench::new();
    let file = bench.file("save.srm", b"unchanged bytes");
    let clean = capture(
        &bench,
        &file,
        10,
        PrecedingSession::CleanExit {
            session_id: "s1".into(),
        },
    );
    let after_crash = capture(
        &bench,
        &file,
        20,
        PrecedingSession::Crashed {
            session_id: "s2".into(),
            signal: 11,
        },
    );
    assert!(
        matches!(after_crash, CaptureOutcome::Created { new_bytes: 0, .. }),
        "shares the stored content, has its own record"
    );
    assert_ne!(
        clean.manifest().snapshot_id,
        after_crash.manifest().snapshot_id
    );
    // Asked again with nothing new known: no third record.
    let again = capture(
        &bench,
        &file,
        30,
        PrecedingSession::Crashed {
            session_id: "s2".into(),
            signal: 11,
        },
    );
    assert!(matches!(again, CaptureOutcome::Unchanged { .. }));
    let all = bench.store.list();
    assert_eq!(all.len(), 2);
    assert!(all[0].body.clean_recovery_point() && !all[1].body.clean_recovery_point());
    assert_eq!(bench.store.usage().objects, 1);
}

#[test]
fn pruning_never_proposes_the_newest_clean_recovery_point() {
    let bench = Bench::new();
    let file = bench.file("save.srm", b"0");
    fs::write(&file, b"good").unwrap();
    let good = capture(
        &bench,
        &file,
        1,
        PrecedingSession::CleanExit {
            session_id: "s1".into(),
        },
    )
    .manifest()
    .snapshot_id
    .clone();
    // Then five suspect snapshots, each different.
    for round in 2..=6 {
        fs::write(&file, format!("suspect {round}")).unwrap();
        capture(
            &bench,
            &file,
            round,
            PrecedingSession::Crashed {
                session_id: format!("s{round}"),
                signal: 11,
            },
        );
    }
    let before = tree(bench.store.root());
    let proposal = bench.store.pruning_proposal(2);
    assert_eq!(
        proposal.candidates.len(),
        3,
        "the three oldest suspect ones"
    );
    assert!(
        proposal
            .candidates
            .iter()
            .all(|candidate| candidate.snapshot_id != good)
    );
    assert!(proposal.nothing_was_deleted);
    assert_eq!(tree(bench.store.root()), before);
}

#[test]
fn a_record_written_before_these_fields_existed_still_loads_and_claims_nothing() {
    let bench = Bench::new();
    let file = bench.file("save.srm", b"old");
    let manifest = capture(&bench, &file, 1, PrecedingSession::unknown("x"))
        .manifest()
        .clone();
    // Rebuild the record as an older EmuWiz would have written it.
    let mut body = manifest.body.clone();
    body.captured_unix = 2;
    body.structure = None;
    body.game_integrity = None;
    body.preceding_session = None;
    let old = SnapshotManifest {
        format_version: 1,
        snapshot_id: body_id(&body),
        body,
    };
    let text = serde_json::to_string(&old).unwrap();
    assert!(!text.contains("preceding_session") && !text.contains("game_integrity"));
    assert!(bench.store.put_manifest(&old).unwrap());
    let loaded = bench.store.get(&old.snapshot_id).unwrap();
    assert!(!loaded.body.clean_recovery_point());
    assert_eq!(advisory_of(&loaded), Advisory::Caution);
    assert_eq!(warning_of(&loaded), None);
    assert!(uncertainty_of(&loaded).unwrap().contains("not recorded"));
}

fn found(format: SaveFormat, integrity: &[&str], unsupported: &[&str]) -> DiscoveredSave {
    DiscoveredSave {
        id: "0123456789abcdef".into(),
        source_path: PathBuf::from("/synthetic/save"),
        entry_name: None,
        emulator: "Any".into(),
        profile: "native".into(),
        emulator_version: None,
        platform: None,
        format,
        artifact_type: format.artifact_type(),
        size_bytes: 1,
        content_sha256: Some("aa".into()),
        container_sha256: None,
        modified_unix: None,
        internal_modified: None,
        identity: SaveIdentity {
            strength: AssociationStrength::None,
            ..SaveIdentity::none("synthetic")
        },
        in_use: InUse::NotDetected,
        integrity: integrity.iter().map(|line| line.to_string()).collect(),
        notes: Vec::new(),
        unsupported: unsupported.iter().map(|line| line.to_string()).collect(),
    }
}

#[test]
fn structure_results_claim_only_what_was_checked() {
    // Memory cards: a real structural check, and it says where it stops.
    let StructureCheck::Passed { what } = structure_of(&found(
        SaveFormat::Ps2CardSave,
        &["two independent card readers list the same saves"],
        &[],
    )) else {
        panic!("a clean card passes its structural check");
    };
    assert!(
        what.contains("two independent readers")
            && what.contains("game's own data inside each save was not checked")
    );
    assert!(matches!(
        structure_of(&found(
            SaveFormat::Ps1CardSave,
            &["the two card readers DISAGREE about this card"],
            &[]
        )),
        StructureCheck::Problems { .. }
    ));
    assert!(matches!(
        structure_of(&found(
            SaveFormat::Ps2CardSave,
            &[],
            &[super::super::discover::CARD_HAS_WARNINGS]
        )),
        StructureCheck::Problems { .. }
    ));
    // GameCube: only the length. Its checksums are named as not checked.
    let StructureCheck::Passed { what } = structure_of(&found(SaveFormat::GameCubeGci, &[], &[]))
    else {
        panic!();
    };
    assert!(
        what.contains("length") && what.contains("checksums") && what.contains("nothing inside")
    );
    // PS3, PSP, cartridge saves and states: no structural claim at all.
    for format in [
        SaveFormat::Ps3Savedata,
        SaveFormat::PspSavedata,
        SaveFormat::CartridgeBattery,
        SaveFormat::SaveState,
    ] {
        assert!(
            matches!(
                structure_of(&found(format, &[], &[])),
                StructureCheck::NotChecked { .. }
            ),
            "{format:?}"
        );
    }

    // A card that does not read cleanly is a warning whatever the session said.
    let bench = Bench::new();
    let file = bench.file("card.ps2", b"x");
    let mut ask = request(DEVICE, &[&file], 1);
    ask.structure = StructureCheck::Problems {
        what: "the memory card does not read cleanly".into(),
    };
    ask.preceding_session = PrecedingSession::CleanExit {
        session_id: "s".into(),
    };
    let manifest = bench
        .store
        .capture(&ask, &CLOSED)
        .unwrap()
        .manifest()
        .clone();
    assert_eq!(advisory_of(&manifest), Advisory::Warning);
    assert!(!manifest.body.clean_recovery_point());
    assert!(
        warning_of(&manifest)
            .unwrap()
            .starts_with("WARNING: the memory card does not read cleanly")
    );
}
