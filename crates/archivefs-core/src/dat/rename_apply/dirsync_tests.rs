//! Deterministic synthetic fault-injection tests for directory durability.
//! Panics in the hook simulate a process death at that point; they are NOT
//! power-loss simulations and prove nothing about device or kernel behaviour.
use super::dirsync::{DirSyncEvent, set_test_hook};
use super::*;
use std::cell::RefCell;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

type Log = Rc<RefCell<Vec<String>>>;

struct Fx {
    _dir: tempfile::TempDir,
    roms: PathBuf,
    sub: PathBuf,
    journal: PathBuf,
    tx: RenameTransaction,
    approved: BTreeSet<String>,
}

fn fx(names: &[&str], cross: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    let sub = roms.join("sub");
    let journal = dir.path().join("journal");
    for d in [&sub, &journal] {
        std::fs::create_dir_all(d).unwrap();
    }
    let sources: Vec<PathBuf> = names.iter().map(|n| write(&roms, n)).collect();
    let proposals = sources
        .iter()
        .zip(names)
        .map(|(s, n)| {
            proposal(
                s.to_str().unwrap(),
                n,
                &n.to_uppercase().replace(".BIN", ".bin"),
                ProposalState::Suggested,
            )
        })
        .collect();
    let plan = plan(proposals, 1, &roms);
    let refs: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
    let approved = approved_of(&refs);
    let mut tx = build_transaction(&plan, &approved, 1).unwrap();
    if cross {
        for e in &mut tx.entries {
            e.destination_path = sub.join(&e.proposed_basename);
        }
    }
    Fx {
        _dir: dir,
        roms,
        sub,
        journal,
        tx,
        approved,
    }
}

fn run(f: &Fx) -> Result<ApplyOutcome, ApplyError> {
    let mut tx = f.tx.clone();
    apply_transaction(&mut ApplyExecution {
        transaction: &mut tx,
        approved_paths: f.approved.clone(),
        current_generation: 1,
        trusted: TrustedRoots::from_paths([&f.roms]),
        journal_dir: f.journal.clone(),
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &no_cancel(),
        directory_policy: super::preflight::DirectoryPolicy::SameFilesystem,
        allow_symlink_source: false,
    })
}

fn persisted(f: &Fx) -> RenameTransaction {
    read_journal(&journal_path(&f.journal, &f.tx.transaction_id).unwrap()).unwrap()
}

/// Installs a hook that logs `before:<dir>`/`after:<dir>` plus the persisted
/// entry-0 state at that instant, and fails or "crashes" as directed.
fn install(f: &Fx, fail_on: Option<PathBuf>, crash: Option<(bool, PathBuf)>) -> Log {
    let log: Log = Rc::default();
    let l = log.clone();
    let journal = journal_path(&f.journal, &f.tx.transaction_id).unwrap();
    let roms = f.roms.clone();
    set_test_hook(Some(Box::new(move |event| {
        let state = read_journal(&journal)
            .map(|t| format!("{:?}", t.entries[0].state))
            .unwrap_or_else(|_| "none".into());
        let name = |p: &Path| {
            if p == roms {
                "roms".to_string()
            } else {
                "sub".to_string()
            }
        };
        let (kind, path) = match event {
            DirSyncEvent::Before(p) => ("before", p),
            DirSyncEvent::After(p) => ("after", p),
        };
        l.borrow_mut()
            .push(format!("{kind}:{}:{state}", name(path)));
        if let Some((on_after, p)) = &crash
            && *on_after == (kind == "after")
            && p == path
        {
            panic!("simulated process death");
        }
        if kind == "before" && fail_on.as_deref() == Some(path) {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    })));
    log
}
fn clear() {
    set_test_hook(None);
}

#[test]
fn same_directory_rename_syncs_one_parent_between_intent_and_completion() {
    let f = fx(&["a.bin"], false);
    let log = install(&f, None, None);
    let out = run(&f).unwrap();
    clear();
    // Intent (`Applying`) persisted and rename done before the sync; success
    // persisted only after the sync returned.
    assert_eq!(
        *log.borrow(),
        ["before:roms:Applying", "after:roms:Applying"]
    );
    assert_eq!(out.transaction.entries[0].state, EntryState::Applied);
    assert_eq!(persisted(&f).entries[0].state, EntryState::Applied);
    assert!(f.roms.join("A.bin").exists() && !f.roms.join("a.bin").exists());
}

#[test]
fn cross_directory_rename_syncs_destination_then_source_parent() {
    let f = fx(&["a.bin"], true);
    let log = install(&f, None, None);
    run(&f).unwrap();
    clear();
    assert_eq!(
        *log.borrow(),
        [
            "before:sub:Applying",
            "after:sub:Applying",
            "before:roms:Applying",
            "after:roms:Applying"
        ]
    );
    assert!(f.sub.join("A.bin").exists());
    assert_eq!(persisted(&f).entries[0].state, EntryState::Applied);
}

fn assert_in_flight_not_success(f: &Fx, out: &ApplyOutcome, dest: &Path, second: &Path) {
    assert_eq!(out.transaction.state, TransactionState::ApplyFailed);
    assert_eq!(out.transaction.entries[0].state, EntryState::Applying);
    assert_eq!(out.summary.applied, 0);
    assert_eq!(out.summary.failed, 0);
    assert_eq!(
        out.summary.unresolved, 1,
        "in-flight rename stays in the totals"
    );
    assert_eq!(out.summary.requested, 2);
    let p = persisted(f);
    assert_eq!(p.entries[0].state, EntryState::Applying, "never Applied");
    assert!(
        p.entries[0]
            .failure_reason
            .as_deref()
            .unwrap()
            .contains("synchronised")
    );
    assert!(
        dest.exists(),
        "the rename happened and is reported as possibly done"
    );
    assert!(!f.roms.join("a.bin").exists());
    assert!(second.exists(), "later entries are not attempted");
    assert_eq!(p.entries[1].state, EntryState::Planned);
}

#[test]
fn destination_parent_sync_failure_never_records_completion() {
    for cross in [false, true] {
        let f = fx(&["a.bin", "b.bin"], cross);
        let parent = if cross { f.sub.clone() } else { f.roms.clone() };
        let log = install(&f, Some(parent.clone()), None);
        let out = run(&f).unwrap();
        clear();
        assert_in_flight_not_success(&f, &out, &parent.join("A.bin"), &f.roms.join("b.bin"));
        if cross {
            // Both directories are still attempted for durability.
            assert!(log.borrow().iter().any(|e| e.starts_with("before:roms")));
        }
    }
}

#[test]
fn source_parent_sync_failure_never_records_completion() {
    let f = fx(&["a.bin", "b.bin"], true);
    let log = install(&f, Some(f.roms.clone()), None);
    let out = run(&f).unwrap();
    clear();
    assert_in_flight_not_success(&f, &out, &f.sub.join("A.bin"), &f.roms.join("b.bin"));
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|e| e.starts_with("before"))
            .count(),
        2
    );
}

#[test]
fn a_failed_sync_is_not_retried_and_is_not_confirmed_by_a_second_apply() {
    let f = fx(&["a.bin"], false);
    install(&f, Some(f.roms.clone()), None);
    let _ = run(&f).unwrap();
    clear();
    // The same transaction cannot be applied again: the source is gone and
    // nothing is renamed twice.
    assert!(run(&f).is_err(), "a second apply must be refused");
    assert_eq!(persisted(&f).entries[0].state, EntryState::Applying);
    assert!(f.roms.join("A.bin").exists());
}

#[test]
fn death_between_rename_and_sync_leaves_applying_and_recovers_from_the_filesystem() {
    let f = fx(&["a.bin"], false);
    install(&f, None, Some((false, f.roms.clone())));
    let r = catch_unwind(AssertUnwindSafe(|| run(&f)));
    clear();
    assert!(r.is_err());
    let mut p = persisted(&f);
    assert_eq!(p.entries[0].state, EntryState::Applying);
    assert_eq!(p.state, TransactionState::Applying);
    assert!(f.roms.join("A.bin").exists() && !f.roms.join("a.bin").exists());
    // Recovery confirms from the filesystem (sync succeeds) and never renames.
    let issues = reconcile_recovery(&mut p, &f.journal).unwrap();
    assert_eq!(issues[0].kind, RecoveryIssueKind::RenameConfirmed);
    assert_eq!(persisted(&f).entries[0].state, EntryState::Applied);
    assert!(f.roms.join("A.bin").exists());
}

#[test]
fn death_between_sync_and_completion_checkpoint_recovers() {
    let f = fx(&["a.bin"], false);
    let log = install(&f, None, Some((true, f.roms.clone())));
    let r = catch_unwind(AssertUnwindSafe(|| run(&f)));
    clear();
    assert!(r.is_err());
    assert_eq!(
        *log.borrow(),
        ["before:roms:Applying", "after:roms:Applying"]
    );
    let mut p = persisted(&f);
    assert_eq!(
        p.entries[0].state,
        EntryState::Applying,
        "completion not yet recorded"
    );
    let issues = reconcile_recovery(&mut p, &f.journal).unwrap();
    assert_eq!(issues[0].kind, RecoveryIssueKind::RenameConfirmed);
    assert_eq!(persisted(&f).entries[0].state, EntryState::Applied);
}

#[test]
fn recovery_does_not_promote_when_the_sync_still_fails_and_blocks_rollback() {
    let f = fx(&["a.bin"], false);
    install(&f, None, Some((false, f.roms.clone())));
    let _ = catch_unwind(AssertUnwindSafe(|| run(&f)));
    clear();
    let mut p = persisted(&f);
    let before = std::fs::read(journal_path(&f.journal, &p.transaction_id).unwrap()).unwrap();
    install(&f, Some(f.roms.clone()), None);
    let issues = reconcile_recovery(&mut p, &f.journal).unwrap();
    assert_eq!(issues[0].kind, RecoveryIssueKind::DirectorySyncFailed);
    assert_eq!(p.entries[0].state, EntryState::Applying);
    assert_eq!(
        std::fs::read(journal_path(&f.journal, &p.transaction_id).unwrap()).unwrap(),
        before,
        "nothing promoted or persisted"
    );
    assert_eq!(
        crate::platform_evidence_fusion::plan_transaction::assess_recovery(&p, &issues),
        crate::platform_evidence_fusion::plan_transaction::RecoveryAssessment::ManualRecoveryRequired
    );
    let mut tx = p.clone();
    let out = rollback_transaction(&mut tx, &f.journal, &no_cancel()).unwrap();
    clear();
    assert_ne!(out.result, RollbackResult::FullyRolledBack);
    assert!(f.roms.join("A.bin").exists() && !f.roms.join("a.bin").exists());
}

#[test]
fn rollback_sync_failure_is_in_flight_then_resolves_without_a_second_rename() {
    let f = fx(&["a.bin"], false);
    let out = run(&f).unwrap();
    let mut tx = out.transaction;
    // Ordering on the happy path first: RollingBack is durable before the sync.
    let log = install(&f, Some(f.roms.clone()), None);
    let r = rollback_transaction(&mut tx, &f.journal, &no_cancel()).unwrap();
    assert_eq!(*log.borrow(), ["before:roms:RollingBack"]);
    assert!(matches!(r.result, RollbackResult::RollbackFailed { .. }));
    assert_eq!(r.transaction.entries[0].state, EntryState::RollingBack);
    assert_ne!(r.transaction.state, TransactionState::RolledBack);
    assert_eq!(persisted(&f).entries[0].state, EntryState::RollingBack);
    assert!(f.roms.join("a.bin").exists() && !f.roms.join("A.bin").exists());
    // Still failing: stays unresolved, no mutation.
    let mut again = persisted(&f);
    let r2 = rollback_transaction(&mut again, &f.journal, &no_cancel()).unwrap();
    assert_ne!(r2.result, RollbackResult::FullyRolledBack);
    assert_eq!(
        std::fs::read(f.roms.join("a.bin")).unwrap(),
        b"fixture contents"
    );
    clear();
    // Sync recovers: reconciled from the filesystem to RolledBack.
    let mut healed = persisted(&f);
    let r3 = rollback_transaction(&mut healed, &f.journal, &no_cancel()).unwrap();
    assert_eq!(r3.result, RollbackResult::FullyRolledBack);
    assert_eq!(persisted(&f).state, TransactionState::RolledBack);
}

#[test]
fn rollback_completion_is_recorded_only_after_the_sync_succeeds() {
    let f = fx(&["a.bin"], true);
    let out = run(&f).unwrap();
    let mut tx = out.transaction;
    let log = install(&f, None, None);
    let r = rollback_transaction(&mut tx, &f.journal, &no_cancel()).unwrap();
    clear();
    assert_eq!(r.result, RollbackResult::FullyRolledBack);
    // Reverse rename: source parent (roms) is the destination of the undo.
    assert_eq!(
        *log.borrow(),
        [
            "before:roms:RollingBack",
            "after:roms:RollingBack",
            "before:sub:RollingBack",
            "after:sub:RollingBack"
        ]
    );
    assert_eq!(persisted(&f).entries[0].state, EntryState::RolledBack);
}

#[test]
fn rollback_death_between_rename_and_sync_recovers_via_reconcile() {
    let f = fx(&["a.bin"], false);
    let out = run(&f).unwrap();
    let mut tx = out.transaction;
    install(&f, None, Some((false, f.roms.clone())));
    let r = catch_unwind(AssertUnwindSafe(|| {
        rollback_transaction(&mut tx, &f.journal, &no_cancel())
    }));
    clear();
    assert!(r.is_err());
    let mut p = persisted(&f);
    assert_eq!(p.entries[0].state, EntryState::RollingBack);
    assert!(f.roms.join("a.bin").exists());
    let issues = reconcile_recovery(&mut p, &f.journal).unwrap();
    assert_eq!(issues[0].kind, RecoveryIssueKind::RenameDidNotHappen);
    assert_eq!(persisted(&f).entries[0].state, EntryState::RolledBack);
}

use crate::platform_evidence_fusion::plan_transaction::{RecoveryAssessment, assess_recovery};

#[test]
fn apply_sync_failure_after_a_successful_rename_is_never_safe_to_resume() {
    let f = fx(&["a.bin"], false);
    install(&f, Some(f.roms.clone()), None);
    let out = run(&f).unwrap();
    // The rename really happened, then the sync failed.
    assert!(f.roms.join("A.bin").exists() && !f.roms.join("a.bin").exists());
    assert_eq!(out.summary.unresolved, 1);
    // 1. Straight from the persisted journal, no issues known.
    let p = persisted(&f);
    assert_eq!(
        assess_recovery(&p, &[]),
        RecoveryAssessment::ManualRecoveryRequired,
        "unresolved Applying must never be SAFE_TO_RESUME, even with no issue recorded"
    );
    // 2. After a reconcile that still cannot sync.
    let mut q = p.clone();
    let issues = reconcile_recovery(&mut q, &f.journal).unwrap();
    assert_eq!(issues[0].kind, RecoveryIssueKind::DirectorySyncFailed);
    assert_eq!(
        assess_recovery(&q, &issues),
        RecoveryAssessment::ManualRecoveryRequired
    );
    // 3. The marker kind alone is also refused (belt and braces).
    let mut settled = p.clone();
    settled.entries[0].state = EntryState::Planned;
    let issue = RecoveryIssue {
        entry_index: 0,
        kind: RecoveryIssueKind::DirectorySyncFailed,
        detail: String::new(),
    };
    assert_eq!(
        assess_recovery(&settled, &[issue]),
        RecoveryAssessment::ManualRecoveryRequired
    );
    clear();
    assert!(persisted(&f).entries[0].state != EntryState::Applied);
}

#[test]
fn restart_with_unresolved_applying_and_no_marker_blocks_replay() {
    // A crash (not a sync error) leaves tx Applying / entry Applying and no
    // failure_reason at all. Even a journal rewritten as ApplyFailed must not
    // become resumable while an entry is still in flight.
    let f = fx(&["a.bin"], false);
    install(&f, None, Some((false, f.roms.clone())));
    let _ = catch_unwind(AssertUnwindSafe(|| run(&f)));
    clear();
    let mut p = persisted(&f);
    assert!(p.entries[0].failure_reason.is_none());
    assert_eq!(
        assess_recovery(&p, &[]),
        RecoveryAssessment::ManualRecoveryRequired
    );
    p.state = TransactionState::ApplyFailed;
    assert_eq!(
        assess_recovery(&p, &[]),
        RecoveryAssessment::ManualRecoveryRequired
    );
    p.state = TransactionState::Planned;
    assert_eq!(
        assess_recovery(&p, &[]),
        RecoveryAssessment::ManualRecoveryRequired
    );
}

#[test]
fn rollback_sync_failure_is_counted_and_never_safe_to_resume() {
    let f = fx(&["a.bin"], false);
    let mut tx = run(&f).unwrap().transaction;
    install(&f, Some(f.roms.clone()), None);
    let r = rollback_transaction(&mut tx, &f.journal, &no_cancel()).unwrap();
    clear();
    let summary = TransactionSummary::from_transaction(&r.transaction);
    assert_eq!(summary.unresolved, 1);
    assert_eq!(summary.applied, 0);
    assert_eq!(summary.requested, 1);
    assert_ne!(r.transaction.state, TransactionState::RolledBack);
    let p = persisted(&f);
    assert_eq!(p.entries[0].state, EntryState::RollingBack);
    assert_eq!(
        assess_recovery(&p, &[]),
        RecoveryAssessment::ManualRecoveryRequired
    );
}

#[test]
fn completion_checkpoint_is_never_written_after_a_failed_sync() {
    // Observe every persisted state across the whole run: Applied must never
    // appear before a successful sync, and never at all when the sync fails.
    let f = fx(&["a.bin"], false);
    let seen: Log = Rc::default();
    let s = seen.clone();
    let journal = journal_path(&f.journal, &f.tx.transaction_id).unwrap();
    set_test_hook(Some(Box::new(move |event| {
        if let DirSyncEvent::Before(_) = event {
            s.borrow_mut().push(
                read_journal(&journal)
                    .map(|t| format!("{:?}", t.entries[0].state))
                    .unwrap_or_default(),
            );
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    })));
    let out = run(&f).unwrap();
    clear();
    assert_eq!(*seen.borrow(), ["Applying"]);
    assert_ne!(out.transaction.entries[0].state, EntryState::Applied);
    assert_ne!(persisted(&f).entries[0].state, EntryState::Applied);
    assert_ne!(persisted(&f).state, TransactionState::Applied);
}

// ---------------------------------------------------------------------------
// R3 sync-failure safety: recovery classification and truthful reporting.
// ---------------------------------------------------------------------------

mod sync_safety {
    use super::*;
    use crate::platform_evidence_fusion::plan_transaction::{RecoveryAssessment, assess_recovery};
    use std::os::unix::fs::MetadataExt;

    fn inode(path: &Path) -> (u64, u64) {
        let m = std::fs::metadata(path).unwrap();
        (m.dev(), m.ino())
    }

    /// The recovery verdict must never be SAFE_TO_RESUME while an entry is in flight,
    /// whatever findings were (or were not) persisted.
    fn assert_blocked(t: &RenameTransaction, why: &str) {
        for issues in [&[][..]] {
            assert_eq!(
                assess_recovery(t, issues),
                RecoveryAssessment::ManualRecoveryRequired,
                "{why}: an unresolved Applying entry must block automatic replay"
            );
        }
    }

    #[test]
    fn failed_sync_with_no_recorded_findings_is_never_safe_to_resume() {
        let f = fx(&["a.bin", "b.bin"], false);
        install(&f, Some(f.roms.clone()), None);
        let out = run(&f).unwrap();
        clear();
        assert_eq!(out.transaction.entries[0].state, EntryState::Applying);
        // Transaction-level state is ApplyFailed with zero Applied entries: the
        // shape that used to fall through to SAFE_TO_RESUME.
        assert_eq!(out.transaction.state, TransactionState::ApplyFailed);
        assert_eq!(out.transaction.applied_count(), 0);
        assert_blocked(&out.transaction, "in-memory outcome, no issues");
        // The sync-failure reason was never durably recorded.
        let mut p = persisted(&f);
        p.entries[0].failure_reason = None;
        assert_blocked(&p, "persisted, reason lost");
        // Even a persisted DirectorySyncFailed finding is conservative, not resumable.
        let issue = RecoveryIssue {
            entry_index: 0,
            kind: RecoveryIssueKind::DirectorySyncFailed,
            detail: "x".into(),
        };
        assert_eq!(
            assess_recovery(&p, &[issue]),
            RecoveryAssessment::ManualRecoveryRequired
        );
    }

    #[test]
    fn a_settled_failure_without_in_flight_entries_is_still_resumable() {
        // Guard against over-blocking: a preflight failure leaves nothing in flight.
        let f = fx(&["a.bin"], false);
        let mut tx = f.tx.clone();
        tx.entries[0].state = EntryState::ApplyFailed;
        tx.state = TransactionState::ApplyFailed;
        assert_eq!(assess_recovery(&tx, &[]), RecoveryAssessment::SafeToResume);
    }

    #[test]
    fn crash_points_never_yield_safe_to_resume() {
        // 1+2: immediately after the rename, before any directory fsync.
        let f = fx(&["a.bin"], false);
        install(&f, None, Some((false, f.roms.clone())));
        assert!(catch_unwind(AssertUnwindSafe(|| run(&f))).is_err());
        clear();
        assert_blocked(&persisted(&f), "crash after rename, before fsync");
        assert!(f.roms.join("A.bin").exists() && !f.roms.join("a.bin").exists());

        // 3: after the destination-directory fsync, before the source-directory fsync.
        let f = fx(&["a.bin"], true);
        install(&f, None, Some((true, f.sub.clone())));
        assert!(catch_unwind(AssertUnwindSafe(|| run(&f))).is_err());
        clear();
        let p = persisted(&f);
        assert_eq!(p.entries[0].state, EntryState::Applying);
        assert_blocked(&p, "crash between destination and source fsync");

        // 4: the fsync failed but the journal never recorded the failure: the journal
        // exactly as it stood when the failing fsync was issued is what survives.
        let f = fx(&["a.bin"], false);
        let journal = journal_path(&f.journal, &f.tx.transaction_id).unwrap();
        let snapshot = Rc::new(RefCell::new(Vec::new()));
        {
            let snapshot = snapshot.clone();
            let journal = journal.clone();
            set_test_hook(Some(Box::new(move |event| {
                if let DirSyncEvent::Before(_) = event {
                    *snapshot.borrow_mut() = std::fs::read(&journal).unwrap();
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            })));
        }
        let _ = run(&f).unwrap();
        clear();
        std::fs::write(&journal, &*snapshot.borrow()).unwrap(); // the error marker did not survive
        let p = persisted(&f);
        assert_eq!(p.entries[0].state, EntryState::Applying);
        assert!(p.entries[0].failure_reason.is_none());
        assert_blocked(&p, "failed fsync, error never recorded");

        // 5: fsync succeeded but the completion checkpoint was never written.
        let f = fx(&["a.bin"], false);
        install(&f, None, Some((true, f.roms.clone())));
        assert!(catch_unwind(AssertUnwindSafe(|| run(&f))).is_err());
        clear();
        let mut p = persisted(&f);
        assert_eq!(p.entries[0].state, EntryState::Applying);
        assert_blocked(&p, "fsync ok, completion checkpoint missing");
        // Reconciliation (positive filesystem identity + a successful sync) is what
        // may settle it - never a replay.
        let before = inode(&f.roms.join("A.bin"));
        let issues = reconcile_recovery(&mut p, &f.journal).unwrap();
        assert_eq!(issues[0].kind, RecoveryIssueKind::RenameConfirmed);
        assert_eq!(inode(&f.roms.join("A.bin")), before);
        assert_ne!(
            assess_recovery(&p, &issues),
            RecoveryAssessment::SafeToResume
        );
    }

    #[test]
    fn restart_cannot_repeat_an_unverified_rename_or_record_completion() {
        let f = fx(&["a.bin", "b.bin"], false);
        install(&f, Some(f.roms.clone()), None);
        let _ = run(&f).unwrap();
        clear();
        let renamed = f.roms.join("A.bin");
        let before = inode(&renamed);
        // Restart: reconciliation with the sync still failing leaves it unresolved ...
        let mut p = persisted(&f);
        install(&f, Some(f.roms.clone()), None);
        let issues = reconcile_recovery(&mut p, &f.journal).unwrap();
        clear();
        assert_eq!(issues[0].kind, RecoveryIssueKind::DirectorySyncFailed);
        assert_eq!(p.entries[0].state, EntryState::Applying);
        // ... and replaying the batch neither renames again nor records completion.
        let mut replay = persisted(&f);
        let result = apply_transaction(&mut ApplyExecution {
            transaction: &mut replay,
            approved_paths: f.approved.clone(),
            current_generation: 1,
            trusted: TrustedRoots::from_paths([&f.roms]),
            journal_dir: f.journal.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &no_cancel(),
            directory_policy: super::super::preflight::DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        });
        if let Ok(out) = result {
            assert_ne!(out.transaction.state, TransactionState::Applied);
            assert_eq!(out.summary.applied, 0);
        }
        assert_eq!(inode(&renamed), before, "no second rename");
        let p = persisted(&f);
        assert_ne!(p.state, TransactionState::Applied);
        assert!(p.entries.iter().all(|e| e.state != EntryState::Applied));
    }

    #[test]
    fn summary_reports_the_unresolved_entry_and_reconciles() {
        let f = fx(&["a.bin", "b.bin", "c.bin"], true);
        install(&f, Some(f.sub.clone()), None);
        let out = run(&f).unwrap();
        clear();
        let s = &out.summary;
        assert_eq!(s.requested, 3);
        assert_eq!(s.applied, 0, "never reported as applied");
        assert_eq!(s.failed, 0, "not a failure either: the rename happened");
        assert_eq!(s.unresolved, 1);
        assert_eq!(s.pending, 2, "later entries were never attempted");
        assert_eq!(
            s.accounted(),
            s.requested,
            "no entry is lost or double-counted"
        );
        assert!(!s.fully_resolved());
        let u = &s.unresolved_entries[0];
        assert_eq!(u.state, EntryState::Applying);
        assert_eq!(u.source_path, f.roms.join("a.bin"));
        assert_eq!(u.destination_path, f.sub.join("A.bin"));
        assert!(u.stage.contains("directory"), "stage: {}", u.stage);
        assert!(u.detail.as_deref().unwrap_or("").contains("synchronised"));
        assert!(
            u.required_action.to_lowercase().contains("recover"),
            "{}",
            u.required_action
        );
        // And nothing recorded a successful completion.
        let p = persisted(&f);
        assert_ne!(p.state, TransactionState::Applied);
    }

    #[test]
    fn every_entry_state_is_counted_exactly_once() {
        let f = fx(&["a.bin"], false);
        let states = [
            EntryState::Planned,
            EntryState::PreflightPassed,
            EntryState::Applying,
            EntryState::Applied,
            EntryState::ApplyFailed,
            EntryState::RollingBack,
            EntryState::RolledBack,
            EntryState::RollbackFailed,
            EntryState::Skipped,
        ];
        let mut tx = f.tx.clone();
        let template = tx.entries[0].clone();
        tx.entries = states
            .iter()
            .map(|state| TransactionEntry {
                state: *state,
                ..template.clone()
            })
            .collect();
        let s = TransactionSummary::from_transaction(&tx);
        assert_eq!(s.requested, states.len());
        assert_eq!(s.accounted(), s.requested);
        assert_eq!(
            (
                s.applied,
                s.skipped,
                s.failed,
                s.rolled_back,
                s.pending,
                s.unresolved
            ),
            (1, 1, 1, 1, 2, 3)
        );
        assert_eq!(s.unresolved_entries.len(), 3);
    }
}
