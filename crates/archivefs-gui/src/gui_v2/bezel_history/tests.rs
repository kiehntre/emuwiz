use super::*;
use crate::cheats_mods::duckstation_environment::test_environment::{self, TestMachine};
use archivefs_core::{bezel_apply::*, bezel_decorations::*, patch_manager::*};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct Fixture {
    _root: tempfile::TempDir,
    destination: PathBuf,
    history: PathBuf,
    backups: PathBuf,
    journal_path: PathBuf,
    journal: SharedApplyJournal,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("retroarch");
    fs::create_dir(&destination).unwrap();
    let game = root.path().join("Game.sfc");
    fs::write(&game, b"fixture game").unwrap();
    let source = root.path().join("bezel.png");
    // This is a transaction-contract fixture; real PNG verification is covered
    // by PR #114. No host emulator is discovered or launched.
    fs::write(&source, b"synthetic-image-bytes").unwrap();
    fs::write(
        destination.join("retroarch.cfg"),
        b"video_driver = \"gl\"\n",
    )
    .unwrap();
    let history = root.path().join("history");
    let backups = root.path().join("backups");
    let source_asset = DecorationAsset {
        id: "test-bezel".into(),
        scope: DecorationScope::Game,
        source: DecorationSource::LocalPack {
            path: source.display().to_string(),
        },
        evidence: DecorationEvidence::VerifiedIdentity {
            identity: "game-id".into(),
        },
        targets: vec![],
        readiness: DecorationReadiness::Ready,
        provenance: DecorationProvenance {
            provider: "test".into(),
            reference: "fixture".into(),
            retrieved_at_unix_seconds: None,
        },
        viewport: Some(ViewportMetadata {
            left: 0,
            top: 0,
            width: 100,
            height: 80,
        }),
    };
    // Exercise the same serialized request against current main and PR #114's
    // additive launch identity. Main ignores that optional future field; #114
    // consumes it to name this exact content/core override.
    let request: BezelApplyRequest = serde_json::from_value(serde_json::json!({
        "source_asset": source_asset,
        "source_image": { "width": 100, "height": 80, "format": "png" },
        "resolved_identity": "game-id", "platform": "SNES", "emulator": "retroarch",
        "core": "snes9x", "config_path": destination.join("retroarch.cfg"),
        "overlay_root": destination.join("overlays"), "approved_roots": [root.path()],
        "retroarch_launch": { "content_path": game, "core_library_name": "snes9x", "home": root.path() }
    })).unwrap();
    let plan = prepare_retroarch_bezel_plan(&request).unwrap();
    let applied = apply_retroarch_bezel_plan(
        &plan,
        &RetroArchBezelApplyOptions {
            general_approved: true,
            replacement_approved: false,
            operation_id: "blue-bezel".into(),
            timestamp_unix_seconds: 42,
            history_root: history.clone(),
            backup_root: backups.clone(),
        },
    )
    .unwrap();
    assert_eq!(applied.journal.status, SharedApplyStatus::Success);
    Fixture {
        _root: root,
        destination,
        history,
        backups,
        journal_path: applied.journal_path.unwrap(),
        journal: applied.journal,
    }
}

fn app(f: &Fixture) -> (ArchiveFsApp, test_environment::Guard) {
    let guard = test_environment::install(TestMachine {
        duckstation: None,
        history_root: f.history.clone(),
        backup_root: f.backups.clone(),
    });
    let mut app = crate::tests::app_for_operation_tests();
    app.ui_mode = crate::GuiMode::AdvancedView;
    app.view = MainView::HistoryLogs;
    reload(&mut app, f);
    (app, guard)
}

fn reload(app: &mut ArchiveFsApp, f: &Fixture) {
    app.shared_history = SharedHistoryState::Ready(crate::LoadedHistory::group(
        discover_shared_apply_history(&f.history),
        vec![],
        &f.backups,
    ));
}

fn settle(app: &mut ArchiveFsApp) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while matches!(
        app.shared_rollback,
        SharedRollbackState::Previewing { .. } | SharedRollbackState::Applying { .. }
    ) {
        app.poll_shared_rollback();
        assert!(Instant::now() < deadline, "Undo worker timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => {
            for s in v {
                text(s, out);
            }
        }
        _ => {}
    }
}

fn render(app: &mut ArchiveFsApp, game: Option<&Path>) -> String {
    let context = egui::Context::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 1800.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show(ui, app, game.is_none());
            });
        },
    );
    let mut out = String::new();
    for shape in output.shapes {
        text(&shape.shape, &mut out);
    }
    out
}

fn preview(app: &mut ArchiveFsApp, f: &Fixture) {
    dispatch(
        app,
        &egui::Context::default(),
        Some(HistoryPageAction::PreviewRollback {
            journal_path: f.journal_path.clone(),
            destination_root: f.destination.clone(),
        }),
    );
    settle(app);
    assert!(
        matches!(&app.shared_rollback, SharedRollbackState::Review { preview, .. } if preview.available)
    );
}

#[test]
fn durable_bezel_record_reuses_gold_preview_confirmation_and_executor_after_restart() {
    let f = fixture();
    let (mut app, _guard) = app(&f);
    let before = fs::read(f.destination.join("retroarch.cfg")).unwrap();
    let source = f._root.path().join("bezel.png");
    let source_before = fs::read(&source).unwrap();
    let decoy = f._root.path().join("flatpak-retroarch.cfg");
    fs::write(&decoy, b"unrelated second installation").unwrap();
    let shown = render(&mut app, None);
    assert!(shown.contains("Bezel applied from"), "{shown}");
    assert!(shown.contains("Preview bezel Undo"));
    assert!(!shown.contains("Cheats added"));
    preview(&mut app, &f);
    assert!(
        f.destination.join("overlays/bezel.png").exists(),
        "preview must never mutate"
    );
    assert!(render(&mut app, None).contains("Confirm exact rollback"));
    dispatch(
        &mut app,
        &egui::Context::default(),
        Some(HistoryPageAction::ConfirmRollback),
    );
    settle(&mut app);
    assert!(
        matches!(&app.shared_rollback, SharedRollbackState::Result(result) if result.status == SharedApplyStatus::Success)
    );
    // Blue's Cheat activity lists CheatPreview entries; bezel Undo is not one.
    let actions: Vec<_> = app.history.entries().map(|entry| entry.action).collect();
    assert!(
        actions.contains(&crate::ActivityAction::BezelUndo),
        "{actions:?}"
    );
    assert!(
        !actions.contains(&crate::ActivityAction::CheatPreview),
        "{actions:?}"
    );
    assert_eq!(
        fs::read(f.destination.join("retroarch.cfg")).unwrap(),
        before
    );
    for entry in &f.journal.entries {
        let relative = entry
            .plan_entry
            .destination_relative_path
            .to_path_buf()
            .unwrap();
        assert!(!f.destination.join(relative).exists());
    }
    assert_eq!(fs::read(&source).unwrap(), source_before);
    assert_eq!(fs::read(&decoy).unwrap(), b"unrelated second installation");
    reload(&mut app, &f);
    // The saved apply journal is immutable. The existing preview checks the
    // separate rollback marker, including after an application restart.
    app.shared_rollback = SharedRollbackState::Idle;
    dispatch(
        &mut app,
        &egui::Context::default(),
        Some(HistoryPageAction::PreviewRollback {
            journal_path: f.journal_path.clone(),
            destination_root: f.destination.clone(),
        }),
    );
    settle(&mut app);
    assert!(
        matches!(&app.shared_rollback, SharedRollbackState::Review { preview, .. } if !preview.available)
    );
    let shown = render(&mut app, None);
    assert!(shown.contains("Already undone"), "{shown}");
    assert!(shown.contains("Rollback blocked"));
}

#[test]
fn edit_after_preview_refuses_undo_and_preserves_changed_and_unrelated_files() {
    let f = fixture();
    let (mut app, _guard) = app(&f);
    preview(&mut app, &f);
    let image = f.destination.join("overlays/bezel.png");
    fs::write(&image, b"user edited this after preview").unwrap();
    dispatch(
        &mut app,
        &egui::Context::default(),
        Some(HistoryPageAction::ConfirmRollback),
    );
    settle(&mut app);
    assert!(
        matches!(&app.shared_rollback, SharedRollbackState::Result(result) if result.status == SharedApplyStatus::Failed)
    );
    assert_eq!(fs::read(&image).unwrap(), b"user edited this after preview");
    assert_eq!(
        fs::read(f.destination.join("retroarch.cfg")).unwrap(),
        b"video_driver = \"gl\"\n"
    );
}

#[test]
fn unsupported_contracts_and_incomplete_operations_are_read_only() {
    let f = fixture();
    let (mut app, _guard) = app(&f);
    for modification in 0..6 {
        let mut journal = f.journal.clone();
        match modification {
            0 => journal.context.adapter = PreviewAdapter::Dolphin,
            1 => journal.entries[0].plan_entry.content_verification = None,
            2 => journal.status = SharedApplyStatus::PartialFailure,
            3 => journal.status = SharedApplyStatus::DryRun,
            4 => journal.schema_version = u32::MAX,
            _ => journal.entries.clear(),
        }
        assert!(!preview_allowed(&journal));
        app.shared_history = SharedHistoryState::Ready(crate::LoadedHistory::group(
            SharedHistoryReport {
                journals: vec![(SharedTransactionPath::from_path(&f.journal_path), journal)],
                warnings: vec![],
                complete: true,
            },
            vec![],
            &f.backups,
        ));
        let shown = render(&mut app, None);
        assert!(shown.contains("Read-only:"), "{shown}");
        assert!(matches!(app.shared_rollback, SharedRollbackState::Idle));
    }
}

#[test]
fn scope_change_and_navigation_do_not_reuse_a_reviewed_undo() {
    let f = fixture();
    let (mut app, _guard) = app(&f);
    assert!(!in_scope(&f.journal, false));
    preview(&mut app, &f);
    let shown = render(&mut app, Some(Path::new("/another/game.sfc")));
    assert!(!shown.contains("Confirm exact rollback"));
    assert!(matches!(app.shared_rollback, SharedRollbackState::Idle));
    assert!(!shown.contains("Bezel applied from"));
    preview(&mut app, &f);
    prepare(&mut app, true);
    prepare(&mut app, false);
    // Gold's global poll cancels unconfirmed previews after leaving History.
    crate::app_polling::poll_and_reconcile(&mut app, &egui::Context::default());
    assert!(matches!(app.shared_rollback, SharedRollbackState::Idle));
    assert!(f.destination.join("overlays/bezel.png").exists());
    let (sender, receiver) = std::sync::mpsc::channel();
    app.shared_rollback = SharedRollbackState::Previewing { receiver };
    render(&mut app, Some(Path::new("/missing-game")));
    assert!(matches!(app.shared_rollback, SharedRollbackState::Idle));
    assert!(sender.send(Err("late preview result".into())).is_err());
}

#[test]
fn classification_uses_record_contract_not_title_or_path() {
    let f = fixture();
    let mut ordinary = f.journal.clone();
    ordinary.context.source_mode = "ordinary_cheats".into();
    ordinary.context.verified_game_identity = "retroarch_bezel".into();
    assert!(!supported(&ordinary));
    assert!(!in_scope(&ordinary, true));
    assert!(supported(&f.journal));
}

#[test]
fn blue_history_route_reloads_bezels_and_missing_game_scope_discards_confirmation() {
    use crate::gui_v2::routes::{Route, Section};
    let f = fixture();
    let (_gold, _guard) = app(&f);
    let context = egui::Context::default();
    let mut blue = crate::gui_v2::tests::fixture(&context);
    blue.router.current = Route::Section(Section::History);
    let frame = |blue: &mut crate::gui_v2::App, events| {
        if let Some(native) = blue.native_workflows.as_mut() {
            native.prepare_blue_history(blue.router.current.section() == Section::History);
            native.poll(&context, &mut blue.activity);
        }
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 2400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| blue.history(ui));
            },
        );
        fn button(shape: &egui::Shape) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == "Preview bezel Undo" => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                egui::Shape::Vec(v) => v.iter().find_map(button),
                _ => None,
            }
        }
        let mut out = String::new();
        let mut preview = None;
        for shape in &output.shapes {
            text(&shape.shape, &mut out);
            preview = preview.or_else(|| button(&shape.shape));
        }
        (out, preview)
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut previous_position: Option<egui::Pos2> = None;
    let mut stable_frames = 0;
    let position = loop {
        let (shown, position) = frame(&mut blue, vec![]);
        if let Some(position) = position {
            assert!(shown.contains("Bezel applied from bezel"), "{shown}");
            assert!(!shown.contains("No history yet"), "{shown}");
            assert!(
                !shown.contains("Cheat activity"),
                "loading history is not a cheat operation: {shown}"
            );
            stable_frames = if previous_position == Some(position) {
                stable_frames + 1
            } else {
                0
            };
            previous_position = Some(position);
            if stable_frames >= 3 {
                break position;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Blue did not reload its bezel records: {shown}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    for pressed in [true, false] {
        frame(
            &mut blue,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (shown, _) = frame(&mut blue, vec![]);
        if shown.contains("Confirm exact rollback") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Blue did not open Gold's confirmation: {shown}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    blue.router.current = Route::Task {
        section: Section::History,
        game: i64::MAX,
    };
    let (shown, _) = frame(&mut blue, vec![]);
    assert!(!shown.contains("Confirm exact rollback"), "{shown}");
    assert!(shown.contains("Open all History"), "{shown}");
    blue.router.current = Route::Section(Section::History);
    let (shown, _) = frame(&mut blue, vec![]);
    assert!(
        !shown.contains("Confirm exact rollback"),
        "a new Preview is required: {shown}"
    );
    assert!(shown.contains("Preview bezel Undo"));
    assert!(f.destination.join("overlays/bezel.png").exists());
}

#[test]
fn returning_to_history_reloads_a_previously_empty_gold_source() {
    let f = fixture();
    let (mut app, _guard) = app(&f);
    app.shared_history = SharedHistoryState::Ready(crate::LoadedHistory::group(
        SharedHistoryReport {
            journals: vec![],
            warnings: vec![],
            complete: true,
        },
        vec![],
        &f.backups,
    ));
    prepare(&mut app, false);
    let deadline = Instant::now() + Duration::from_secs(10);
    let ctx = egui::Context::default();
    loop {
        prepare_source(&mut app, &ctx, true);
        if matches!(&app.shared_history, SharedHistoryState::Ready(loaded) if !loaded.report.journals.is_empty())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "returning to History kept a stale empty source"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(f.destination.join("overlays/bezel.png").exists());
}

#[test]
fn undo_activity_label_follows_the_journal_and_is_neutral_when_unknown() {
    let f = fixture();
    let (mut app, _guard) = app(&f);
    assert_eq!(
        app.shared_rollback_activity(&f.journal_path),
        crate::ActivityAction::BezelUndo
    );
    // A cheat journal keeps the existing cheat label.
    let mut report = discover_shared_apply_history(&f.history);
    for (_, journal) in &mut report.journals {
        journal.context.source_mode = "retroarch_cheat".into();
    }
    app.shared_history =
        SharedHistoryState::Ready(crate::LoadedHistory::group(report, vec![], &f.backups));
    assert_eq!(
        app.shared_rollback_activity(&f.journal_path),
        crate::ActivityAction::CheatPreview
    );
    // A record missing from the loaded list is not guessed.
    assert_eq!(
        app.shared_rollback_activity(&f.history.join("other.json")),
        crate::ActivityAction::Undo
    );
    // History reloading: Preview still runs through Gold, labelled neutrally.
    app.shared_history = SharedHistoryState::NotLoaded;
    preview(&mut app, &f);
    let actions: Vec<_> = app.history.entries().map(|entry| entry.action).collect();
    assert!(
        actions.contains(&crate::ActivityAction::Undo),
        "{actions:?}"
    );
    assert!(
        !actions.contains(&crate::ActivityAction::CheatPreview)
            && !actions.contains(&crate::ActivityAction::BezelUndo),
        "{actions:?}"
    );
    assert!(f.destination.join("overlays/bezel.png").exists());
}

#[test]
fn blue_history_lists_gold_undo_activity_with_its_classification_and_refreshes() {
    use crate::activity_history::{ActivityOutcome, HistoryEntry};
    use crate::gui_v2::routes::{Route, Section};
    let f = fixture();
    let (_gold, _guard) = app(&f);
    let context = egui::Context::default();
    let mut blue = crate::gui_v2::tests::fixture(&context);
    blue.router.current = Route::Section(Section::History);
    let render = |blue: &mut crate::gui_v2::App| {
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 8000.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| blue.history(ui));
            },
        );
        let mut out = String::new();
        for shape in &output.shapes {
            text(&shape.shape, &mut out);
        }
        out
    };
    render(&mut blue);
    let gold = &mut blue.native_workflows.as_mut().unwrap().app;
    for (action, message) in [
        (crate::ActivityAction::CheatPreview, "cheat record"),
        (crate::ActivityAction::BezelUndo, "bezel record"),
        (crate::ActivityAction::Undo, "unknown record"),
    ] {
        gold.history.record(HistoryEntry::new(
            action,
            None,
            ActivityOutcome::Completed,
            message,
        ));
    }
    let shown = render(&mut blue);
    let section = |heading: &str| {
        let start = shown
            .find(&format!("{heading}\n"))
            .unwrap_or_else(|| panic!("{heading} missing: {shown}"));
        shown[start..].split("Bezel history & Undo").next().unwrap()
    };
    let cheats = section("Cheat activity");
    let cheats = cheats.split("Undo activity\n").next().unwrap();
    assert!(cheats.contains("Cheats & Mods preview") && cheats.contains("cheat record"));
    assert!(!cheats.contains("bezel record") && !cheats.contains("unknown record"));
    let undo = section("Undo activity");
    assert!(
        undo.contains("Bezel Undo\nFinished\nbezel record"),
        "{shown}"
    );
    assert!(
        undo.contains("Undo activity\nFinished\nunknown record"),
        "{shown}"
    );
    assert!(!undo.contains("cheat record") && !undo.contains("Cheats & Mods"));
    // A real Gold preview while History is reloading appears on the next
    // frame, neutrally labelled, alongside the bezel card.
    let gold = &mut blue.native_workflows.as_mut().unwrap().app;
    gold.shared_history = SharedHistoryState::NotLoaded;
    preview(gold, &f);
    let shown = render(&mut blue);
    let (_, undo) = shown.split_once("Undo activity\n").unwrap();
    assert!(
        undo.contains("Rollback preview completed and is available."),
        "{shown}"
    );
    assert!(shown.contains("Bezel history & Undo"), "{shown}");
    assert!(f.destination.join("overlays/bezel.png").exists());
}

/// Job #22 (exFAT): a partly failed Apply whose automatic rollback finished
/// is shown as refused with nothing to undo, not "needs attention"; one
/// without the rollback marker still asks for review. Neither offers Undo,
/// and the kept backup is visible on both.
#[test]
fn refused_and_rolled_back_apply_is_not_shown_as_needing_attention() {
    let f = fixture();
    let mut journal = f.journal.clone();
    journal.status = SharedApplyStatus::PartialFailure;
    journal.entries[0].backup_path = Some(SharedTransactionPath::from_path(
        &f.backups.join("refused/override.bak"),
    ));
    for (id, rolled_back) in [("refused-op", true), ("unresolved-op", false)] {
        journal.operation_id = id.into();
        journal.timestamp_unix_seconds += 1;
        fs::write(
            f.history.join(format!("{id}.json")),
            serde_json::to_vec_pretty(&journal).unwrap(),
        )
        .unwrap();
        if rolled_back {
            fs::write(f.history.join(format!("{id}.rollback.json")), b"{}").unwrap();
        }
    }
    let (mut app, _guard) = app(&f);
    let shown = render(&mut app, None);
    assert!(shown.contains("Bezel change refused from"), "{shown}");
    assert!(
        shown.contains("Refused and undone automatically: no changes remain"),
        "{shown}"
    );
    assert_eq!(
        shown.matches("Bezel changes need attention from").count(),
        1,
        "only the unresolved record asks for review: {shown}"
    );
    assert_eq!(
        shown.matches("Backup kept: ").count(),
        2,
        "the kept backup is shown on both records: {shown}"
    );
    // The successful fixture apply is the only record that offers Undo.
    assert_eq!(shown.matches("Bezel applied from").count(), 1, "{shown}");
    // Once undone (marker written by the executor), it no longer offers Undo
    // after a restart either.
    fs::write(f.journal_path.with_extension("rollback.json"), b"{}").unwrap();
    reload(&mut app, &f);
    let shown = render(&mut app, None);
    assert!(shown.contains("Bezel changes undone from"), "{shown}");
    assert!(!shown.contains("Bezel applied from"), "{shown}");
    assert_eq!(
        shown
            .matches("Already undone. This record is read-only.")
            .count(),
        1,
        "{shown}"
    );
}
