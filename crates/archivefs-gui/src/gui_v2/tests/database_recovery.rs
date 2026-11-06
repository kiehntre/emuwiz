//! Blue's quick History shows library-database restore receipts and offers
//! the same rollback and reconciliation as the full History window, through
//! the same core executors, on a worker thread. Disposable databases only.
use super::super::database_recovery::State;
use super::super::routes::{Route, Section};
use super::*;
use archivefs_core::operation::OperationState;
use archivefs_core::{
    Database, DatabaseRestoreState, database_restore_fault_injection as fault,
    prepare_database_restore, read_database_restore_receipt, restore_database,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CONFIRMATION: &str = "RESTORE DATABASE";

struct Estate {
    _dir: tempfile::TempDir,
    live: PathBuf,
    backup: PathBuf,
}

fn estate() -> Estate {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("library.sqlite3");
    let backup = dir.path().join("selected.backup");
    let mut database = Database::open_or_create(&live).unwrap();
    database.add_platform_alias("live-only", "snes").unwrap();
    database.close().unwrap();
    Database::open_or_create(&backup).unwrap().close().unwrap();
    Estate {
        _dir: dir,
        live,
        backup,
    }
}

fn aliases(path: &Path) -> Vec<String> {
    Database::open_read_only(path)
        .unwrap()
        .list_platform_aliases()
        .unwrap()
        .into_iter()
        .map(|alias| alias.alias)
        .collect()
}

fn pinned(live: &Path) -> State {
    State::pinned(live.to_path_buf())
}

/// Renders the section once and returns the frame's visible text.
fn render(context: &egui::Context, state: &mut State, busy: bool) -> (Vec<String>, bool) {
    let mut reload = false;
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 1200.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                reload |= state.show(ui, busy);
            });
        },
    );
    (text(&output), reload)
}

/// Renders until the worker has reported (bounded), returning the last frame.
fn settle(context: &egui::Context, state: &mut State, busy: bool) -> (Vec<String>, bool) {
    let started = Instant::now();
    let mut reload = false;
    loop {
        let (_, changed) = render(context, state, busy);
        reload |= changed;
        state.poll();
        if !state.working() && started.elapsed() > Duration::from_millis(50) {
            // One more frame so the text reflects the worker's result.
            let (strings, changed) = render(context, state, busy);
            reload |= changed;
            if !state.working() {
                return (strings, reload);
            }
        }
        if started.elapsed() > Duration::from_secs(30) {
            panic!("the recovery worker never finished");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn completed_restore(estate: &Estate) -> archivefs_core::DatabaseRestoreReceipt {
    let plan = prepare_database_restore(&estate.live, &estate.backup).unwrap();
    restore_database(&plan, CONFIRMATION).unwrap().receipt
}

#[test]
fn completed_restore_is_listed_and_rolls_back_after_the_same_confirmation() {
    let estate = estate();
    let receipt = completed_restore(&estate);
    assert!(
        aliases(&estate.live).is_empty(),
        "the restore replaced the live database"
    );
    let context = egui::Context::default();
    let mut state = pinned(&estate.live);
    let (strings, _) = settle(&context, &mut state, false);
    assert!(
        strings.iter().any(|s| s == "Database restore & recovery"),
        "{strings:?}"
    );
    assert!(
        strings.iter().any(|s| s == "Database restore completed"),
        "{strings:?}"
    );
    assert!(
        strings.iter().any(|s| s == "Review rollback"),
        "{strings:?}"
    );
    let row = state
        .rows()
        .iter()
        .find(|r| r.operation_id == receipt.operation_id)
        .cloned()
        .expect("row");
    assert_eq!(row.state, OperationState::Completed);
    assert!(row.rollback_offered);
    assert!(!row.reconcile_offered);

    // Review runs core's read-only checks on a worker; the button stays
    // disabled until the exact phrase is typed.
    state.request_rollback_review(context.clone(), row.receipt_path.clone(), false);
    let (strings, _) = settle(&context, &mut state, false);
    assert!(
        strings.iter().any(|s| s == "Review rollback"),
        "{strings:?}"
    );
    assert!(
        strings.iter().any(|s| s == "Roll back restore"),
        "{strings:?}"
    );
    assert!(state.rollback_review().is_some());
    assert!(!state.rollback_enabled(false));
    state.set_confirmation("restore database");
    assert!(!state.rollback_enabled(false));
    state.set_confirmation(CONFIRMATION);
    assert!(state.rollback_enabled(false));
    assert!(
        !state.rollback_enabled(true),
        "a busy catalogue disables the rollback"
    );

    state.request_rollback(context.clone(), false);
    let (strings, reload) = settle(&context, &mut state, false);
    assert!(
        reload,
        "a rollback replaces the live file, so the library must reload"
    );
    assert!(
        state.message().contains("rollback completed"),
        "{}",
        state.message()
    );
    assert_eq!(aliases(&estate.live), vec!["live-only".to_string()]);
    assert!(
        strings.iter().any(|s| s == "Database restore rolled back"),
        "{strings:?}"
    );
    assert!(state.rollback_review().is_none());
    let on_disk = read_database_restore_receipt(&row.receipt_path).unwrap();
    assert_eq!(on_disk.state, DatabaseRestoreState::RolledBack);
}

#[test]
fn interrupted_restore_is_flagged_and_reconciles_through_core() {
    let estate = estate();
    let plan = prepare_database_restore(&estate.live, &estate.backup).unwrap();
    let crashed = {
        let _guard = fault::install(|stage| {
            if stage == fault::STAGE_AFTER_EXCHANGE {
                panic!("simulated crash after the exchange")
            }
            Ok(())
        });
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restore_database(&plan, CONFIRMATION)
        }))
        .is_err()
    };
    assert!(crashed);
    let context = egui::Context::default();
    let mut state = pinned(&estate.live);
    let (strings, _) = settle(&context, &mut state, false);
    assert!(
        strings
            .iter()
            .any(|s| s == "Database restore interrupted: reconciliation required"),
        "{strings:?}"
    );
    assert!(
        strings.iter().any(|s| s == "Reconcile interrupted restore"),
        "{strings:?}"
    );
    assert!(state.needs_attention());
    let row = state.rows()[0].clone();
    assert!(row.interrupted());
    assert!(row.reconcile_offered);
    assert!(
        !row.rollback_offered,
        "no rollback until the receipt is truthful"
    );
    assert!(
        row.summary.contains("after the replacement"),
        "{}",
        row.summary
    );

    state.request_reconcile(context.clone(), row.receipt_path.clone(), false);
    let (strings, reload) = settle(&context, &mut state, false);
    assert!(
        reload,
        "the replacement was found in place; the library in memory may predate it"
    );
    assert!(
        state.message().starts_with("Reconciled:"),
        "{}",
        state.message()
    );
    assert!(
        strings.iter().any(|s| s == "Database restore completed"),
        "{strings:?}"
    );
    assert!(!state.needs_attention());
    let row = state.rows()[0].clone();
    assert_eq!(row.state, OperationState::Completed);
    assert!(
        row.rollback_offered,
        "a reconciled restore can be rolled back"
    );
    let on_disk = read_database_restore_receipt(&row.receipt_path).unwrap();
    assert_eq!(on_disk.state, DatabaseRestoreState::Completed);
    assert_eq!(on_disk.original_intact, Some(false));
}

#[test]
fn busy_catalogue_and_stale_review_are_refused_without_changing_anything() {
    let estate = estate();
    let receipt = completed_restore(&estate);
    let context = egui::Context::default();
    let mut state = pinned(&estate.live);
    let (strings, _) = settle(&context, &mut state, true);
    assert!(
        strings
            .iter()
            .any(|s| s.contains("wait until the current load, scan or background job")),
        "{strings:?}"
    );
    let path = state.rows()[0].receipt_path.clone();
    state.request_rollback_review(context.clone(), path.clone(), true);
    assert!(state.message().contains("refused"), "{}", state.message());
    assert!(state.rollback_review().is_none());
    state.request_reconcile(context.clone(), path.clone(), true);
    assert!(state.message().contains("refused"), "{}", state.message());

    // Idle: the review passes, then the restored database is written to
    // before the rollback runs; core refuses the stale operation.
    state.request_rollback_review(context.clone(), path.clone(), false);
    settle(&context, &mut state, false);
    assert!(state.rollback_review().is_some());
    let mut database = Database::open_or_create(&estate.live).unwrap();
    database.add_platform_alias("after-restore", "nes").unwrap();
    database.close().unwrap();
    state.set_confirmation(CONFIRMATION);
    state.request_rollback(context.clone(), false);
    let (_, reload) = settle(&context, &mut state, false);
    assert!(!reload);
    assert!(
        state.message().contains("changed since the restore"),
        "{}",
        state.message()
    );
    assert!(
        state.rollback_review().is_none(),
        "a refused rollback drops the stale review"
    );
    assert_eq!(aliases(&estate.live), vec!["after-restore".to_string()]);
    let on_disk = read_database_restore_receipt(&path).unwrap();
    assert_eq!(
        on_disk.state,
        DatabaseRestoreState::Completed,
        "{}",
        receipt.operation_id
    );
    assert!(on_disk.emergency_backup_path.as_ref().unwrap().exists());
}

#[test]
fn blue_history_page_shows_the_recovery_section_for_a_pinned_database() {
    let estate = estate();
    completed_restore(&estate);
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.database_recovery = pinned(&estate.live);
    app.go(Route::Section(Section::History));
    let started = Instant::now();
    let strings = loop {
        let output = frame(&context, &mut app, [1280.0, 900.0]);
        let strings = text(&output);
        if strings.iter().any(|s| s == "Database restore completed") {
            break strings;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "{strings:?}");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        strings.iter().any(|s| s == "Database restore & recovery"),
        "{strings:?}"
    );
    assert!(
        strings.iter().any(|s| s == "Review rollback"),
        "{strings:?}"
    );
}
