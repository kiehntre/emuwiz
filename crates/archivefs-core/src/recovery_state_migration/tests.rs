use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::dat::rename_apply::model::{
    EntryState, ExactResumeEnvelope, ExactResumeOperation, ObjectIdentity, ObjectKind,
    RenameTransaction, TransactionOperation, TransactionState,
};
use crate::source_root_migration::MigrationClassification;

fn identity() -> ObjectIdentity {
    ObjectIdentity {
        size_bytes: 1,
        modified_unix: 1,
        kind: ObjectKind::RegularFile,
        #[cfg(unix)]
        ino: 1,
        #[cfg(unix)]
        dev: 1,
    }
}

fn transaction(
    state: TransactionState,
    entry_state: EntryState,
    source: &Path,
    dest: &Path,
) -> RenameTransaction {
    RenameTransaction {
        transaction_id: "tx-1".into(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 1,
        source_scan_root: source.parent().unwrap().display().to_string(),
        state,
        entries: vec![crate::dat::rename_apply::model::TransactionEntry {
            source_path: source.to_path_buf(),
            destination_path: dest.to_path_buf(),
            original_basename: "a.rom".into(),
            proposed_basename: "a.rom".into(),
            identity: identity(),
            operation: TransactionOperation::RenameMove,
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state: entry_state,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: BTreeMap::new(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: BTreeMap::new(),
    }
}

#[test]
fn pending_source_rebases_exact_suffix_when_new_file_exists() {
    let old = tempfile::tempdir().unwrap();
    let new = tempfile::tempdir().unwrap();
    let source = old.path().join("roms/a.rom");
    let candidate = new.path().join("roms/a.rom");
    fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    fs::write(&candidate, b"x").unwrap();
    let report = plan_recovery_state_migration(
        old.path(),
        new.path(),
        &[transaction(
            TransactionState::Planned,
            EntryState::Planned,
            &source,
            &old.path().join("library/a.rom"),
        )],
    );
    assert_eq!(report.totals.source_candidates, 1);
    assert_eq!(
        report.source_root_plan.migration.proposals[0]
            .candidate_path
            .as_deref(),
        Some(candidate.as_path())
    );
}

#[test]
fn already_current_pending_path_is_not_rebased_again() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("roms/a.rom");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, b"x").unwrap();
    let report = plan_recovery_state_migration(
        root.path(),
        root.path(),
        &[transaction(
            TransactionState::Planned,
            EntryState::Planned,
            &source,
            &root.path().join("library/a.rom"),
        )],
    );
    assert_eq!(report.totals.source_already_current, 1);
}

#[test]
fn settled_and_consistent_rolled_back_are_historical() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("old/a.rom");
    let report = plan_recovery_state_migration(
        t.path().join("old"),
        t.path().join("new"),
        &[transaction(
            TransactionState::RolledBack,
            EntryState::RolledBack,
            &source,
            &t.path().join("dest/a.rom"),
        )],
    );
    assert_eq!(report.totals.settled_historical, 1);
    assert!(
        report
            .references
            .iter()
            .all(|r| r.disposition == RecoveryMigrationDisposition::HistoricalOnly)
    );
}

#[test]
fn apply_failed_rolling_back_and_rollback_failed_remain_live() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("old/a.rom");
    let transactions = [
        transaction(
            TransactionState::ApplyFailed,
            EntryState::ApplyFailed,
            &source,
            &t.path().join("dest/a.rom"),
        ),
        transaction(
            TransactionState::RollingBack,
            EntryState::RollingBack,
            &source,
            &t.path().join("dest/a.rom"),
        ),
        transaction(
            TransactionState::RollbackFailed,
            EntryState::RollbackFailed,
            &source,
            &t.path().join("dest/a.rom"),
        ),
    ];
    let report =
        plan_recovery_state_migration(t.path().join("old"), t.path().join("new"), &transactions);
    assert_eq!(report.totals.live_actionable, 3);
}

#[test]
fn applied_entry_is_rollback_review_not_destination_rebase() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("old/a.rom");
    let report = plan_recovery_state_migration(
        t.path().join("old"),
        t.path().join("new"),
        &[transaction(
            TransactionState::Applied,
            EntryState::Applied,
            &source,
            &t.path().join("library/a.rom"),
        )],
    );
    assert!(
        report
            .references
            .iter()
            .any(|r| r.disposition
                == RecoveryMigrationDisposition::RollbackPathChangedReviewRequired)
    );
}

#[test]
fn destination_and_external_paths_are_never_blindly_rebased() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("old/a.rom");
    let dest = t.path().join("old/library/a.rom");
    let external = PathBuf::from("/tmp/external/a.rom");
    let mut tx = transaction(
        TransactionState::Planned,
        EntryState::Planned,
        &source,
        &dest,
    );
    tx.entries[0].operation = TransactionOperation::CreateSymlink {
        expected_target: external.clone(),
        destination_root: t.path().join("old/library"),
    };
    let report = plan_recovery_state_migration(t.path().join("old"), t.path().join("new"), &[tx]);
    assert!(
        report
            .references
            .iter()
            .any(|r| r.role == RecoveryMigrationRole::LiveDestination
                && r.disposition == RecoveryMigrationDisposition::DestinationOwned)
    );
    assert!(
        report
            .references
            .iter()
            .any(|r| r.role == RecoveryMigrationRole::SymlinkExpectedTarget
                && r.disposition == RecoveryMigrationDisposition::External)
    );
}

#[test]
fn contradictory_rolled_back_state_is_technical_review() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("old/a.rom");
    let tx = transaction(
        TransactionState::RolledBack,
        EntryState::ApplyFailed,
        &source,
        &t.path().join("dest/a.rom"),
    );
    let report = plan_recovery_state_migration(t.path().join("old"), t.path().join("new"), &[tx]);
    assert_eq!(report.totals.invalid_technical_review, 1);
    assert!(
        report
            .references
            .iter()
            .all(|r| r.disposition == RecoveryMigrationDisposition::InvalidTechnicalReview)
    );
}

#[test]
fn exact_resume_paths_remain_historical_approval_bound() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("old/a.rom");
    let mut tx = transaction(
        TransactionState::Planned,
        EntryState::Planned,
        &source,
        &t.path().join("dest/a.rom"),
    );
    let envelope = ExactResumeEnvelope {
        format_version: 1,
        transaction_id: "tx-1".into(),
        approved_generation: 1,
        classifier_version: "v".into(),
        plan_digest: "digest".into(),
        source_scan_root: t.path().join("old").display().to_string(),
        approved_source_paths: vec![source.display().to_string()],
        operations: vec![ExactResumeOperation {
            index: 0,
            source_path: source.clone(),
            destination_path: t.path().join("dest/a.rom"),
            operation: TransactionOperation::RenameMove,
            identity: identity(),
            original_basename: "a.rom".into(),
            proposed_basename: "a.rom".into(),
        }],
        created_at_unix: 1,
    };
    tx.unknown.insert(
        EXACT_RESUME_ENVELOPE_KEY.into(),
        serde_json::to_value(envelope).unwrap(),
    );
    let report = plan_recovery_state_migration(t.path().join("old"), t.path().join("new"), &[tx]);
    assert_eq!(report.totals.exact_resume, 1);
    assert!(
        report
            .exact_resume
            .iter()
            .all(|p| p.classification == MigrationClassification::HistoricalOnly)
    );
}

#[test]
fn symlink_escape_and_missing_old_root_fail_closed() {
    let t = tempfile::tempdir().unwrap();
    let old = t.path().join("missing-old");
    let new = t.path().join("new");
    fs::create_dir_all(&new).unwrap();
    let source = old.join("../escape/a.rom");
    let report = plan_recovery_state_migration(
        old,
        new,
        &[transaction(
            TransactionState::Planned,
            EntryState::Planned,
            &source,
            &t.path().join("dest/a.rom"),
        )],
    );
    assert_ne!(report.totals.source_candidates, 1);
}

#[test]
fn planning_is_deterministic_and_does_not_write_journals() {
    let t = tempfile::tempdir().unwrap();
    let old = t.path().join("old");
    let new = t.path().join("new");
    fs::create_dir_all(new.join("roms")).unwrap();
    fs::write(new.join("roms/a.rom"), b"x").unwrap();
    let tx = transaction(
        TransactionState::Planned,
        EntryState::Planned,
        &old.join("roms/a.rom"),
        &old.join("dest/a.rom"),
    );
    let first = plan_recovery_state_migration(&old, &new, std::slice::from_ref(&tx));
    let second = plan_recovery_state_migration(&old, &new, std::slice::from_ref(&tx));
    assert_eq!(first, second);
    assert!(
        t.path()
            .join("rename-transactions")
            .symlink_metadata()
            .is_err()
    );
}

#[test]
fn legacy_journal_without_envelope_is_not_upgraded() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("old/a.rom");
    let tx = transaction(
        TransactionState::ApplyFailed,
        EntryState::ApplyFailed,
        &source,
        &t.path().join("dest/a.rom"),
    );
    let report = plan_recovery_state_migration(t.path().join("old"), t.path().join("new"), &[tx]);
    assert_eq!(report.totals.legacy, 1);
    assert_eq!(report.totals.exact_resume, 0);
}

#[test]
fn exact_resume_state_parser_is_read_only() {
    let tx = transaction(
        TransactionState::Planned,
        EntryState::Planned,
        Path::new("/old/a"),
        Path::new("/dest/a"),
    );
    assert!(
        crate::dat::rename_apply::exact_resume::exact_resume_state(&tx)
            .unwrap()
            .is_none()
    );
}
