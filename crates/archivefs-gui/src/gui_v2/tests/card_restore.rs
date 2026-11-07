//! Blue Saves page: PCSX2 card snapshots and one-folder restore. Disposable
//! synthetic cards only; the Apply step is driven through the worker
//! functions with a fixed closure observer, never the live process scan.
use super::super::card_restore::{
    self, CARD_RESTORE_WHERE, Flow, Outcome, Paths, SAVESTATE_RESTORE_UNSUPPORTED, WorkDirs,
};
use super::super::save_snapshots::restore_unavailable;
use super::*;
use crate::pcsx2_page::ps2_inventory_fixture;
use archivefs_core::{
    memory_card_inventory::restore_guard::Ps2RestoreGuardOptions,
    save_snapshots::{
        SaveSnapshot,
        tree_restore::safety::{DirectorySaveBinding, EmulatorQuiescence, QuiescenceProvider},
    },
};

struct Closed;
impl QuiescenceProvider for Closed {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        EmulatorQuiescence::Closed
    }
}

struct World {
    _dir: tempfile::TempDir,
    card: std::path::PathBuf,
    record: PersistentStateRecord,
    paths: Paths,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("Mcd001.ps2");
        std::fs::write(&card, ps2_inventory_fixture()).unwrap();
        let mut record = state_record(PersistentStateType::MemoryCard, StateEmulator::Pcsx2);
        record.path = card.clone();
        let journal_dir = dir.path().join("journals");
        std::fs::create_dir(&journal_dir).unwrap();
        let paths = Paths {
            snapshot_root: dir.path().join("snapshots"),
            dirs: WorkDirs {
                psu_dir: dir.path().join("work/psu"),
                backup_dir: dir.path().join("work/backups"),
            },
            journal_dir,
        };
        Self {
            _dir: dir,
            card,
            record,
            paths,
        }
    }

    fn location(&self) -> archivefs_core::save_snapshots::SaveLocation {
        card_restore::card_location(&self.record, std::slice::from_ref(&self.record)).unwrap()
    }

    fn app(&self, context: &egui::Context) -> App {
        let mut app = fixture(context);
        app.saves_states.inventory = Some(saves_inventory(vec![self.record.clone()]));
        let state = &mut app.saves_states.card_restore;
        state.paths = Some(self.paths.clone());
        state.paths_resolved = true;
        // The records panel is exercised by its own tests; here it must not
        // occupy the single job slot with its automatic first check.
        state.panel.records_requested = true;
        app
    }

    fn snapshot(&self) -> SaveSnapshot {
        card_restore::create_card_snapshot(&self.paths.snapshot_root, &self.location(), &Closed)
            .unwrap();
        card_restore::load_card_history(&self.paths.snapshot_root, &self.location(), &Closed)
            .snapshots[0]
            .snapshot
            .clone()
    }

    /// Mark the history as loaded for the current inventory generation.
    fn load_history(&self, app: &mut App) {
        let history =
            card_restore::load_card_history(&self.paths.snapshot_root, &self.location(), &Closed);
        let state = &mut app.saves_states.card_restore;
        state.loaded_for = Some(app.saves_states.generation);
        state.histories.insert(self.card.clone(), history);
    }
}

fn click(context: &egui::Context, app: &mut App, label: &str) -> Vec<String> {
    let first = saves_frame(context, app, [1280.0, 2400.0]);
    let point = text_bounds(&first, label)
        .first()
        .unwrap_or_else(|| panic!("{label} is visible: {:?}", text(&first)))
        .center();
    for events in [
        vec![
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
        vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    ] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 2400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    super::super::saves_states::show(app, ui, None);
                });
            },
        );
    }
    text(&first)
}

fn shown(context: &egui::Context, app: &mut App) -> Vec<String> {
    text(&saves_frame(context, app, [1280.0, 2400.0]))
}

fn has(strings: &[String], wanted: &str) -> bool {
    strings.iter().any(|value| value.contains(wanted))
}

#[test]
fn saves_page_offers_card_snapshots_without_tying_them_to_a_game() {
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    world.snapshot();
    world.load_history(&mut app);
    let strings = shown(&context, &mut app);
    for expected in [
        "Card snapshots and save folder restore",
        "Verified",
        "Restore a save folder…",
        "Create card snapshot",
    ] {
        assert!(has(&strings, expected), "missing {expected}: {strings:?}");
    }
    // Clicking only reads the snapshot: a job is queued, nothing is written.
    let before = std::fs::read(&world.card).unwrap();
    click(&context, &mut app, "Restore a save folder…");
    assert!(app.saves_states.card_restore.job.is_some());
    assert_eq!(std::fs::read(&world.card).unwrap(), before);
}

#[test]
fn choose_preview_cancel_and_undo_journey_is_explicit_and_reversible() {
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    let snapshot = world.snapshot();
    world.load_history(&mut app);
    let mut played = std::fs::read(&world.card).unwrap();
    played[(41 + 3) * 1024..(41 + 3) * 1024 + 100].fill(0x11);
    std::fs::write(&world.card, &played).unwrap();

    // Choose: folders the snapshot really holds, with no game assumption.
    let folders = card_restore::snapshot_folders(&snapshot, &world.location());
    card_restore::receive(
        &mut app,
        0,
        Outcome::Folders {
            snapshot: Box::new(snapshot.clone()),
            location: world.location(),
            result: folders.clone(),
        },
    );
    let strings = shown(&context, &mut app);
    for expected in [
        "Choose one save folder to restore",
        "BASLUS-00001TEST",
        "The whole card is never replaced",
        "does not assume which game",
        "Preview restore",
    ] {
        assert!(has(&strings, expected), "missing {expected}: {strings:?}");
    }

    // Confirm: everything affected is named before any write.
    let raw = folders.unwrap()[0].raw_name.clone();
    let preview =
        card_restore::prepare_restore(&snapshot, &world.location(), &raw, &world.paths.dirs, 1)
            .unwrap();
    let psu = preview.plan.source_psu_path.clone();
    let backup = preview.plan.backup_path.clone();
    card_restore::receive(
        &mut app,
        0,
        Outcome::Prepared {
            location: world.location(),
            result: Ok(Box::new(preview.clone())),
        },
    );
    let strings = shown(&context, &mut app);
    for expected in [
        "Preview restore",
        "Nothing has been changed yet",
        "Emulator: PCSX2",
        "Save folder: BASLUS-00001TEST",
        "icon.sys (100 bytes)",
        "Memory card to change:",
        "replace the copy of this folder",
        "Backup of the whole card",
        "Back up card and restore this folder",
    ] {
        assert!(has(&strings, expected), "missing {expected}: {strings:?}");
    }
    assert_eq!(std::fs::read(&world.card).unwrap(), played);

    // Cancel: nothing written, the exported folder is cleaned up.
    click(&context, &mut app, "Cancel");
    assert!(matches!(app.saves_states.card_restore.flow, Flow::Idle));
    assert!(!psu.exists());
    assert!(!backup.exists());
    assert_eq!(std::fs::read(&world.card).unwrap(), played);

    // Restore (worker with a fixed closure observer), then Undo from Blue.
    let preview =
        card_restore::prepare_restore(&snapshot, &world.location(), &raw, &world.paths.dirs, 2)
            .unwrap();
    let done = card_restore::apply_restore(
        &preview.plan,
        &Closed,
        &Ps2RestoreGuardOptions {
            journal_dir: world.paths.journal_dir.clone(),
            unix_seconds: 3,
        },
    )
    .unwrap();
    assert_ne!(std::fs::read(&world.card).unwrap(), played);
    card_restore::receive(
        &mut app,
        0,
        Outcome::Applied {
            location: world.location(),
            result: Ok(Box::new(done.clone())),
        },
    );
    let strings = shown(&context, &mut app);
    assert!(has(&strings, "Save folder restored"), "{strings:?}");
    assert!(has(&strings, "Undo restore"));
    // Not clicked: the fixture's live backend would run Undo with the real
    // process scan. Undo is driven with the fixed observer instead and its
    // outcome delivered as the worker would.
    let result = card_restore::undo_restore(&done.journal_path, &Closed, 4);
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(std::fs::read(&world.card).unwrap(), played);
    card_restore::receive(
        &mut app,
        0,
        Outcome::Undone {
            restore: Box::new(done),
            location: world.location(),
            result,
        },
    );
    assert!(has(&shown(&context, &mut app), "restore was undone"));
}

#[test]
fn refused_steps_and_unsupported_records_are_explained() {
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    world.load_history(&mut app);
    card_restore::receive(
        &mut app,
        0,
        Outcome::Applied {
            location: world.location(),
            result: Err(
                "The memory card changed since preview; inspect it again before restoring.".into(),
            ),
        },
    );
    let strings = shown(&context, &mut app);
    assert!(has(&strings, "changed since preview"), "{strings:?}");
    assert!(matches!(app.saves_states.card_restore.flow, Flow::Idle));

    // The same card listed twice is ambiguous; savestates are explained.
    let mut savestate = state_record(PersistentStateType::SaveState, StateEmulator::Pcsx2);
    savestate.path = world.card.with_file_name("SLUS-20001.p2s");
    app.saves_states.inventory = Some(saves_inventory(vec![
        world.record.clone(),
        world.record.clone(),
        savestate,
    ]));
    let strings = shown(&context, &mut app);
    assert!(has(&strings, "listed more than once"), "{strings:?}");
    assert!(
        has(&strings, "Savestates cannot be restored"),
        "{strings:?}"
    );
    assert!(!has(&strings, "Restore a save folder…"));

    // Game pages say why Restore is disabled, per save type.
    assert_eq!(
        restore_unavailable(PersistentStateType::SaveState),
        SAVESTATE_RESTORE_UNSUPPORTED
    );
    assert_eq!(
        restore_unavailable(PersistentStateType::MemoryCard),
        CARD_RESTORE_WHERE
    );
}

#[test]
fn a_failed_step_drops_a_stale_preview_and_keeps_undo_reachable() {
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    let snapshot = world.snapshot();
    world.load_history(&mut app);
    let raw = card_restore::snapshot_folders(&snapshot, &world.location()).unwrap()[0]
        .raw_name
        .clone();
    let preview =
        card_restore::prepare_restore(&snapshot, &world.location(), &raw, &world.paths.dirs, 1)
            .unwrap();
    app.saves_states.card_restore.flow = Flow::Confirm {
        preview: Box::new(preview),
        location: world.location(),
    };
    app.saves_states.card_restore.job = Some(7);
    card_restore::job_failed(&mut app, "worker stopped");
    assert!(matches!(app.saves_states.card_restore.flow, Flow::Idle));
    assert!(app.saves_states.card_restore.job.is_none());
    let strings = shown(&context, &mut app);
    assert!(has(&strings, "did not finish"), "{strings:?}");
    assert!(!has(&strings, "Back up card and restore this folder"));
}

#[test]
fn card_history_loads_even_when_the_first_view_filters_cards_out() {
    // Codex P2: history is loaded once per inventory, so it must include
    // cards hidden by the search or type filter at that moment.
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    world.snapshot();
    app.saves_states.filter = super::super::saves_states::Filter::SaveStates;
    let strings = shown(&context, &mut app);
    assert!(!has(&strings, "Card snapshots and save folder restore"));
    assert!(
        app.saves_states.card_restore.job.is_some(),
        "the hidden card's history is still requested"
    );

    // Delivered as the worker would: the card's snapshot is listed once
    // the filter is cleared, without another load.
    let job = app.saves_states.card_restore.job.unwrap();
    let mut histories = std::collections::BTreeMap::new();
    histories.insert(
        world.card.clone(),
        card_restore::load_card_history(&world.paths.snapshot_root, &world.location(), &Closed),
    );
    let generation = app.saves_states.generation;
    card_restore::receive(
        &mut app,
        job,
        Outcome::History {
            histories,
            generation,
        },
    );
    app.saves_states.filter = super::super::saves_states::Filter::All;
    let strings = shown(&context, &mut app);
    assert!(has(&strings, "Restore a save folder…"), "{strings:?}");
}

#[test]
fn saves_page_shows_restore_records_review_and_storage_panels() {
    use super::super::card_restore::recovery;
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    world.load_history(&mut app);
    let garbage = world.paths.journal_dir.join("ps2-psu-restore-garbage.json");
    std::fs::write(&garbage, b"not a journal").unwrap();
    app.saves_states.card_restore.panel.records =
        Some(recovery::load_records(&world.paths.journal_dir));
    let strings = shown(&context, &mut app);
    for expected in [
        "Memory card restore records · 1 need attention",
        "Unreadable",
        "Restores to every memory card stay paused",
        "Show evidence…",
        "Refresh records",
        "Restore storage",
    ] {
        assert!(has(&strings, expected), "missing {expected}: {strings:?}");
    }
    app.saves_states.card_restore.panel.review = Some(recovery::review_preview(&garbage).unwrap());
    let strings = shown(&context, &mut app);
    for expected in [
        "Review restore record",
        "Evidence to inspect",
        "cannot tell which memory card",
        "This cannot be marked reviewed",
    ] {
        assert!(has(&strings, expected), "missing {expected}: {strings:?}");
    }
    assert!(
        !has(&strings, "Mark as reviewed"),
        "no acknowledgement for unreadable records"
    );
    assert_eq!(std::fs::read(&garbage).unwrap(), b"not a journal");
}

// ---- Night Watch II regressions (uncommitted; FAIL on 571d4afb while the defects exist) ----

/// NW2-B-03: a failed PS2 card restore must not be recorded as a success.
#[test]
fn nw2_b03_failed_card_restore_is_recorded_as_failed_in_activity() {
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    let id = app.activity.queue_with(
        "Restoring a memory-card folder",
        Route::Section(Section::Activity),
        activity::CancelPolicy::SafeNow,
    );
    app.activity.start(id);
    app.saves_states.card_restore.job = Some(id);
    let (sender, receiver) = std::sync::mpsc::channel();
    app.backend.rx = receiver;
    sender
        .send(super::super::backend::Event::Finished {
            id,
            outcome: Ok(super::super::backend::Payload::CardRestore(Box::new(
                Outcome::Applied {
                    location: world.location(),
                    result: Err("rollback failed: the card could not be reversed safely".into()),
                },
            ))),
        })
        .unwrap();
    app.poll(&context);
    assert!(!app.activity.jobs[&id].active(), "job settled");
    assert_eq!(
        app.activity.failed(),
        1,
        "a failed card restore was recorded as a success in Activity"
    );
}

/// NW2-B-03: a refused Undo from the restore-records panel ("Undo was not
/// done. …") must reach Activity as a failure, not as "Finished".
#[test]
fn nw2_b03_refused_panel_undo_is_recorded_as_failed_in_activity() {
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    let id = app.activity.queue_with(
        "Undoing a memory-card restore",
        Route::Section(Section::Activity),
        activity::CancelPolicy::SafeNow,
    );
    app.activity.start(id);
    app.saves_states.card_restore.job = Some(id);
    let (sender, receiver) = std::sync::mpsc::channel();
    app.backend.rx = receiver;
    sender
        .send(super::super::backend::Event::Finished {
            id,
            outcome: Ok(super::super::backend::Payload::CardRestore(Box::new(
                Outcome::Panel(card_restore::panel::PanelOutcome::Records {
                    report: Default::default(),
                    messages: vec!["Undo was not done. PCSX2 is running.".into()],
                    failed: true,
                }),
            ))),
        })
        .unwrap();
    app.poll(&context);
    let job = &app.activity.jobs[&id];
    assert!(!job.active(), "job settled");
    assert_eq!(job.phase, activity::Phase::Failed, "{}", job.summary);
    assert_eq!(job.summary, "Undo was not done. PCSX2 is running.");
    assert_eq!(app.saves_states.card_restore.job, None);
    // The panel still shows the per-record message inline.
    assert_eq!(
        app.saves_states.card_restore.panel.messages,
        vec!["Undo was not done. PCSX2 is running.".to_string()]
    );
}

/// A successful records re-read stays "Finished".
#[test]
fn panel_records_reread_is_recorded_as_finished() {
    let context = egui::Context::default();
    let world = World::new();
    let mut app = world.app(&context);
    let id = app.activity.queue_with(
        "Checking restore records",
        Route::Section(Section::Activity),
        activity::CancelPolicy::SafeNow,
    );
    app.activity.start(id);
    app.saves_states.card_restore.job = Some(id);
    let (sender, receiver) = std::sync::mpsc::channel();
    app.backend.rx = receiver;
    sender
        .send(super::super::backend::Event::Finished {
            id,
            outcome: Ok(super::super::backend::Payload::CardRestore(Box::new(
                Outcome::Panel(card_restore::panel::PanelOutcome::Records {
                    report: Default::default(),
                    messages: Vec::new(),
                    failed: false,
                }),
            ))),
        })
        .unwrap();
    app.poll(&context);
    assert_eq!(app.activity.jobs[&id].phase, activity::Phase::Complete);
    assert_eq!(app.activity.failed(), 0);
}
