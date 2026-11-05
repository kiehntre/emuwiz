//! Blue game page: the SRAM restore journey through the real UI code. A
//! disposable synthetic cartridge/save only. Worker steps run through the
//! module's own functions with a fixed closure observer (never the live
//! process scan), and their outcomes are delivered as the backend would.
use super::super::save_snapshots::{self, SnapshotReport, snapshot_target};
use super::super::single_file_restore::{self as sfr, Dirs, Flow, Job, Outcome};
use super::*;
use archivefs_core::save_snapshots::tree_restore::safety::{
    DirectorySaveBinding, EmulatorQuiescence, QuiescenceProvider,
};
use archivefs_core::{
    game_identity::{
        IdentityConfidence, IdentityEvidence, IdentityKind, IdentityProvenance, IdentityStatus,
    },
    sram_provenance::{Context, DirectoryIdentity, Object, OriginState, Scope, SramOrigin, Stamp},
};

struct Fixed(EmulatorQuiescence);
impl QuiescenceProvider for Fixed {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        self.0
    }
}
const CLOSED: Fixed = Fixed(EmulatorQuiescence::Closed);
const RUNNING: Fixed = Fixed(EmulatorQuiescence::Running);

struct World {
    dir: tempfile::TempDir,
    rom: std::path::PathBuf,
    save: std::path::PathBuf,
    record: PersistentStateRecord,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let rom = dir.path().join("games/Synthetic.gb");
        let save = dir.path().join("saves/Synthetic.srm");
        std::fs::create_dir_all(rom.parent().unwrap()).unwrap();
        std::fs::create_dir_all(save.parent().unwrap()).unwrap();
        std::fs::write(&rom, b"synthetic cartridge bytes").unwrap();
        std::fs::write(&save, b"CHECKPOINT-ONE").unwrap();
        let stamp = Stamp {
            device: 1,
            inode: 2,
            size: 8192,
            modified: 0,
            modified_ns: 0,
            changed: 0,
            changed_ns: 0,
            links: 1,
            mode: 0o100600,
            uid: 1,
        };
        let object = Object {
            path: rom.clone(),
            stamp,
            sha256: "synthetic-rom-digest".into(),
        };
        let identity = IdentityEvidence {
            kind: IdentityKind::LooseRomSha256,
            status: IdentityStatus::Verified,
            value: Some(object.sha256.clone()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: rom.clone(),
                member_path: None,
                member_index: None,
                method: "fixture".into(),
            },
            diagnostic: "fixture".into(),
        };
        let mut record = state_record(PersistentStateType::NativeSave, StateEmulator::RetroArch);
        record.path = save.clone();
        record.sram_origin = Some(SramOrigin {
            state: OriginState::Associated,
            session: "session".into(),
            context: Context {
                cartridge: object.clone(),
                cartridge_identity: identity,
                core: object.clone(),
                executable: object.clone(),
                scope: Scope {
                    selected_executable: rom.clone(),
                    selected_core: rom.clone(),
                    configuration: object.clone(),
                    saves: dir.path().join("saves"),
                    saves_identity: DirectoryIdentity {
                        device: 1,
                        inode: 3,
                    },
                    profile: "Native/User".into(),
                },
            },
            artifact: object,
            explanation: "witness".into(),
        });
        Self {
            dir,
            rom,
            save,
            record,
        }
    }

    fn game(&self) -> Game {
        let mut archive = archive(1, "Synthetic", Some("Game Boy"));
        archive.absolute_path = self.rom.clone();
        Game::from_archive(archive)
    }

    fn dirs(&self) -> Dirs {
        Dirs {
            backup_root: self.dir.path().join("restore-backups"),
            receipt_root: self.dir.path().join("restore-receipts"),
        }
    }

    fn root(&self) -> std::path::PathBuf {
        self.dir.path().join("snapshots")
    }

    /// A fresh app, as after a restart: history is read from disk.
    fn app(&self, context: &egui::Context) -> App {
        let mut app = fixture(context);
        app.library = Arc::new(Library::new(vec![{
            let mut archive = archive(1, "Synthetic", Some("Game Boy"));
            archive.absolute_path = self.rom.clone();
            archive
        }]));
        app.saves_states.inventory = Some(saves_inventory(vec![self.record.clone()]));
        app.saves_states
            .single_file
            .dirs_for_tests(Some(self.dirs()));
        let panel = &mut app.saves_states.snapshots;
        panel.root = Some(self.root());
        panel.root_resolved = true;
        self.reload_history(&mut app);
        app
    }

    fn reload_history(&self, app: &mut App) {
        let target = snapshot_target(&self.record, &self.game()).unwrap();
        let dirs = self.dirs();
        let report = save_snapshots::load_history(
            &self.root(),
            &[target],
            &CLOSED,
            Some((&dirs.receipt_root, &dirs.backup_root)),
        );
        let generation = app.saves_states.generation;
        app.saves_states.snapshots.report = Some(report);
        app.saves_states.snapshots.loaded_for = Some((1, generation));
    }
}

fn game_page(context: &egui::Context, app: &mut App) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 2400.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                super::super::saves_states::show(app, ui, Some(1));
            });
        },
    )
}

fn shown(context: &egui::Context, app: &mut App) -> Vec<String> {
    text(&game_page(context, app))
}

fn has(strings: &[String], wanted: &str) -> bool {
    strings.iter().any(|value| value.contains(wanted))
}

/// A real pointer click on the visible label. No backend job is started by
/// the labels clicked here (Cancel).
fn click(context: &egui::Context, app: &mut App, label: &str) {
    let first = game_page(context, app);
    let point = text_bounds(&first, label)
        .first()
        .unwrap_or_else(|| panic!("{label} is visible: {:?}", text(&first)))
        .center();
    for pressed in [true, false] {
        let mut events = vec![egui::Event::PointerMoved(point)];
        events.push(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 2400.0),
                )),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    super::super::saves_states::show(app, ui, Some(1));
                });
            },
        );
    }
}

fn prepare(world: &World, app: &mut App, observer: &dyn QuiescenceProvider) {
    let target = snapshot_target(&world.record, &world.game()).unwrap();
    let snapshot = app.saves_states.snapshots.report.as_ref().unwrap().records[&world.save].entries
        [0]
    .snapshot
    .clone();
    let outcome = sfr::run_with(
        Job::Prepare {
            snapshot: Box::new(snapshot),
            target: Box::new(target),
        },
        observer,
    );
    sfr::receive(app, 0, outcome);
}

#[test]
fn sram_restore_and_undo_journey_through_the_game_page_including_restart() {
    let context = egui::Context::default();
    let world = World::new();
    let target = snapshot_target(&world.record, &world.game()).unwrap();
    // A verified snapshot of the good save, then play on.
    save_snapshots::create(&world.root(), &target, &CLOSED).unwrap();
    std::fs::write(&world.save, b"CHECKPOINT-TWO-LATER").unwrap();
    // Played on after the snapshot (whole-second timestamps).
    std::fs::File::options()
        .write(true)
        .open(&world.save)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(120))
        .unwrap();
    let mut app = world.app(&context);

    // The snapshot is listed and Restore is offered as a preview, not a write.
    let strings = shown(&context, &mut app);
    for expected in ["Verified", "Preview restore", "Create snapshot"] {
        assert!(has(&strings, expected), "missing {expected}: {strings:?}");
    }
    assert!(!has(&strings, "Confirm restore"));

    // Preview names exactly what changes and offers Confirm / Cancel.
    prepare(&world, &mut app, &CLOSED);
    let strings = shown(&context, &mut app);
    for expected in [
        "Review this restore",
        "Synthetic.srm",
        "checksum verified",
        "Your current save: 20 bytes",
        "replaces this one file",
        "No other save is touched",
        "newer than this snapshot",
        "Confirm restore",
        "Cancel",
    ] {
        assert!(has(&strings, expected), "missing {expected}: {strings:?}");
    }
    assert_eq!(std::fs::read(&world.save).unwrap(), b"CHECKPOINT-TWO-LATER");

    // Cancel is a real click and writes nothing.
    click(&context, &mut app, "Cancel");
    assert!(matches!(app.saves_states.single_file.flow, Flow::Idle));
    assert_eq!(std::fs::read(&world.save).unwrap(), b"CHECKPOINT-TWO-LATER");
    assert!(!world.dirs().backup_root.exists());

    // Confirm (worker with the fixed observer): contents verified.
    prepare(&world, &mut app, &CLOSED);
    let Flow::Confirm(preview) = app.saves_states.single_file.flow.clone() else {
        panic!("preview expected")
    };
    let outcome = sfr::run_with(
        Job::Apply {
            preview,
            dirs: world.dirs(),
        },
        &CLOSED,
    );
    sfr::receive(&mut app, 0, outcome);
    assert_eq!(std::fs::read(&world.save).unwrap(), b"CHECKPOINT-ONE");
    let strings = shown(&context, &mut app);
    for expected in ["Save restored and verified", "Undo this restore"] {
        assert!(has(&strings, expected), "missing {expected}: {strings:?}");
    }

    // Restart: a fresh app finds the Undo from the journal alone.
    drop(app);
    let mut restarted = world.app(&context);
    let strings = shown(&context, &mut restarted);
    assert!(has(&strings, "Undo last restore"), "{strings:?}");
    let receipt = restarted
        .saves_states
        .snapshots
        .report
        .as_ref()
        .unwrap()
        .records[&world.save]
        .undoable
        .clone()
        .expect("undoable receipt after restart");

    // Undo (worker): original contents verified, and no Undo is offered again.
    let outcome = sfr::run_with(
        Job::Undo {
            receipt,
            target: Box::new(target),
        },
        &CLOSED,
    );
    sfr::receive(&mut restarted, 0, outcome);
    assert_eq!(std::fs::read(&world.save).unwrap(), b"CHECKPOINT-TWO-LATER");
    assert!(has(&shown(&context, &mut restarted), "Undo complete"));
    world.reload_history(&mut restarted);
    assert!(
        restarted
            .saves_states
            .snapshots
            .report
            .as_ref()
            .unwrap()
            .records[&world.save]
            .undoable
            .is_none(),
        "an undone restore is not offered for Undo again"
    );
}

#[test]
fn a_running_or_unknown_emulator_blocks_preview_confirm_restore_and_undo() {
    let context = egui::Context::default();
    let world = World::new();
    let target = snapshot_target(&world.record, &world.game()).unwrap();
    save_snapshots::create(&world.root(), &target, &CLOSED).unwrap();
    std::fs::write(&world.save, b"CHECKPOINT-TWO-LATER").unwrap();
    let mut app = world.app(&context);

    // Preview while running: the reason is shown and Confirm is not offered.
    prepare(&world, &mut app, &RUNNING);
    let strings = shown(&context, &mut app);
    assert!(has(&strings, "Blocked:"), "{strings:?}");
    let Flow::Confirm(preview) = app.saves_states.single_file.flow.clone() else {
        panic!("preview expected")
    };
    assert!(!preview.preflight.ready());

    // Even a forced Apply is refused at the moment of the write.
    for state in [
        EmulatorQuiescence::Running,
        EmulatorQuiescence::Unknown,
        EmulatorQuiescence::CardInUse,
    ] {
        let outcome = sfr::run_with(
            Job::Apply {
                preview: preview.clone(),
                dirs: world.dirs(),
            },
            &Fixed(state),
        );
        let Outcome::Applied { result, .. } = outcome else {
            unreachable!()
        };
        assert!(result.is_err(), "{state:?}");
        assert_eq!(std::fs::read(&world.save).unwrap(), b"CHECKPOINT-TWO-LATER");
    }

    // A restore that did happen cannot be undone while the emulator runs or is ambiguous.
    prepare(&world, &mut app, &CLOSED);
    let Flow::Confirm(ok) = app.saves_states.single_file.flow.clone() else {
        panic!()
    };
    let Outcome::Applied { result, .. } = sfr::run_with(
        Job::Apply {
            preview: ok,
            dirs: world.dirs(),
        },
        &CLOSED,
    ) else {
        unreachable!()
    };
    let (receipt, _) = result.unwrap();
    for state in [
        EmulatorQuiescence::Running,
        EmulatorQuiescence::Unknown,
        EmulatorQuiescence::CardInUse,
    ] {
        let outcome = sfr::run_with(
            Job::Undo {
                receipt: receipt.clone(),
                target: Box::new(target.clone()),
            },
            &Fixed(state),
        );
        let Outcome::Undone { result, .. } = outcome else {
            unreachable!()
        };
        assert!(result.is_err(), "{state:?}");
        assert_eq!(std::fs::read(&world.save).unwrap(), b"CHECKPOINT-ONE");
    }
}

#[test]
fn only_witnessed_sram_of_this_game_is_offered_restore() {
    let world = World::new();
    let game = world.game();
    assert!(snapshot_target(&world.record, &game).is_ok());
    // Another cartridge's file, an unwitnessed save and a savestate never reach it.
    let mut other = game.clone();
    other.archive.absolute_path = "/games/other/Other.gb".into();
    assert!(snapshot_target(&world.record, &other).is_err());
    let mut plain = world.record.clone();
    plain.sram_origin = None;
    let plain_target = snapshot_target(&plain, &game);
    assert!(
        plain_target.is_err() || !sfr::restorable(&plain_target.unwrap()),
        "an unwitnessed file must never be typed as SRAM"
    );
    let mut state = world.record.clone();
    state.state_type = PersistentStateType::SaveState;
    let state_target = snapshot_target(&state, &game);
    assert!(state_target.is_err() || !sfr::restorable(&state_target.unwrap()));
}
