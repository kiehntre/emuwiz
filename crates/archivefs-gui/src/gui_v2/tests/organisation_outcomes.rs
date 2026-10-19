//! Blue verified-game organisation reports only outcomes it can prove. Real
//! core Apply/Undo on disposable files, handed to the real job finisher.
use super::super::CanonicalOrganisationJobKind as Kind;
use super::*;
use crate::rom_organisation_page::RomOrganisationPageState;
use archivefs_core::dat::rename_apply::identity::capture_identity;
use archivefs_core::dat::rename_apply::model::{
    EntryState, RenameTransaction, TransactionEntry, TransactionState,
};
use archivefs_core::dat::rename_apply::preflight::DirectoryPolicy;
use archivefs_core::dat::rename_apply::{
    ApplyError, ApplyExecution, ApplyOutcome, HardConflictMode, apply_transaction,
};
use archivefs_core::safe_read::TrustedRoots;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Writes `<name>.old.bin` and plans its rename to `destination`.
fn entry(root: &Path, name: &str, destination: PathBuf) -> TransactionEntry {
    let source = root.join(format!("{name}.old.bin"));
    fs::write(&source, format!("bytes of {name}")).unwrap();
    TransactionEntry {
        source_path: source.clone(),
        original_basename: format!("{name}.old.bin"),
        proposed_basename: destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        destination_path: destination,
        identity: capture_identity(&source).unwrap(),
        operation: Default::default(),
        preflight_passed: false,
        preflight_failures: Vec::new(),
        state: EntryState::Planned,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    }
}

/// A real journalled core apply of `entries` inside `root`.
fn real_apply(
    root: &Path,
    journal: &Path,
    entries: Vec<TransactionEntry>,
) -> (Result<ApplyOutcome, ApplyError>, RenameTransaction) {
    let approved = entries
        .iter()
        .map(|entry| entry.source_path.to_string_lossy().into_owned())
        .collect();
    let mut transaction = RenameTransaction {
        transaction_id: archivefs_core::dat::rename_apply::journal::new_transaction_id(now()),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: now(),
        source_scan_root: root.to_string_lossy().into_owned(),
        state: TransactionState::Planned,
        entries,
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = apply_transaction(&mut ApplyExecution {
        transaction: &mut transaction,
        approved_paths: approved,
        current_generation: 1,
        trusted: TrustedRoots::from_paths([root]),
        journal_dir: journal.to_path_buf(),
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &cancel,
        directory_policy: DirectoryPolicy::SameFilesystem,
        allow_symlink_source: false,
    });
    (result, transaction)
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    journal: PathBuf,
}

fn disposable() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap().join("roms");
    let journal = fs::canonicalize(dir.path()).unwrap().join("journal");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&journal).unwrap();
    Fixture {
        _dir: dir,
        root,
        journal,
    }
}

fn reviewed_page(fixture: &Fixture) -> RomOrganisationPageState {
    let mut page = RomOrganisationPageState::default();
    page.configure_for_test(fixture.root.clone(), fixture.journal.clone());
    page.hold_plan_for_test(&fixture.root, "reviewed-source");
    page
}

/// Hands a worker's page state to the real finisher, as the shell does.
fn finish(app: &mut App, kind: Kind, worker: RomOrganisationPageState) -> u64 {
    let id = app
        .activity
        .queue("Organisation", Route::Section(Section::Build), false);
    let generation = app.canonical_organisation_generation;
    app.canonical_organisation_job = Some(super::super::CanonicalOrganisationJob {
        id,
        kind,
        generation,
        input_fingerprint: app.canonical_organisation.input_fingerprint(),
    });
    app.finish_canonical_organisation_job(id, kind, Box::new(worker), generation);
    id
}

/// An app whose page already holds a completed real apply of `names`.
fn applied_app(fixture: &Fixture, names: &[&str]) -> (App, RenameTransaction, u64) {
    let entries = names
        .iter()
        .map(|name| {
            entry(
                &fixture.root,
                name,
                fixture.root.join(format!("{name}.new.bin")),
            )
        })
        .collect();
    let (result, transaction) = real_apply(&fixture.root, &fixture.journal, entries);
    assert!(result.is_ok() && transaction.state == TransactionState::Applied);
    let context = egui::Context::default();
    let mut app = fixture_app(&context);
    let mut worker = reviewed_page(fixture);
    worker.record_apply_result_for_test(result, transaction.clone());
    app.canonical_organisation = reviewed_page(fixture);
    let id = finish(&mut app, Kind::Apply, worker);
    (app, transaction, id)
}

fn fixture_app(context: &egui::Context) -> App {
    super::fixture(context)
}

fn history_state(app: &App, id: &str) -> TransactionState {
    app.canonical_organisation_history
        .iter()
        .find(|item| item.transaction_id == id)
        .expect("the transaction is recorded in History")
        .state
}

/// Whether the page, as rendered, offers its Undo action.
fn page_offers_undo(page: &mut RomOrganisationPageState) -> bool {
    let context = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1400.0, 4000.0),
        )),
        ..Default::default()
    };
    let output = context.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::rom_organisation_page::show_rom_organisation_page(ui, page);
        });
    });
    visible_text(&output, "Roll back this organisation")
}

fn undo(app: &mut App) -> u64 {
    let mut worker = app.canonical_organisation.clone();
    worker.rollback();
    finish(app, Kind::Rollback, worker)
}

#[test]
fn complete_apply_is_reported_as_completed_and_recorded_in_history() {
    let fixture = disposable();
    let (app, transaction, id) = applied_app(&fixture, &["a"]);
    let job = &app.activity.jobs[&id];
    assert_eq!(job.phase, Phase::Complete);
    assert!(
        job.summary.contains("organisation completed"),
        "{}",
        job.summary
    );
    assert_eq!(
        history_state(&app, &transaction.transaction_id),
        TransactionState::Applied
    );
    assert_eq!(
        app.canonical_organisation.applied().unwrap().transaction_id,
        transaction.transaction_id
    );
    assert!(fixture.root.join("a.new.bin").exists());
}

#[test]
fn partial_apply_is_not_success_and_keeps_undo_plan_and_approvals() {
    let fixture = disposable();
    let locked = fixture.root.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(locked.join("probe"), b"x").is_ok() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("SKIPPED: directory permissions are not enforced for this user");
        return;
    }
    let entries = vec![
        entry(&fixture.root, "a", fixture.root.join("a.new.bin")),
        entry(&fixture.root, "b", locked.join("b.new.bin")),
    ];
    let (result, transaction) = real_apply(&fixture.root, &fixture.journal, entries);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    // The engine returns Ok for a batch that failed after preflight.
    let summary = &result
        .as_ref()
        .expect("post-preflight failure is Ok")
        .summary;
    assert!(
        summary.applied == 1 && !summary.fully_resolved(),
        "{summary:?}"
    );

    let context = egui::Context::default();
    let mut app = fixture_app(&context);
    app.canonical_organisation = reviewed_page(&fixture);
    let mut worker = reviewed_page(&fixture);
    worker.record_apply_result_for_test(result, transaction.clone());
    let id = finish(&mut app, Kind::Apply, worker);

    let job = &app.activity.jobs[&id];
    assert_eq!(job.phase, Phase::Failed);
    assert!(job.summary.contains("did not complete"), "{}", job.summary);
    assert!(!job.summary.contains("organisation completed"));
    assert!(job.technical.contains("1 failed"), "{}", job.technical);
    assert_eq!(
        history_state(&app, &transaction.transaction_id),
        TransactionState::ApplyFailed
    );
    let page = &app.canonical_organisation;
    assert_eq!(
        page.applied().unwrap().transaction_id,
        transaction.transaction_id
    );
    assert!(page.has_plan_for_test());
    assert_eq!(
        page.approvals_for_test(),
        vec!["reviewed-source".to_string()]
    );
    assert!(fixture.root.join("a.new.bin").exists());
    assert!(fixture.root.join("b.old.bin").exists());
}

#[test]
fn failed_apply_without_changes_says_nothing_was_applied_and_records_nothing() {
    let fixture = disposable();
    let entries = vec![entry(&fixture.root, "a", fixture.root.join("a.new.bin"))];
    let (result, transaction) = real_apply(&fixture.root, &fixture.journal, entries);
    assert!(result.is_ok());
    // A stale plan is refused before any mutation.
    let refused = archivefs_core::dat::rename_apply::ApplyError::StalePlan {
        plan: 1,
        current: 2,
    };
    let mut untouched = transaction.clone();
    for entry in &mut untouched.entries {
        entry.state = EntryState::Planned;
    }
    let context = egui::Context::default();
    let mut app = fixture_app(&context);
    app.canonical_organisation = reviewed_page(&fixture);
    let mut worker = reviewed_page(&fixture);
    worker.record_apply_result_for_test(Err(refused), untouched);
    let id = finish(&mut app, Kind::Apply, worker);
    let job = &app.activity.jobs[&id];
    assert_eq!(job.phase, Phase::Failed);
    assert!(job.summary.contains("stopped safely"), "{}", job.summary);
    assert!(app.canonical_organisation_history.is_empty());
    assert!(app.canonical_organisation.applied().is_none());
    assert!(app.canonical_organisation.has_plan_for_test());
}

#[test]
fn failed_apply_after_changes_keeps_an_undo_handle_and_history() {
    let fixture = disposable();
    let entries = vec![entry(&fixture.root, "a", fixture.root.join("a.new.bin"))];
    let (_, moved) = real_apply(&fixture.root, &fixture.journal, entries);
    // As when a journal write fails after the rename already happened.
    let failure = archivefs_core::dat::rename_apply::ApplyError::Journal("disk full".into());
    let context = egui::Context::default();
    let mut app = fixture_app(&context);
    app.canonical_organisation = reviewed_page(&fixture);
    let mut worker = reviewed_page(&fixture);
    worker.record_apply_result_for_test(Err(failure), moved.clone());
    let id = finish(&mut app, Kind::Apply, worker);
    let job = &app.activity.jobs[&id];
    assert_eq!(job.phase, Phase::Failed);
    assert!(
        job.summary.contains("Some changes may have been made"),
        "{}",
        job.summary
    );
    assert_eq!(
        app.canonical_organisation.applied().unwrap().transaction_id,
        moved.transaction_id
    );
    assert!(
        app.canonical_organisation_history
            .iter()
            .any(|item| item.transaction_id == moved.transaction_id)
    );
    assert!(page_offers_undo(&mut app.canonical_organisation));
}

#[test]
fn complete_undo_is_proven_before_history_is_marked_rolled_back() {
    let fixture = disposable();
    let (mut app, transaction, _) = applied_app(&fixture, &["a"]);
    let id = undo(&mut app);
    let job = &app.activity.jobs[&id];
    assert_eq!(job.phase, Phase::Complete);
    assert!(job.summary.contains("undone safely"), "{}", job.summary);
    assert_eq!(
        history_state(&app, &transaction.transaction_id),
        TransactionState::RolledBack
    );
    assert!(fixture.root.join("a.old.bin").exists());
    assert!(!fixture.root.join("a.new.bin").exists());
    assert!(app.canonical_organisation.applied().is_none());
    // History holds the journal receipt, not just a relabelled state.
    let recorded = app
        .canonical_organisation_history
        .iter()
        .find(|item| item.transaction_id == transaction.transaction_id)
        .unwrap();
    assert!(recorded.is_fully_rolled_back());
    assert!(!page_offers_undo(&mut app.canonical_organisation));
}

#[test]
fn partial_undo_keeps_handle_plan_and_never_marks_history_rolled_back() {
    let fixture = disposable();
    let (mut app, transaction, _) = applied_app(&fixture, &["a", "b"]);
    // Undo reverses b first, then finds a's original name taken.
    let blocker = fixture.root.join("a.old.bin");
    fs::write(&blocker, b"someone else's file").unwrap();

    let id = undo(&mut app);
    let job = &app.activity.jobs[&id];
    assert_eq!(job.phase, Phase::Failed);
    assert!(
        job.summary.contains("Undo did not finish"),
        "{}",
        job.summary
    );
    assert!(!job.summary.contains("undone safely"));
    assert_eq!(
        history_state(&app, &transaction.transaction_id),
        TransactionState::RollbackFailed
    );
    let page = &app.canonical_organisation;
    assert_eq!(
        page.applied().unwrap().transaction_id,
        transaction.transaction_id
    );
    assert!(page.has_plan_for_test());
    assert_eq!(
        page.approvals_for_test(),
        vec!["reviewed-source".to_string()]
    );
    assert!(fixture.root.join("b.old.bin").exists());
    assert!(fixture.root.join("a.new.bin").exists());
    assert_eq!(fs::read(&blocker).unwrap(), b"someone else's file");

    assert!(page_offers_undo(&mut app.canonical_organisation));

    // The blocker still present: the guarded retry changes nothing.
    let again = undo(&mut app);
    assert_eq!(app.activity.jobs[&again].phase, Phase::Failed);
    assert!(
        app.activity.jobs[&again].technical.contains("NOT complete"),
        "{}",
        app.activity.jobs[&again].technical
    );
    assert_ne!(
        history_state(&app, &transaction.transaction_id),
        TransactionState::RolledBack
    );
    assert_eq!(fs::read(&blocker).unwrap(), b"someone else's file");
    assert!(fixture.root.join("a.new.bin").exists());

    // Once the obstruction is gone, Undo retries that file, and only then is
    // History marked rolled back.
    fs::remove_file(&blocker).unwrap();
    let last = undo(&mut app);
    assert_eq!(app.activity.jobs[&last].phase, Phase::Complete);
    assert_eq!(
        history_state(&app, &transaction.transaction_id),
        TransactionState::RolledBack
    );
    assert!(fixture.root.join("a.old.bin").exists());
    assert!(!fixture.root.join("a.new.bin").exists());
    assert!(app.canonical_organisation.applied().is_none());
}

#[test]
fn guarded_undo_refusals_change_nothing_and_keep_the_handle() {
    let fixture = disposable();
    let (mut app, transaction, _) = applied_app(&fixture, &["a"]);

    // Advanced Protection needs its own separate approval.
    let mut worker = app.canonical_organisation.clone();
    worker.select_advanced_protection_for_test(true);
    worker.rollback();
    let id = finish(&mut app, Kind::Rollback, worker);
    assert_eq!(app.activity.jobs[&id].phase, Phase::Failed);
    assert!(
        app.activity.jobs[&id]
            .summary
            .contains("Undo could not finish safely")
    );

    // The organisation folder is unavailable.
    let mut worker = app.canonical_organisation.clone();
    worker.select_advanced_protection_for_test(false);
    worker.clear_root_for_test();
    worker.rollback();
    let id = finish(&mut app, Kind::Rollback, worker);
    assert_eq!(app.activity.jobs[&id].phase, Phase::Failed);
    assert!(app.activity.jobs[&id].technical.contains("not available"));
    // The kept handle is still reachable from the page once the folder returns.
    let mut remounted = app.canonical_organisation.clone();
    remounted.configure_for_test(fixture.root.clone(), fixture.journal.clone());
    assert!(page_offers_undo(&mut remounted));

    assert_eq!(
        history_state(&app, &transaction.transaction_id),
        TransactionState::Applied
    );
    assert!(fixture.root.join("a.new.bin").exists());
    assert!(!fixture.root.join("a.old.bin").exists());
    // The guarded page still holds the original handle. Restore its root and
    // standard protection; the earlier refusals are not carried forward.
    app.canonical_organisation
        .configure_for_test(fixture.root.clone(), fixture.journal.clone());
    app.canonical_organisation
        .select_advanced_protection_for_test(false);
    assert_eq!(
        app.canonical_organisation.applied().unwrap().transaction_id,
        transaction.transaction_id
    );
    let id = undo(&mut app);
    assert_eq!(app.activity.jobs[&id].phase, Phase::Complete);
    assert_eq!(
        history_state(&app, &transaction.transaction_id),
        TransactionState::RolledBack
    );
}

#[test]
fn undo_without_a_handle_is_refused_and_history_is_untouched() {
    let fixture = disposable();
    let context = egui::Context::default();
    let mut app = fixture_app(&context);
    app.canonical_organisation = reviewed_page(&fixture);
    let id = undo(&mut app);
    let job = &app.activity.jobs[&id];
    assert_eq!(job.phase, Phase::Failed);
    assert!(
        job.technical.contains("no applied organisation"),
        "{}",
        job.technical
    );
    assert!(app.canonical_organisation_history.is_empty());
}
