//! Night Watch Blue wiring: the outcome Blue shows must agree with the
//! receipt or payload the worker actually produced (S04-03 / S06-05 /
//! S06-06 / S06-07 / NW2-B-06).
use super::*;
use crate::gui_v2::activity::Phase;
use crate::gui_v2::backend::DuplicateRepairRecord;
use crate::gui_v2::quarantine_outcome::tests::{partial_failure, transaction};
use archivefs_core::dat::rename_apply::model::TransactionState;

fn record(
    transaction: archivefs_core::dat::rename_apply::model::RenameTransaction,
) -> DuplicateRepairRecord {
    DuplicateRepairRecord {
        transaction,
        trusted_root: "/games".into(),
        journal_dir: "/journal".into(),
    }
}

fn phase(app: &App, id: u64) -> Phase {
    app.activity.jobs[&id].phase.clone()
}

#[test]
fn duplicate_quarantine_that_stopped_part_way_is_not_shown_as_moved() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.duplicate_report = Some(Default::default());
    let id = app.activity.queue(
        "Moving the duplicate copy",
        Route::Section(Section::Problems),
        true,
    );
    app.activity.start(id);
    app.repair_job = Some(id);

    app.duplicate_applied(id, record(partial_failure()));

    assert_eq!(
        phase(&app, id),
        Phase::Failed,
        "a partial batch needs attention"
    );
    assert_eq!(app.activity.failed(), 1);
    let shown = app.repair_result.clone().unwrap();
    assert!(shown.contains("moved 1 of 3"), "{shown}");
    assert!(shown.contains("changed since the check"), "{shown}");
    assert!(
        !shown.starts_with("The duplicate copy was moved"),
        "{shown}"
    );
    assert_eq!(app.repair_job, None);
    assert_eq!(
        app.repair_history.len(),
        1,
        "the receipt is kept for History Undo"
    );
    assert!(
        app.duplicate_report.is_none(),
        "the stale duplicate report is dropped"
    );
    assert!(app.problem_summary.is_none());
}

#[test]
fn duplicate_quarantine_that_moved_everything_is_finished() {
    use archivefs_core::dat::rename_apply::model::EntryState;
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let id = app.activity.queue(
        "Moving the duplicate copy",
        Route::Section(Section::Problems),
        true,
    );
    app.activity.start(id);
    app.repair_job = Some(id);
    let mut done = partial_failure();
    done.state = TransactionState::Applied;
    for entry in &mut done.entries {
        entry.state = EntryState::Applied;
        entry.failure_reason = None;
    }

    app.duplicate_applied(id, record(done));

    assert_eq!(phase(&app, id), Phase::Complete);
    assert!(
        app.repair_result
            .clone()
            .unwrap()
            .starts_with("Moved 3 file(s)")
    );
}

#[test]
fn duplicate_quarantine_cancelled_between_entries_is_stopped() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let id = app.activity.queue(
        "Moving the duplicate copy",
        Route::Section(Section::Problems),
        true,
    );
    app.activity.start(id);
    app.repair_job = Some(id);
    app.activity.jobs[&id].request_cancel();

    app.duplicate_applied(id, record(partial_failure()));

    assert_eq!(phase(&app, id), Phase::Cancelled);
    assert!(
        app.repair_result
            .clone()
            .unwrap()
            .contains("you stopped it")
    );
}

#[test]
fn equivalent_duplicate_apply_that_stopped_part_way_reports_an_error_not_a_move() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.equiv.job = Some(5);

    app.equivalent_applied(record(partial_failure()));

    assert!(
        app.equiv.result.is_none(),
        "no 'Moved N' claim with an Undo link"
    );
    let error = app.equiv.error.clone().unwrap();
    assert!(error.contains("moved 1 of 3"), "{error}");
    assert_eq!(app.repair_history.len(), 1);
}

#[test]
fn equivalent_duplicate_apply_that_finished_still_reports_the_move() {
    use archivefs_core::dat::rename_apply::model::EntryState;
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let mut done = partial_failure();
    done.state = TransactionState::Applied;
    for entry in &mut done.entries {
        entry.state = EntryState::Applied;
        entry.failure_reason = None;
    }
    app.equivalent_applied(record(done));
    let (message, transaction_id) = app.equiv.result.clone().unwrap();
    assert!(message.starts_with("Moved 3 file(s)"), "{message}");
    assert_eq!(transaction_id, "dup-test");
    assert!(app.equiv.error.is_none());
}

#[test]
fn library_scan_that_skipped_folders_needs_attention_once() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let id = app.activity.queue(
        "Scanning your game folders",
        Route::Section(Section::Games),
        false,
    );
    app.activity.start(id);
    app.load_job = Some(id);
    let warning = "2 game folders could not be read. Their previous entries were kept.".to_string();
    let library = Arc::new(Library {
        scan_warning: Some(warning.clone()),
        ..Library::default()
    });

    app.library_loaded(id, library);

    let job = &app.activity.jobs[&id];
    assert_eq!(job.phase, Phase::Failed, "{}", job.summary);
    assert!(job.summary.starts_with(&warning), "{}", job.summary);
    assert_eq!(job.technical, warning);
    assert_eq!(app.activity.failed(), 1);
    assert_eq!(
        app.notice.as_ref().map(|n| n.message.clone()),
        Some(warning)
    );
    assert!(app.loaded);
}

#[test]
fn library_scan_without_problems_is_finished() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let id = app.activity.queue(
        "Scanning your game folders",
        Route::Section(Section::Games),
        false,
    );
    app.activity.start(id);
    app.library_loaded(id, Arc::new(Library::default()));
    assert_eq!(phase(&app, id), Phase::Complete);
    assert_eq!(app.activity.failed(), 0);
    assert!(app.notice.is_none());
}

#[test]
fn history_undo_refused_while_another_undo_runs_says_so() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let running = record(transaction(TransactionState::Applied, Vec::new()));
    app.repair_history.push(record({
        let mut t = transaction(TransactionState::Applied, Vec::new());
        t.transaction_id = "second".into();
        t
    }));
    app.undo_job = Some(crate::gui_v2::duplicate_undo::DuplicateUndoJob {
        id: 9,
        generation: 1,
        before: running,
    });

    app.undo_history_entry("second");

    assert!(
        app.undo_job.as_ref().is_some_and(|job| job.id == 9),
        "the running Undo is untouched"
    );
    let notice = app
        .history_view
        .notice
        .clone()
        .expect("the refusal is visible");
    assert!(notice.contains("Another Undo is still running"), "{notice}");
    app.router.current = Route::Section(Section::History);
    let strings = scrolled_text(&context, &mut app, [1280.0, 820.0]);
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Another Undo is still running"))
    );
}

#[test]
fn history_undo_of_an_unloaded_receipt_says_so() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.undo_history_entry("never-loaded");
    assert!(app.undo_job.is_none());
    let notice = app.history_view.notice.clone().unwrap();
    assert!(notice.contains("no longer loaded"), "{notice}");
}
