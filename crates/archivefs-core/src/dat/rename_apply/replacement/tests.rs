use super::*;
use crate::dat::rename_apply::{
    executor::*, journal::*, model::*, preflight::DirectoryPolicy, rollback::*,
};
use crate::safe_read::TrustedRoots;
use std::{collections::BTreeSet, fs, sync::atomic::AtomicBool};

struct Fixture {
    root: tempfile::TempDir,
    tx: RenameTransaction,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("stage")).unwrap();
        let source = root.path().join("stage/new.zip");
        let destination = root.path().join("parent.zip");
        fs::write(&source, b"verified replacement").unwrap();
        fs::write(&destination, b"exact original").unwrap();
        let entry = TransactionEntry {
            identity: capture_identity(&source).unwrap(),
            operation: TransactionOperation::ReplaceExisting {
                original_identity: capture_identity(&destination).unwrap(),
                destination_root: root.path().to_owned(),
            },
            source_path: source,
            destination_path: destination,
            original_basename: "new.zip".into(),
            proposed_basename: "parent.zip".into(),
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        };
        let tx = RenameTransaction {
            transaction_id: new_transaction_id(1234),
            plan_generation: 1,
            classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.into()),
            created_at_unix: 1234,
            source_scan_root: root.path().to_string_lossy().into_owned(),
            state: TransactionState::Planned,
            entries: vec![entry],
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        };
        Self { root, tx }
    }
    fn journal(&self) -> std::path::PathBuf {
        self.root.path().join("journal")
    }
    fn run(&mut self) -> Result<ApplyOutcome, ApplyError> {
        let journal = self.journal();
        let approved_paths = BTreeSet::from([self.tx.entries[0]
            .source_path
            .to_string_lossy()
            .into_owned()]);
        apply_transaction(&mut ApplyExecution {
            transaction: &mut self.tx,
            approved_paths,
            current_generation: 1,
            trusted: TrustedRoots::from_paths([self.root.path()]),
            journal_dir: journal,
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
    }
    fn undo(&mut self) -> RollbackOutcome {
        let journal = self.journal();
        rollback_transaction_confined(
            &mut self.tx,
            &journal,
            &AtomicBool::new(false),
            &TrustedRoots::from_paths([self.root.path()]),
        )
        .unwrap()
    }
}

#[test]
fn replacement_and_restart_undo_preserve_exact_original_and_repeated_undo_is_safe() {
    let mut f = Fixture::new();
    assert_eq!(
        f.run().unwrap().transaction.state,
        TransactionState::Applied
    );
    assert_eq!(
        fs::read(&f.tx.entries[0].destination_path).unwrap(),
        b"verified replacement"
    );
    assert_eq!(
        fs::read(&f.tx.entries[0].source_path).unwrap(),
        b"exact original"
    );
    f.tx = read_journal(&journal_path(&f.journal(), &f.tx.transaction_id).unwrap()).unwrap();
    assert_eq!(f.undo().transaction.state, TransactionState::RolledBack);
    assert_eq!(
        fs::read(&f.tx.entries[0].destination_path).unwrap(),
        b"exact original"
    );
    assert_eq!(f.undo().transaction.state, TransactionState::RolledBack);
}

#[test]
fn target_or_staged_source_changed_since_review_is_refused() {
    for source in [false, true] {
        let mut f = Fixture::new();
        let path = if source {
            &f.tx.entries[0].source_path
        } else {
            &f.tx.entries[0].destination_path
        };
        fs::write(path, b"unreviewed change").unwrap();
        assert!(f.run().is_err());
        assert_eq!(
            fs::read(&f.tx.entries[0].destination_path).unwrap(),
            if source {
                b"exact original".as_slice()
            } else {
                b"unreviewed change"
            }
        );
    }
}

#[test]
fn replacement_requires_explicit_approved_source() {
    let mut f = Fixture::new();
    let journal_dir = f.journal();
    let result = apply_transaction(&mut ApplyExecution {
        transaction: &mut f.tx,
        approved_paths: BTreeSet::new(),
        current_generation: 1,
        trusted: TrustedRoots::from_paths([f.root.path()]),
        journal_dir,
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &AtomicBool::new(false),
        directory_policy: DirectoryPolicy::SameFilesystem,
        allow_symlink_source: false,
    });
    assert!(result.is_err() || result.unwrap().transaction.applied_count() == 0);
    assert_eq!(
        fs::read(&f.tx.entries[0].destination_path).unwrap(),
        b"exact original"
    );
}

#[test]
fn failed_journal_before_exchange_leaves_both_files_unchanged() {
    let mut f = Fixture::new();
    fs::write(f.journal(), b"not a directory").unwrap();
    assert!(f.run().is_err());
    assert_eq!(
        fs::read(&f.tx.entries[0].destination_path).unwrap(),
        b"exact original"
    );
    assert_eq!(
        fs::read(&f.tx.entries[0].source_path).unwrap(),
        b"verified replacement"
    );
}

#[test]
fn failure_before_exchange_does_not_publish() {
    let mut f = Fixture::new();
    FAULT.with(|p| p.set(Some("before_exchange")));
    let result = f.run();
    FAULT.with(|p| p.set(None));
    assert_eq!(
        result.unwrap().transaction.state,
        TransactionState::ApplyFailed
    );
    assert_eq!(
        fs::read(&f.tx.entries[0].destination_path).unwrap(),
        b"exact original"
    );
}

#[test]
fn post_exchange_failure_remains_durably_recoverable_not_success() {
    let mut f = Fixture::new();
    FAULT.with(|p| p.set(Some("after_exchange")));
    let result = f.run();
    FAULT.with(|p| p.set(None));
    assert_eq!(
        result.unwrap().transaction.state,
        TransactionState::ApplyFailed
    );
    assert_eq!(f.tx.entries[0].state, EntryState::Applying);
    assert_eq!(
        fs::read(&f.tx.entries[0].source_path).unwrap(),
        b"exact original"
    );
    f.tx = read_journal(&journal_path(&f.journal(), &f.tx.transaction_id).unwrap()).unwrap();
    assert_eq!(f.undo().transaction.state, TransactionState::RolledBack);
    assert_eq!(
        fs::read(&f.tx.entries[0].destination_path).unwrap(),
        b"exact original"
    );
}

#[test]
fn interrupted_exchange_and_undo_are_reconciled_from_both_identities() {
    let mut f = Fixture::new();
    f.tx.state = TransactionState::Applying;
    f.tx.entries[0].state = EntryState::Applying;
    write_journal(&f.journal(), &f.tx).unwrap();
    apply(&f.tx.entries[0]).unwrap();
    let journal = f.journal();
    super::super::reconcile::reconcile_recovery(&mut f.tx, &journal).unwrap();
    assert_eq!(f.tx.entries[0].state, EntryState::Applied);
    f.tx.state = TransactionState::RollingBack;
    f.tx.entries[0].state = EntryState::RollingBack;
    write_journal(&f.journal(), &f.tx).unwrap();
    undo(&f.tx.entries[0]).unwrap();
    let journal = f.journal();
    super::super::reconcile::reconcile_recovery(&mut f.tx, &journal).unwrap();
    assert_eq!(f.tx.entries[0].state, EntryState::RolledBack);
}

#[test]
fn changed_live_target_or_original_backup_blocks_undo() {
    for source in [false, true] {
        let mut f = Fixture::new();
        f.run().unwrap();
        let changed = if source {
            f.tx.entries[0].source_path.clone()
        } else {
            f.tx.entries[0].destination_path.clone()
        };
        fs::write(&changed, b"user progress").unwrap();
        assert_ne!(f.undo().transaction.state, TransactionState::RolledBack);
        assert_eq!(fs::read(changed).unwrap(), b"user progress");
    }
}

#[test]
fn symlink_hardlink_and_wrong_authority_are_refused() {
    for case in 0..3 {
        let mut f = Fixture::new();
        match case {
            0 => {
                fs::remove_file(&f.tx.entries[0].destination_path).unwrap();
                std::os::unix::fs::symlink(
                    &f.tx.entries[0].source_path,
                    &f.tx.entries[0].destination_path,
                )
                .unwrap();
            }
            1 => {
                fs::hard_link(
                    &f.tx.entries[0].destination_path,
                    f.root.path().join("alias"),
                )
                .unwrap();
            }
            _ => {
                if let TransactionOperation::ReplaceExisting {
                    destination_root, ..
                } = &mut f.tx.entries[0].operation
                {
                    *destination_root = f.root.path().join("unrelated");
                }
            }
        }
        assert!(f.run().is_err());
    }
}

#[test]
fn confined_undo_refuses_roots_that_no_longer_authorize_the_paths() {
    let mut f = Fixture::new();
    f.run().unwrap();
    let journal = f.journal();
    let unrelated = tempfile::tempdir().unwrap();
    let result = rollback_transaction_confined(
        &mut f.tx,
        &journal,
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([unrelated.path()]),
    )
    .unwrap();
    assert_ne!(result.transaction.state, TransactionState::RolledBack);
    assert_eq!(
        fs::read(&f.tx.entries[0].destination_path).unwrap(),
        b"verified replacement"
    );
    assert_eq!(
        fs::read(&f.tx.entries[0].source_path).unwrap(),
        b"exact original"
    );
}

#[test]
fn failed_exchange_syscall_leaves_original_available() {
    let f = Fixture::new();
    assert!(
        super::super::noclobber::exchange(
            &f.root.path().join("absent-stage"),
            &f.tx.entries[0].destination_path,
        )
        .is_err()
    );
    assert_eq!(
        fs::read(&f.tx.entries[0].destination_path).unwrap(),
        b"exact original"
    );
    assert_eq!(
        fs::read(&f.tx.entries[0].source_path).unwrap(),
        b"verified replacement"
    );
}
