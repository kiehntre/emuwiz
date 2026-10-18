use super::*;
use archivefs_core::dat::rename_apply::{journal, model::TransactionState};
use archivefs_core::repair::{
    DuplicateHashCache, KeeperEvidence, apply_quarantine_transaction, build_quarantine_transaction,
    plan_duplicate_quarantine,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;
use std::fs;

struct Fixture {
    _dir: tempfile::TempDir,
    record: DuplicateRepairRecord,
    survivor: PathBuf,
}

impl Fixture {
    fn new(copies: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        fs::create_dir(&root).unwrap();
        let survivor = root.join("keeper.bin");
        fs::write(&survivor, b"synthetic duplicate progress").unwrap();
        let mut evidence = vec![KeeperEvidence {
            path: survivor.clone(),
            already_canonical: true,
            verified_confident: true,
            dat_source_id: None,
            dat_source_display: None,
            game_name: None,
            rom_name: None,
            verdict_label: None,
        }];
        for n in 0..copies {
            let path = root.join(format!("copy-{n}.bin"));
            fs::write(&path, b"synthetic duplicate progress").unwrap();
            evidence.push(KeeperEvidence {
                path,
                already_canonical: false,
                verified_confident: false,
                ..evidence[0].clone()
            });
        }
        let trusted = TrustedRoots::from_paths([&root]);
        let mut cache = DuplicateHashCache::new();
        let plan = plan_duplicate_quarantine(&evidence, &root, &trusted, &mut cache, None).unwrap();
        let mut tx = build_quarantine_transaction(
            &plan.proposals,
            &survivor,
            &root,
            1,
            &mut cache,
            &trusted,
            None,
        )
        .unwrap();
        let journal_dir = dir.path().join("journal");
        apply_quarantine_transaction(
            &mut tx,
            &survivor,
            &root,
            1,
            trusted,
            &journal_dir,
            &AtomicBool::new(false),
            &mut cache,
        )
        .unwrap();
        Self {
            _dir: dir,
            record: DuplicateRepairRecord {
                transaction: tx,
                trusted_root: root,
                journal_dir,
            },
            survivor,
        }
    }

    fn persisted(&self) -> archivefs_core::dat::rename_apply::model::RenameTransaction {
        journal::read_journal(
            &journal::journal_path(
                &self.record.journal_dir,
                &self.record.transaction.transaction_id,
            )
            .unwrap(),
        )
        .unwrap()
    }
}

fn deliver(record: DuplicateRepairRecord, cancel: bool) -> super::super::App {
    let (answers, _rx) = mpsc::channel();
    let payload = execute(
        1,
        Command::UndoDuplicateRepair {
            generation: 1,
            record: Box::new(record.clone()),
            cancel: Arc::new(AtomicBool::new(cancel)),
        },
        &answers,
    );
    deliver_payload(record, payload)
}

fn deliver_payload(
    record: DuplicateRepairRecord,
    outcome: Result<Payload, String>,
) -> super::super::App {
    let context = egui::Context::default();
    let mut app = super::super::tests::fixture(&context);
    let (requests, _rx) = mpsc::channel();
    let (answers, rx) = mpsc::channel();
    app.backend = Backend { tx: requests, rx };
    app.repair_history.push(record.clone());
    let id = app.activity.queue(
        "Undoing the duplicate quarantine",
        super::super::routes::Route::Section(super::super::routes::Section::History),
        false,
    );
    app.undo_generation = 1;
    app.undo_job = Some(super::super::duplicate_undo::DuplicateUndoJob {
        id,
        generation: 1,
        before: record,
    });
    answers.send(Event::Finished { id, outcome }).unwrap();
    app.poll(&context);
    assert_eq!(
        app.activity.jobs.values().next().unwrap().summary,
        *app.repair_result.as_ref().unwrap(),
        "the generic completion path must not replace the Undo result"
    );
    app
}

#[test]
fn failed_undo_is_not_reported_as_gui_success() {
    let fx = Fixture::new(1);
    let entry = &fx.record.transaction.entries[0];
    fs::write(&entry.source_path, b"newer unrelated content").unwrap();
    let app = deliver(fx.record.clone(), false);
    assert_eq!(fx.persisted().state, TransactionState::RollbackFailed);
    assert_eq!(
        fs::read(&entry.source_path).unwrap(),
        b"newer unrelated content"
    );
    assert_eq!(
        fs::read(&entry.destination_path).unwrap(),
        b"synthetic duplicate progress"
    );
    assert_ne!(
        app.repair_result.as_deref(),
        Some("The quarantined file was restored safely.")
    );
    assert!(matches!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Failed
    ));
}

fn history(app: &super::super::App) -> super::super::history_view::HistoryEntry {
    let record = &app.repair_history[0];
    super::super::history_view::HistoryEntry::from_transaction(
        &record.transaction,
        super::super::history_view::Source::DuplicateQuarantine,
        Some(&record.journal_dir),
    )
}

fn assert_incomplete(app: &super::super::App) {
    use super::super::history_view::{Outcome, UndoStatus};
    let entry = history(app);
    assert_eq!(entry.outcome, Outcome::NeedsAttention);
    assert_eq!(entry.undone_when, None);
    assert_eq!(entry.undo, UndoStatus::Available);
    assert!(
        !app.history_view
            .previews
            .contains_key(&entry.transaction_id)
    );
    assert!(app.undo_job.is_none());
}

#[test]
fn full_undo_preserves_success_and_exact_original_bytes() {
    let fx = Fixture::new(2);
    let app = deliver(fx.record.clone(), false);
    assert_eq!(
        app.repair_result.as_deref(),
        Some("The quarantined file was restored safely.")
    );
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Complete
    );
    assert_eq!(fx.persisted().state, TransactionState::RolledBack);
    for entry in &fx.record.transaction.entries {
        assert_eq!(
            fs::read(&entry.source_path).unwrap(),
            b"synthetic duplicate progress"
        );
        assert!(!entry.destination_path.exists());
    }
    assert_eq!(
        fs::read(&fx.survivor).unwrap(),
        b"synthetic duplicate progress"
    );
    assert_eq!(
        history(&app).undo,
        super::super::history_view::UndoStatus::AlreadyUndone
    );
    // An already completed receipt remains a safe, successful no-op.
    let app = deliver(app.repair_history[0].clone(), true);
    assert_eq!(
        history(&app).undo,
        super::super::history_view::UndoStatus::AlreadyUndone
    );
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Complete
    );
}

#[test]
fn partial_undo_keeps_obstructed_bytes_and_recovery_preview() {
    let fx = Fixture::new(2);
    let blocked = &fx.record.transaction.entries[0];
    let restored = &fx.record.transaction.entries[1];
    fs::write(&blocked.source_path, b"newer progress").unwrap();
    let app = deliver(fx.record.clone(), false);
    assert!(
        app.repair_result
            .as_ref()
            .unwrap()
            .contains("only partially restored")
    );
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Failed
    );
    assert_incomplete(&app);
    assert_eq!(fx.persisted().state, TransactionState::RollbackFailed);
    assert_eq!(fx.persisted(), app.repair_history[0].transaction);
    assert_eq!(fs::read(&blocked.source_path).unwrap(), b"newer progress");
    assert_eq!(
        fs::read(&blocked.destination_path).unwrap(),
        b"synthetic duplicate progress"
    );
    assert_eq!(
        fs::read(&restored.source_path).unwrap(),
        b"synthetic duplicate progress"
    );
    assert!(!restored.destination_path.exists());
    assert!(blocked.destination_path.parent().unwrap().exists());
    let preview = super::super::history_view::check_undo_safety(&history(&app));
    assert_eq!(preview.checked, 1);
    assert!(!preview.safe());
    fs::remove_file(&blocked.source_path).unwrap();
    assert!(super::super::history_view::check_undo_safety(&history(&app)).safe());
    let retry = deliver(app.repair_history[0].clone(), false);
    assert_eq!(fx.persisted().state, TransactionState::RolledBack);
    assert_eq!(
        fs::read(&blocked.source_path).unwrap(),
        b"synthetic duplicate progress"
    );
    assert_eq!(
        history(&retry).undo,
        super::super::history_view::UndoStatus::AlreadyUndone
    );
}

#[test]
fn repeated_failure_never_becomes_success_and_can_retry_after_obstruction_removed() {
    let fx = Fixture::new(1);
    let entry = &fx.record.transaction.entries[0];
    fs::write(&entry.source_path, b"newer unrelated content").unwrap();
    let first = deliver(fx.record.clone(), false);
    assert_incomplete(&first);
    let second = deliver(first.repair_history[0].clone(), false);
    assert_incomplete(&second);
    assert!(
        second
            .repair_result
            .as_ref()
            .unwrap()
            .contains("Undo remains uncertain")
    );
    assert_eq!(fx.persisted().state, TransactionState::RollbackFailed);
    assert_eq!(
        fs::read(&entry.source_path).unwrap(),
        b"newer unrelated content"
    );
    assert!(entry.destination_path.exists());
    fs::remove_file(&entry.source_path).unwrap();
    let retry = deliver(second.repair_history[0].clone(), false);
    assert_eq!(fx.persisted().state, TransactionState::RolledBack);
    assert_eq!(
        fs::read(&entry.source_path).unwrap(),
        b"synthetic duplicate progress"
    );
    assert!(!entry.destination_path.exists());
    assert_eq!(
        history(&retry).undo,
        super::super::history_view::UndoStatus::AlreadyUndone
    );
}

#[test]
fn cancelled_before_first_move_keeps_retry_eligible() {
    let fx = Fixture::new(1);
    let app = deliver(fx.record.clone(), true);
    assert_incomplete(&app);
    assert!(
        app.repair_result
            .as_ref()
            .unwrap()
            .contains("stopped before restoration completed")
    );
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Cancelled
    );
    assert_eq!(fx.persisted().state, TransactionState::RollbackFailed);
    assert!(!fx.record.transaction.entries[0].source_path.exists());
    assert!(fx.record.transaction.entries[0].destination_path.exists());
    let retry = deliver(app.repair_history[0].clone(), false);
    assert_eq!(fx.persisted().state, TransactionState::RolledBack);
    assert_eq!(
        history(&retry).undo,
        super::super::history_view::UndoStatus::AlreadyUndone
    );
}

#[test]
fn cancelled_after_partial_progress_survives_restart_and_retry() {
    let fx = Fixture::new(32);
    let first_restored = fx
        .record
        .transaction
        .entries
        .last()
        .unwrap()
        .source_path
        .clone();
    let record = fx.record.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let worker = std::thread::spawn(move || {
        let (answers, _rx) = mpsc::channel();
        execute(
            1,
            Command::UndoDuplicateRepair {
                generation: 1,
                record: Box::new(record),
                cancel: worker_cancel,
            },
            &answers,
        )
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !first_restored.exists() && !worker.is_finished() && std::time::Instant::now() < deadline
    {
        std::thread::yield_now();
    }
    cancel.store(true, Ordering::Relaxed);
    let payload = worker.join().unwrap().unwrap();
    let Payload::DuplicateUndone(result) = &payload else {
        panic!("wrong Undo payload")
    };
    assert!(matches!(
        result.outcome.as_ref().unwrap().rollback.result,
        archivefs_core::dat::rename_apply::model::RollbackResult::PartiallyRolledBack { .. }
    ));
    let app = deliver_payload(fx.record.clone(), Ok(payload));
    assert_incomplete(&app);
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Cancelled
    );
    let persisted = fx.persisted();
    assert_eq!(persisted.state, TransactionState::RollbackFailed);
    assert_eq!(persisted, app.repair_history[0].transaction);
    use archivefs_core::dat::rename_apply::model::EntryState;
    assert!(
        persisted
            .entries
            .iter()
            .any(|entry| entry.state == EntryState::RolledBack)
    );
    assert!(
        persisted
            .entries
            .iter()
            .any(|entry| entry.state == EntryState::Applied)
    );
    for entry in &persisted.entries {
        if entry.state == EntryState::RolledBack {
            assert_eq!(
                fs::read(&entry.source_path).unwrap(),
                b"synthetic duplicate progress"
            );
            assert!(!entry.destination_path.exists());
        } else {
            assert!(!entry.source_path.exists());
            assert_eq!(
                fs::read(&entry.destination_path).unwrap(),
                b"synthetic duplicate progress"
            );
            assert!(entry.destination_path.parent().unwrap().exists());
        }
    }
    // Exercise real restart discovery using this test binary's private HOME.
    let restart_dir = journal::default_rename_transaction_dir().unwrap();
    journal::write_journal(&restart_dir, &persisted).unwrap();
    let (answers, _rx) = mpsc::channel();
    let Payload::RepairHistory { duplicates, .. } =
        execute(0, Command::LoadRepairHistory, &answers).unwrap()
    else {
        panic!("wrong History payload")
    };
    let record = duplicates
        .into_iter()
        .find(|record| record.transaction.transaction_id == persisted.transaction_id)
        .unwrap();
    assert_eq!(record.transaction, persisted);
    let retry = deliver(record, false);
    assert_eq!(
        history(&retry).undo,
        super::super::history_view::UndoStatus::AlreadyUndone
    );
    let reloaded = journal::read_journal(
        &journal::journal_path(&restart_dir, &persisted.transaction_id).unwrap(),
    )
    .unwrap();
    assert_eq!(reloaded.state, TransactionState::RolledBack);
    for entry in &persisted.entries {
        assert_eq!(
            fs::read(&entry.source_path).unwrap(),
            b"synthetic duplicate progress"
        );
        assert!(!entry.destination_path.exists());
    }
}

#[test]
fn journal_error_reports_unconfirmed_restoration_and_preserves_evidence() {
    let fx = Fixture::new(1);
    let backup = fx.record.journal_dir.with_extension("preserved");
    fs::rename(&fx.record.journal_dir, &backup).unwrap();
    fs::write(&fx.record.journal_dir, b"synthetic path obstruction").unwrap();
    let app = deliver(fx.record.clone(), false);
    assert!(
        app.repair_result
            .as_ref()
            .unwrap()
            .contains("could not confirm restoration")
    );
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Failed
    );
    // The obstructed journal namespace cannot prove that no coordinator owns
    // this receipt. Transaction-level retry eligibility cannot waive that check.
    use super::super::history_view::{Outcome, UndoStatus};
    let blocked = history(&app);
    assert_eq!(blocked.outcome, Outcome::NeedsAttention);
    assert_eq!(blocked.undone_when, None);
    assert!(matches!(
        blocked.undo,
        UndoStatus::NeedsReview(reason)
            if reason.contains("Receipt ownership cannot be checked")
                && reason.contains("ownership namespace unavailable")
                && reason.contains("independent Undo is blocked")
    ));
    assert!(
        !app.history_view
            .previews
            .contains_key(&blocked.transaction_id)
    );
    assert!(app.undo_job.is_none());
    assert!(
        backup
            .join(format!("{}.json", fx.record.transaction.transaction_id))
            .exists()
    );
    let entry = &fx.record.transaction.entries[0];
    assert!(!entry.source_path.exists());
    assert_eq!(
        fs::read(&entry.destination_path).unwrap(),
        b"synthetic duplicate progress"
    );
    fs::remove_file(&fx.record.journal_dir).unwrap();
    fs::rename(&backup, &fx.record.journal_dir).unwrap();
    // Only the restored, inspectable namespace and unchanged receipt establish
    // independent Undo eligibility again. The actual retry still revalidates it.
    assert_eq!(fx.persisted(), fx.record.transaction);
    assert_eq!(
        archivefs_core::launch::es_de_publish::operation::is_coordinated(
            &fx.record.journal_dir,
            &fx.record.transaction,
        ),
        Ok(false)
    );
    assert_eq!(history(&app).undo, UndoStatus::Available);
    let retry = deliver(app.repair_history[0].clone(), false);
    assert_eq!(
        history(&retry).undo,
        super::super::history_view::UndoStatus::AlreadyUndone
    );
    assert_eq!(fx.persisted().state, TransactionState::RolledBack);
}

#[test]
fn retry_revalidates_changed_quarantined_content_without_claiming_success() {
    let fx = Fixture::new(1);
    let entry = &fx.record.transaction.entries[0];
    fs::write(&entry.source_path, b"newer unrelated content").unwrap();
    let first = deliver(fx.record.clone(), false);
    fs::remove_file(&entry.source_path).unwrap();
    fs::write(&entry.destination_path, b"changed quarantine payload").unwrap();
    let retry = deliver(first.repair_history[0].clone(), false);
    assert_incomplete(&retry);
    assert_eq!(fx.persisted().state, TransactionState::RollbackFailed);
    assert!(!entry.source_path.exists());
    assert_eq!(
        fs::read(&entry.destination_path).unwrap(),
        b"changed quarantine payload"
    );
    assert_eq!(
        retry.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Failed
    );
}

// Historical promotion blocker: exercise it against the promoted lookup correction.
// Unavailable evidence must still forbid completion and retain the in-flight entry.
#[cfg(target_os = "linux")]
#[test]
fn shared_engine_eloop_requires_inspectable_quarantine_namespace() {
    use archivefs_core::dat::rename_apply::model::{EntryState, RollbackResult};
    let fx = Fixture::new(1);
    let mut record = fx.record.clone();
    let entry = &mut record.transaction.entries[0];
    fs::rename(&entry.destination_path, &entry.source_path).unwrap();
    entry.state = EntryState::RollbackFailed;
    record.transaction.state = TransactionState::RollbackFailed;
    let source = entry.source_path.clone();
    let destination = entry.destination_path.clone();
    journal::write_journal(&record.journal_dir, &record.transaction).unwrap();
    let parent = destination.parent().unwrap();
    let preserved = fx
        .record
        .trusted_root
        .join("preserved-quarantine-directory");
    fs::rename(parent, &preserved).unwrap();
    std::os::unix::fs::symlink(parent.file_name().unwrap(), parent).unwrap();
    // Linux ELOOP: the namespace is unavailable, not proven absent.
    assert_eq!(
        fs::symlink_metadata(&destination)
            .unwrap_err()
            .raw_os_error(),
        Some(40)
    );
    let before = fs::read(&source).unwrap();
    let result = super::super::duplicate_undo::undo(record, &AtomicBool::new(false), 1);
    assert_eq!(fs::read(&source).unwrap(), before);
    assert_eq!(
        fs::read(&fx.survivor).unwrap(),
        b"synthetic duplicate progress"
    );
    assert!(
        !matches!(
            result.outcome.as_ref().unwrap().rollback.result,
            RollbackResult::FullyRolledBack
        ),
        "ELOOP must remain unresolved; shared reconcile.rs currently mistakes lookup failure for absence"
    );
    assert_eq!(
        result.record.transaction.state,
        TransactionState::RollbackFailed
    );
    assert_eq!(fx.persisted().state, TransactionState::RollbackFailed);
    assert_eq!(fx.persisted().entries[0].state, EntryState::RollingBack);
}

fn pending(app: &mut super::super::App, record: &DuplicateRepairRecord, generation: u64) -> u64 {
    let id = app.activity.queue(
        "Undo",
        super::super::routes::Route::Section(super::super::routes::Section::History),
        false,
    );
    app.undo_generation = generation;
    app.undo_job = Some(super::super::duplicate_undo::DuplicateUndoJob {
        id,
        generation,
        before: record.clone(),
    });
    id
}

#[test]
fn completion_requires_job_transaction_and_generation_identity() {
    let fx = Fixture::new(1);
    let ctx = egui::Context::default();
    let mut app = super::super::tests::fixture(&ctx);
    app.repair_history.push(fx.record.clone());
    let id = pending(&mut app, &fx.record, 7);
    let original = app.repair_history[0].transaction.clone();
    for (event_id, generation, wrong_transaction) in
        [(id + 1, 7, false), (id, 6, false), (id, 7, true)]
    {
        let mut record = fx.record.clone();
        if wrong_transaction {
            record.transaction.transaction_id = "different-transaction".into();
        }
        app.duplicate_undo_done(
            event_id,
            super::super::duplicate_undo::DuplicateUndoResult {
                generation,
                record,
                outcome: Err("obsolete failure".into()),
            },
        );
        assert_eq!(app.undo_job.as_ref().unwrap().id, id);
        assert_eq!(app.repair_history[0].transaction, original);
        assert!(app.activity.jobs[&id].active());
        assert!(app.repair_result.is_none());
    }
    let result = super::super::duplicate_undo::undo(fx.record.clone(), &AtomicBool::new(false), 7);
    app.duplicate_undo_done(id, result);
    assert!(app.undo_job.is_none());
    assert_eq!(
        app.repair_history[0].transaction.state,
        TransactionState::RolledBack
    );
}

#[test]
fn delayed_failed_completion_preserves_newer_success_and_busy_slot() {
    let fx = Fixture::new(1);
    let source = &fx.record.transaction.entries[0].source_path;
    fs::write(source, b"unrelated obstruction").unwrap();
    let old = super::super::duplicate_undo::undo(fx.record.clone(), &AtomicBool::new(false), 1);
    assert_eq!(
        old.record.transaction.state,
        TransactionState::RollbackFailed
    );
    fs::remove_file(source).unwrap();
    let ctx = egui::Context::default();
    let mut app = super::super::tests::fixture(&ctx);
    app.repair_history.push(old.record.clone());
    let old_id = pending(&mut app, &old.record, 1);
    let new_id = pending(&mut app, &old.record, 2);
    let newer = super::super::duplicate_undo::undo(old.record.clone(), &AtomicBool::new(false), 2);
    app.duplicate_undo_done(new_id, newer);
    assert_eq!(fx.persisted().state, TransactionState::RolledBack);
    let success = app.repair_result.clone();
    let latest = app.repair_history[0].clone();
    let busy = pending(&mut app, &latest, 3);
    app.duplicate_undo_done(old_id, old);
    assert_eq!(app.undo_job.as_ref().unwrap().id, busy);
    assert_eq!(app.repair_history[0].transaction, latest.transaction);
    assert_eq!(app.repair_result, success);
    assert_eq!(
        app.activity.jobs[&new_id].phase,
        super::super::activity::Phase::Complete
    );
    assert_eq!(
        app.activity.jobs[&old_id].phase,
        super::super::activity::Phase::Superseded
    );
    assert_eq!(fs::read(source).unwrap(), b"synthetic duplicate progress");
}

#[test]
fn matching_completion_cannot_replace_newer_history_receipt() {
    let fx = Fixture::new(1);
    let ctx = egui::Context::default();
    let mut app = super::super::tests::fixture(&ctx);
    app.repair_history.push(fx.record.clone());
    let id = pending(&mut app, &fx.record, 1);
    let newer = super::super::duplicate_undo::undo(fx.record.clone(), &AtomicBool::new(false), 2);
    app.repair_history[0] = newer.record.clone();
    app.duplicate_undo_done(
        id,
        super::super::duplicate_undo::DuplicateUndoResult {
            generation: 1,
            record: fx.record.clone(),
            outcome: Err("delayed failure".into()),
        },
    );
    assert_eq!(app.repair_history[0].transaction, newer.record.transaction);
    assert!(app.undo_job.is_none());
    assert_eq!(
        app.activity.jobs[&id].phase,
        super::super::activity::Phase::Superseded
    );
    assert!(app.repair_result.is_none());
}

#[test]
fn reordered_history_dispatches_immutable_transaction_id() {
    let fx = Fixture::new(1);
    let other = Fixture::new(1);
    let ctx = egui::Context::default();
    let mut app = super::super::tests::fixture(&ctx);
    let (tx, rx) = mpsc::channel();
    let (_answers, answers) = mpsc::channel();
    app.backend = Backend { tx, rx: answers };
    app.equivalent_applied(fx.record.clone());
    let target = app.equiv.result.as_ref().unwrap().1.clone();
    app.repair_history.insert(0, other.record.clone());
    app.undo_history_entry(&target);
    let (_, command) = rx.recv().unwrap();
    let Command::UndoDuplicateRepair {
        record, generation, ..
    } = command
    else {
        panic!("wrong command");
    };
    assert_eq!(
        record.transaction.transaction_id,
        fx.record.transaction.transaction_id
    );
    assert_eq!(generation, app.undo_generation);
    assert_eq!(
        app.undo_job.as_ref().unwrap().before.transaction,
        fx.record.transaction
    );
}

fn equivalent_panel_text(record: &DuplicateRepairRecord) -> String {
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(value) => {
                text.push_str(value.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, text);
                }
            }
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    let mut app = super::super::tests::fixture(&ctx);
    app.equiv.result = Some((
        "Synthetic quarantine receipt".into(),
        record.transaction.transaction_id.clone(),
    ));
    app.repair_history.push(record.clone());
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200., 1200.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.equivalent_duplicates_section(ui));
        },
    );
    let mut text = String::new();
    for shape in output.shapes {
        collect(&shape.shape, &mut text);
    }
    text
}

#[test]
fn equivalent_panel_reports_full_partial_failed_and_cancelled_restoration() {
    let full = Fixture::new(1);
    let restored =
        super::super::duplicate_undo::undo(full.record.clone(), &AtomicBool::new(false), 1);
    assert!(equivalent_panel_text(&restored.record).contains("Undone — the files are back"));
    let failed = Fixture::new(1);
    fs::write(
        &failed.record.transaction.entries[0].source_path,
        b"unrelated obstruction",
    )
    .unwrap();
    let result =
        super::super::duplicate_undo::undo(failed.record.clone(), &AtomicBool::new(false), 1);
    let text = equivalent_panel_text(&result.record);
    assert!(text.contains("Undo failed or remains unresolved"));
    assert!(text.contains("put the remaining files back"));
    assert!(!text.contains("Undone —"));
    let partial = Fixture::new(2);
    fs::write(
        &partial.record.transaction.entries[0].source_path,
        b"unrelated obstruction",
    )
    .unwrap();
    let result =
        super::super::duplicate_undo::undo(partial.record.clone(), &AtomicBool::new(false), 1);
    let text = equivalent_panel_text(&result.record);
    assert!(text.contains("Undo incomplete"));
    assert!(!text.contains("Undone —"));
    let cancelled = Fixture::new(1);
    let result =
        super::super::duplicate_undo::undo(cancelled.record.clone(), &AtomicBool::new(true), 1);
    let text = equivalent_panel_text(&result.record);
    assert!(text.contains("Undo stopped"));
    assert!(!text.contains("Undone —"));
    for state in [
        TransactionState::Planned,
        TransactionState::Applying,
        TransactionState::ApplyFailed,
    ] {
        let mut record = cancelled.record.clone();
        record.transaction.state = state;
        let text = equivalent_panel_text(&record);
        assert!(text.contains("Restoration needs review"));
        assert!(!text.contains("Undone —"));
    }
}

#[test]
fn dispatch_does_not_finish_current_activity_for_obsolete_generation() {
    let fx = Fixture::new(1);
    let ctx = egui::Context::default();
    let mut app = super::super::tests::fixture(&ctx);
    let (tx, _requests) = mpsc::channel();
    let (answers, rx) = mpsc::channel();
    app.backend = Backend { tx, rx };
    app.repair_history.push(fx.record.clone());
    let id = pending(&mut app, &fx.record, 2);
    answers
        .send(Event::Finished {
            id,
            outcome: Ok(Payload::DuplicateUndone(Box::new(
                super::super::duplicate_undo::DuplicateUndoResult {
                    generation: 1,
                    record: fx.record.clone(),
                    outcome: Err("obsolete".into()),
                },
            ))),
        })
        .unwrap();
    app.poll(&ctx);
    assert!(app.activity.jobs[&id].active());
    assert_eq!(app.undo_job.as_ref().unwrap().generation, 2);
    assert_eq!(app.repair_history[0].transaction, fx.record.transaction);
    assert!(app.repair_result.is_none());
}

#[test]
fn missing_quarantined_file_never_becomes_restored_on_repeated_undo() {
    let fx = Fixture::new(1);
    let entry = &fx.record.transaction.entries[0];
    fs::remove_file(&entry.destination_path).unwrap();
    let first = deliver(fx.record.clone(), false);
    assert_incomplete(&first);
    let second = deliver(first.repair_history[0].clone(), false);
    assert_incomplete(&second);
    assert!(
        second
            .repair_result
            .as_deref()
            .unwrap()
            .contains("uncertain")
    );
    assert!(!entry.source_path.exists());
    assert!(!entry.destination_path.exists());
    assert_eq!(fx.persisted(), second.repair_history[0].transaction);
    assert_eq!(
        fs::read(&fx.survivor).unwrap(),
        b"synthetic duplicate progress"
    );
}

#[cfg(unix)]
#[test]
fn inaccessible_interrupted_quarantine_keeps_uncertainty_and_can_retry() {
    use archivefs_core::dat::rename_apply::model::EntryState;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    struct RestoreMode(PathBuf, u32);
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            fs::set_permissions(&self.0, fs::Permissions::from_mode(self.1)).unwrap();
        }
    }
    let fx = Fixture::new(1);
    if fs::metadata(&fx.record.trusted_root).unwrap().uid() == 0 {
        return; // Root bypasses DAC; the ELOOP refusal above still runs.
    }
    let entry = &fx.record.transaction.entries[0];
    let parent = entry.destination_path.parent().unwrap();
    let guard = RestoreMode(
        parent.to_owned(),
        fs::metadata(parent).unwrap().permissions().mode(),
    );
    let mut record = fx.record.clone();
    record.transaction.state = TransactionState::RollingBack;
    record.transaction.entries[0].state = EntryState::RollingBack;
    journal::write_journal(&record.journal_dir, &record.transaction).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        fs::symlink_metadata(&entry.destination_path)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let app = deliver(record, false);
    assert_incomplete(&app);
    assert!(app.repair_result.as_deref().unwrap().contains("uncertain"));
    assert!(
        app.activity
            .jobs
            .values()
            .next()
            .unwrap()
            .technical
            .contains("Permission denied")
    );
    assert_eq!(fx.persisted().entries[0].state, EntryState::RollingBack);
    drop(guard);
    assert_eq!(
        fs::read(&entry.destination_path).unwrap(),
        b"synthetic duplicate progress"
    );
    let retry = deliver(app.repair_history[0].clone(), false);
    assert_eq!(
        history(&retry).undo,
        super::super::history_view::UndoStatus::AlreadyUndone
    );
    assert!(fx.persisted().is_fully_rolled_back());
}

#[test]
fn contradictory_terminal_receipt_is_never_shown_as_restored_or_rewritten() {
    use super::super::history_view::{Outcome, UndoStatus};
    let fx = Fixture::new(1);
    let mut record = fx.record.clone();
    record.transaction.state = TransactionState::RolledBack;
    record.transaction.entries[0].rolled_back_at_unix = Some(42);
    journal::write_journal(&record.journal_dir, &record.transaction).unwrap();
    let path =
        journal::journal_path(&record.journal_dir, &record.transaction.transaction_id).unwrap();
    let before = fs::read(&path).unwrap();
    let app = deliver(record, false);
    let projected = history(&app);
    assert_eq!(projected.outcome, Outcome::NeedsAttention);
    assert!(matches!(projected.undo, UndoStatus::NeedsReview(_)));
    assert_eq!(projected.undone_when, None);
    assert!(!equivalent_panel_text(&app.repair_history[0]).contains("Undone —"));
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Failed
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    let entry = &fx.record.transaction.entries[0];
    assert!(!entry.source_path.exists());
    assert_eq!(
        fs::read(&entry.destination_path).unwrap(),
        b"synthetic duplicate progress"
    );
}

#[test]
fn inconsistent_nested_success_cannot_mark_an_applied_receipt_restored() {
    use archivefs_core::dat::rename_apply::{RollbackOutcome, model::RollbackResult};
    let fx = Fixture::new(1);
    let result = super::super::duplicate_undo::DuplicateUndoResult {
        generation: 1,
        record: fx.record.clone(),
        outcome: Ok(archivefs_core::repair::QuarantineRollbackOutcome {
            rollback: RollbackOutcome {
                result: RollbackResult::FullyRolledBack,
                transaction: fx.record.transaction.clone(),
            },
            directories_removed: vec![],
            directories_remaining: vec![],
        }),
    };
    let app = deliver_payload(
        fx.record.clone(),
        Ok(Payload::DuplicateUndone(Box::new(result))),
    );
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Failed
    );
    assert!(
        app.repair_result
            .as_deref()
            .unwrap()
            .contains("inconsistent")
    );
    assert_eq!(
        history(&app).undo,
        super::super::history_view::UndoStatus::Available
    );
    assert_eq!(fx.persisted(), fx.record.transaction);
}

#[test]
fn incomplete_history_load_preserves_existing_quarantine_receipts() {
    let fx = Fixture::new(1);
    let dir = journal::default_rename_transaction_dir().unwrap();
    journal::write_journal(&dir, &fx.record.transaction).unwrap();
    let corrupt = dir.join("duplicate-undo-corrupt.json");
    fs::write(&corrupt, b"{truncated").unwrap();
    let (answers, _rx) = mpsc::channel();
    let outcome = execute(1, Command::LoadRepairHistory, &answers);
    assert!(matches!(&outcome, Err(error) if error.contains("History could not be fully read")));
    let context = egui::Context::default();
    let mut app = super::super::tests::fixture(&context);
    let (tx, _requests) = mpsc::channel();
    let (answers, rx) = mpsc::channel();
    app.backend = Backend { tx, rx };
    app.repair_history.push(fx.record.clone());
    let id = app.activity.queue(
        "Reading History",
        super::super::routes::Route::Section(super::super::routes::Section::History),
        false,
    );
    answers.send(Event::Finished { id, outcome }).unwrap();
    app.poll(&context);
    assert_eq!(app.repair_history[0].transaction, fx.record.transaction);
    assert_eq!(
        app.activity.jobs[&id].phase,
        super::super::activity::Phase::Failed
    );
    assert_eq!(fs::read(&corrupt).unwrap(), b"{truncated");
    fs::remove_file(corrupt).unwrap(); // Only this test's disposable corrupt fixture.
}

#[test]
fn terminal_readback_after_failed_acknowledgement_remains_unconfirmed() {
    use super::super::history_view::{Outcome, UndoStatus};
    let fx = Fixture::new(1);
    let mut result =
        super::super::duplicate_undo::undo(fx.record.clone(), &AtomicBool::new(false), 1);
    assert!(result.record.transaction.is_fully_rolled_back());
    let persisted = fx.persisted();
    assert!(persisted.is_fully_rolled_back());
    // Inject the outer error after publication: readback alone cannot acknowledge
    // a journal write/sync error. The real terminal receipt is retained on disk.
    result.outcome = Err("injected journal publication acknowledgement failure".into());
    let app = deliver_payload(
        fx.record.clone(),
        Ok(Payload::DuplicateUndone(Box::new(result))),
    );
    assert_eq!(
        app.activity.jobs.values().next().unwrap().phase,
        super::super::activity::Phase::Failed
    );
    assert_eq!(history(&app).outcome, Outcome::NeedsAttention);
    assert_ne!(history(&app).undo, UndoStatus::AlreadyUndone);
    assert_eq!(history(&app).undone_when, None);
    assert!(!equivalent_panel_text(&app.repair_history[0]).contains("Undone —"));
    assert!(equivalent_panel_text(&app.repair_history[0]).contains("completion is unconfirmed"));
    assert_eq!(
        fx.persisted(),
        persisted,
        "presentation must not rewrite authority"
    );
    for entry in &persisted.entries {
        assert_eq!(
            fs::read(&entry.source_path).unwrap(),
            b"synthetic duplicate progress"
        );
        assert!(!entry.destination_path.exists());
    }
}
