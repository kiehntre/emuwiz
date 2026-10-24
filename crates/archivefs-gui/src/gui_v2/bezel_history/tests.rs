use super::*;
use crate::cheats_mods::duckstation_environment::test_environment::{self, TestMachine};
use archivefs_core::{bezel_apply::*, bezel_decorations::*, patch_manager::*};
use std::{
    fs,
    path::PathBuf,
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
    let request = BezelApplyRequest {
        source_asset: DecorationAsset {
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
        },
        source_image: LocalBezelImageInfo {
            width: 100,
            height: 80,
            format: "png".into(),
        },
        resolved_identity: "game-id".into(),
        platform: "SNES".into(),
        emulator: "retroarch".into(),
        core: Some("snes9x".into()),
        config_path: Some(destination.join("retroarch.cfg")),
        overlay_root: Some(destination.join("overlays")),
        approved_roots: vec![root.path().to_path_buf()],
    };
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
                show(ui, app, game);
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
    assert!(!in_scope(&f.journal, Some(Path::new("/a/game.sfc"))));
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
}

#[test]
fn classification_uses_record_contract_not_title_or_path() {
    let f = fixture();
    let mut ordinary = f.journal.clone();
    ordinary.context.source_mode = "ordinary_cheats".into();
    ordinary.context.verified_game_identity = "retroarch_bezel".into();
    assert!(!supported(&ordinary));
    assert!(!in_scope(&ordinary, None));
    assert!(supported(&f.journal));
}
