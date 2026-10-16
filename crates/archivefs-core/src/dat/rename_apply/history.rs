//! Durable, non-destructive visibility state for recovery history.
//!
//! Recovery journals remain the authoritative transaction records. This small
//! sidecar only records which stable transaction IDs the user has archived
//! from the active attention surface. It never rewrites a journal, changes a
//! transaction state, or touches a game file.
//!
//! Archive eligibility is deliberately conservative. A record is stale only
//! when the core can prove that no exact-resume evidence and no safe rollback
//! path remain. Missing or ambiguous filesystem evidence is `Uncertain`, not
//! archiveable, because a remount, retry, or manual intervention might make it
//! actionable again.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::exact_resume::{exact_resume_state, has_envelope};
use super::identity::{capture_identity, identity_matches};
use super::model::{EntryState, RenameTransaction, TransactionState};

/// A deliberately extension-less filename so `list_journals` never treats the
/// visibility index as a transaction journal.
pub const RECOVERY_HISTORY_STATE_FILE: &str = "recovery-history-state";

/// Durable visibility-only state. The transaction journals remain untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryHistoryState {
    #[serde(default)]
    pub archived_transaction_ids: BTreeSet<String>,
}

/// Why the active recovery surface may safely stop showing a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleRecoveryReason {
    /// A pre-exact-resume journal has no approval envelope and has no applied
    /// or in-flight entry left that rollback could act on.
    LegacyWithoutExactResume,
    /// Exact resume was explicitly marked permanently unavailable, and no
    /// applied/in-flight entry remains.
    PermanentlyNonResumable,
    /// A rollback attempt is permanently invalid for the recorded object: the
    /// current destination exists but is not the object in the journal.
    RollbackIdentityChanged,
    /// The transaction has no operation left to resume or roll back.
    NoActionableChanges,
}

/// Conservative result used by both GUI surfaces and the archive gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCleanupClassification {
    Actionable,
    Stale(StaleRecoveryReason),
    /// The current evidence is not enough to safely hide this record.
    Uncertain,
}

impl StaleRecoveryReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::LegacyWithoutExactResume => "legacy journal without exact resume evidence",
            Self::PermanentlyNonResumable => "permanently non-resumable transaction",
            Self::RollbackIdentityChanged => "rollback identity no longer matches the disk",
            Self::NoActionableChanges => "no actionable changes remain",
        }
    }
}

impl RecoveryCleanupClassification {
    pub fn is_stale(self) -> bool {
        matches!(self, Self::Stale(_))
    }
}

/// Classifies a journal using only its durable state and read-only filesystem
/// identity checks. Any envelope is retained as actionable because this
/// function cannot prove current plan generation/digest compatibility.
pub fn classify_recovery_cleanup(transaction: &RenameTransaction) -> RecoveryCleanupClassification {
    if super::journal::verify_applied_containment(transaction).is_err() {
        return RecoveryCleanupClassification::Uncertain;
    }
    let has_applied = transaction
        .entries
        .iter()
        .any(|entry| entry.state == EntryState::Applied);
    let has_in_flight = transaction
        .entries
        .iter()
        .any(|entry| matches!(entry.state, EntryState::Applying | EntryState::RollingBack));
    let has_unresolved_entry = transaction.entries.iter().any(|entry| {
        matches!(
            entry.state,
            EntryState::Planned
                | EntryState::PreflightPassed
                | EntryState::Applying
                | EntryState::RollingBack
        )
    });

    if has_envelope(transaction) {
        match exact_resume_state(transaction) {
            Ok(Some(super::model::ExactResumeState::NotResumable))
                if !has_applied && !has_in_flight && !has_unresolved_entry =>
            {
                return RecoveryCleanupClassification::Stale(
                    StaleRecoveryReason::PermanentlyNonResumable,
                );
            }
            Ok(Some(super::model::ExactResumeState::NotResumable)) => {}
            Ok(_) => return RecoveryCleanupClassification::Actionable,
            Err(_) => return RecoveryCleanupClassification::Uncertain,
        }
    }

    // A rollback failure that left no `Applied` entry is normally no longer
    // executable by the rollback engine. It is stale only when every failed
    // destination is present but belongs to a different object; a missing or
    // otherwise ambiguous path remains uncertain for remount/retry recovery.
    if transaction.state == TransactionState::RollbackFailed
        && transaction
            .entries
            .iter()
            .any(|entry| entry.state == EntryState::RollbackFailed)
        && !has_applied
        && !has_in_flight
    {
        let failed_entries = transaction
            .entries
            .iter()
            .filter(|entry| entry.state == EntryState::RollbackFailed)
            .collect::<Vec<_>>();
        let mut identity_changed = false;
        for entry in failed_entries {
            if !source_is_proven_absent(&entry.source_path) {
                return RecoveryCleanupClassification::Uncertain;
            }
            let Ok(current) = capture_identity(&entry.destination_path) else {
                return RecoveryCleanupClassification::Uncertain;
            };
            if !identity_matches(&entry.identity, &current) {
                identity_changed = true;
            } else {
                return RecoveryCleanupClassification::Uncertain;
            }
        }
        if identity_changed {
            return RecoveryCleanupClassification::Stale(
                StaleRecoveryReason::RollbackIdentityChanged,
            );
        }
    }

    if !has_applied && !has_in_flight && !has_unresolved_entry {
        if transaction.state == TransactionState::RolledBack || !transaction.state.needs_recovery()
        {
            return RecoveryCleanupClassification::Stale(StaleRecoveryReason::NoActionableChanges);
        }
        return RecoveryCleanupClassification::Stale(StaleRecoveryReason::LegacyWithoutExactResume);
    }

    // A settled Applied transaction, or an interrupted transaction with
    // applied entries, remains rollbackable unless the current destination is
    // present and provably belongs to a different object. A missing path is
    // intentionally uncertain: it may reappear after a remount or repair.
    let applied_entries = transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .collect::<Vec<_>>();
    if !applied_entries.is_empty() {
        let mut identity_changed = false;
        for entry in applied_entries {
            if !source_is_proven_absent(&entry.source_path) {
                return RecoveryCleanupClassification::Uncertain;
            }
            let Ok(current) = capture_identity(&entry.destination_path) else {
                return RecoveryCleanupClassification::Uncertain;
            };
            if !identity_matches(&entry.identity, &current) {
                identity_changed = true;
            }
        }
        if identity_changed {
            return RecoveryCleanupClassification::Stale(
                StaleRecoveryReason::RollbackIdentityChanged,
            );
        }
        return RecoveryCleanupClassification::Actionable;
    }

    // In-flight entries are never hidden: reconciliation has not proved what
    // happened, even when the overall transaction state looks terminal.
    RecoveryCleanupClassification::Uncertain
}

/// Only NotFound is evidence of absence in the observed namespace.
fn source_is_proven_absent(path: &Path) -> bool {
    matches!(super::lookup::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

/// Returns the sidecar path without creating it.
pub fn recovery_history_state_path(journal_dir: &Path) -> PathBuf {
    journal_dir.join(RECOVERY_HISTORY_STATE_FILE)
}

/// Loads visibility state. An absent sidecar is the normal first-run state.
pub fn load_recovery_history_state(journal_dir: &Path) -> Result<RecoveryHistoryState, String> {
    let path = recovery_history_state_path(journal_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    serde_json::from_str(&text)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))
}

fn write_recovery_history_state(
    journal_dir: &Path,
    state: &RecoveryHistoryState,
) -> Result<(), String> {
    let body = serde_json::to_string_pretty(state)
        .map_err(|error| format!("could not encode recovery history state: {error}"))?;
    crate::atomic_write_text(
        &recovery_history_state_path(journal_dir),
        &format!("{body}\n"),
    )
    .map_err(|error| error.to_string())
}

/// Archives one record only if a fresh journal read still proves it stale.
/// Returns the updated visibility state; no game path or journal is written.
pub fn archive_recovery_transaction(
    journal_dir: &Path,
    transaction_id: &str,
) -> Result<RecoveryHistoryState, String> {
    let path = super::journal_path(journal_dir, transaction_id)
        .ok_or_else(|| format!("transaction id '{transaction_id}' cannot name a journal file"))?;
    let transaction = super::read_journal(&path).map_err(|error| error.to_string())?;
    if !classify_recovery_cleanup(&transaction).is_stale() {
        return Err("this record is still actionable or its status is uncertain".to_string());
    }
    let mut state = load_recovery_history_state(journal_dir)?;
    state
        .archived_transaction_ids
        .insert(transaction_id.to_string());
    write_recovery_history_state(journal_dir, &state)?;
    Ok(state)
}

/// Archives a frozen set, rechecking every journal immediately before the
/// sidecar write. Actionable and uncertain records are left visible and
/// reported; the caller can safely present a partial result.
pub fn archive_recovery_transactions(
    journal_dir: &Path,
    transaction_ids: &[String],
) -> Result<(RecoveryHistoryState, Vec<String>), String> {
    let mut state = load_recovery_history_state(journal_dir)?;
    let mut rejected = Vec::new();
    for transaction_id in transaction_ids {
        let Some(path) = super::journal_path(journal_dir, transaction_id) else {
            rejected.push(format!("{transaction_id}: invalid transaction id"));
            continue;
        };
        let transaction = match super::read_journal(&path) {
            Ok(transaction) => transaction,
            Err(error) => {
                rejected.push(format!("{transaction_id}: {error}"));
                continue;
            }
        };
        if classify_recovery_cleanup(&transaction).is_stale() {
            state
                .archived_transaction_ids
                .insert(transaction_id.clone());
        } else {
            rejected.push(format!(
                "{transaction_id}: still actionable or status is uncertain"
            ));
        }
    }
    if rejected.len() < transaction_ids.len() {
        write_recovery_history_state(journal_dir, &state)?;
    }
    Ok((state, rejected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn transaction(
        id: &str,
        state: TransactionState,
        entry_state: EntryState,
    ) -> RenameTransaction {
        RenameTransaction {
            transaction_id: id.to_string(),
            plan_generation: 1,
            classifier_version: None,
            created_at_unix: 1,
            source_scan_root: String::new(),
            state,
            entries: vec![super::super::model::TransactionEntry {
                source_path: PathBuf::from("/definitely-missing/source.bin"),
                destination_path: PathBuf::from("/definitely-missing/destination.bin"),
                original_basename: "source.bin".to_string(),
                proposed_basename: "destination.bin".to_string(),
                identity: super::super::model::ObjectIdentity {
                    freshness: None,
                    size_bytes: 1,
                    modified_unix: 1,
                    kind: super::super::model::ObjectKind::RegularFile,
                    #[cfg(unix)]
                    ino: 1,
                    #[cfg(unix)]
                    dev: 1,
                },
                operation: Default::default(),
                preflight_passed: false,
                preflight_failures: Vec::new(),
                state: entry_state,
                failure_reason: None,
                applied_at_unix: None,
                rolled_back_at_unix: None,
                unknown: Default::default(),
            }],
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    #[test]
    fn legacy_without_any_applied_or_in_flight_entry_is_stale() {
        let tx = transaction(
            "legacy",
            TransactionState::ApplyFailed,
            EntryState::ApplyFailed,
        );
        assert_eq!(
            classify_recovery_cleanup(&tx),
            RecoveryCleanupClassification::Stale(StaleRecoveryReason::LegacyWithoutExactResume)
        );
    }

    #[test]
    fn in_flight_missing_evidence_is_uncertain() {
        let tx = transaction(
            "uncertain",
            TransactionState::Applying,
            EntryState::Applying,
        );
        assert_eq!(
            classify_recovery_cleanup(&tx),
            RecoveryCleanupClassification::Uncertain
        );
    }

    #[test]
    fn exact_resume_evidence_is_never_archiveable() {
        let mut tx = transaction(
            "resumable",
            TransactionState::ApplyFailed,
            EntryState::ApplyFailed,
        );
        tx.unknown.insert(
            "emuwiz_exact_resume_envelope".to_string(),
            serde_json::json!({"format_version": 1}),
        );
        assert_eq!(
            classify_recovery_cleanup(&tx),
            RecoveryCleanupClassification::Actionable
        );
    }

    #[test]
    fn explicit_not_resumable_marker_is_archiveable_only_without_pending_work() {
        let mut tx = transaction(
            "not-resumable",
            TransactionState::ApplyFailed,
            EntryState::ApplyFailed,
        );
        tx.unknown.insert(
            "emuwiz_exact_resume_envelope".to_string(),
            serde_json::json!({"format_version": 1}),
        );
        tx.unknown.insert(
            "emuwiz_exact_resume_state".to_string(),
            serde_json::json!("not_resumable"),
        );
        assert_eq!(
            classify_recovery_cleanup(&tx),
            RecoveryCleanupClassification::Stale(StaleRecoveryReason::PermanentlyNonResumable)
        );
    }

    #[test]
    fn a_live_rollbackable_destination_is_never_archiveable() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination.bin");
        std::fs::write(&destination, b"original").unwrap();
        let identity = super::super::identity::capture_identity(&destination).unwrap();
        let mut tx = transaction(
            "rollbackable",
            TransactionState::Applied,
            EntryState::Applied,
        );
        tx.entries[0].source_path = dir.path().join("source.bin");
        tx.entries[0].destination_path = destination;
        tx.entries[0].identity = identity;
        assert_eq!(
            classify_recovery_cleanup(&tx),
            RecoveryCleanupClassification::Actionable
        );
    }

    #[test]
    fn an_identity_changed_rollback_record_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination.bin");
        std::fs::write(&destination, b"different object").unwrap();
        let mut tx = transaction(
            "invalid-rollback",
            TransactionState::RollbackFailed,
            EntryState::Applied,
        );
        tx.entries[0].source_path = dir.path().join("source.bin");
        tx.entries[0].destination_path = destination;
        // Deliberately does not match the live destination identity.
        assert_eq!(
            classify_recovery_cleanup(&tx),
            RecoveryCleanupClassification::Stale(StaleRecoveryReason::RollbackIdentityChanged)
        );
    }

    #[test]
    fn a_rollback_failed_entry_with_a_different_destination_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination.bin");
        std::fs::write(&destination, b"different object").unwrap();
        let mut tx = transaction(
            "invalid-rollback-failed",
            TransactionState::RollbackFailed,
            EntryState::RollbackFailed,
        );
        tx.entries[0].source_path = dir.path().join("source.bin");
        tx.entries[0].destination_path = destination;
        assert_eq!(
            classify_recovery_cleanup(&tx),
            RecoveryCleanupClassification::Stale(StaleRecoveryReason::RollbackIdentityChanged)
        );
    }

    #[test]
    fn missing_rollback_destination_is_uncertain_not_stale() {
        let dir = tempfile::tempdir().unwrap();
        let mut tx = transaction(
            "temporary-missing",
            TransactionState::Applied,
            EntryState::Applied,
        );
        tx.entries[0].source_path = dir.path().join("source.bin");
        tx.entries[0].destination_path = dir.path().join("temporarily-unmounted.bin");
        assert_eq!(
            classify_recovery_cleanup(&tx),
            RecoveryCleanupClassification::Uncertain
        );
    }

    #[test]
    fn archiving_writes_only_the_sidecar_and_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let journal_dir = dir.path().join("journals");
        std::fs::create_dir_all(&journal_dir).unwrap();
        let tx = transaction(
            "legacy-archive",
            TransactionState::ApplyFailed,
            EntryState::ApplyFailed,
        );
        super::super::write_journal(&journal_dir, &tx).unwrap();
        let journal_path = super::super::journal_path(&journal_dir, &tx.transaction_id).unwrap();
        let journal_before = std::fs::read_to_string(&journal_path).unwrap();

        let state = archive_recovery_transaction(&journal_dir, &tx.transaction_id).unwrap();
        assert!(state.archived_transaction_ids.contains(&tx.transaction_id));
        assert_eq!(
            std::fs::read_to_string(&journal_path).unwrap(),
            journal_before
        );
        assert!(recovery_history_state_path(&journal_dir).is_file());

        let reloaded = load_recovery_history_state(&journal_dir).unwrap();
        assert!(
            reloaded
                .archived_transaction_ids
                .contains(&tx.transaction_id)
        );
        assert_eq!(super::super::read_journal(&journal_path).unwrap(), tx);
    }
    // Restore directory access before TempDir cleanup, including on assertion failure.
    struct SourceFaultFixture {
        dir: tempfile::TempDir,
        blocked_parent: Option<PathBuf>,
        tx: RenameTransaction,
    }
    impl Drop for SourceFaultFixture {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            if let Some(parent) = &self.blocked_parent {
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum SourceFault {
        Loop,
        PermissionDenied,
    }

    fn source_fault_fixture(fault: SourceFault, applied: bool) -> SourceFaultFixture {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("source-parent");
        std::fs::create_dir(&parent).unwrap();
        let source = parent.join("source.bin");
        std::fs::write(&source, b"original").unwrap();
        let identity = capture_identity(&source).unwrap();
        let destination = dir.path().join("destination.bin");
        std::fs::write(&destination, b"different destination contents").unwrap();
        let blocked_parent = match fault {
            SourceFault::Loop => {
                std::fs::remove_file(&source).unwrap();
                std::fs::remove_dir(&parent).unwrap();
                symlink("source-parent", &parent).unwrap();
                None
            }
            SourceFault::PermissionDenied => {
                std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0)).unwrap();
                Some(parent)
            }
        };
        let error = std::fs::symlink_metadata(&source).unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(match fault {
                SourceFault::Loop => libc::ELOOP,
                SourceFault::PermissionDenied => libc::EACCES,
            })
        );
        let mut tx = transaction(
            "source-lookup-error",
            if applied {
                TransactionState::Applied
            } else {
                TransactionState::RollbackFailed
            },
            if applied {
                EntryState::Applied
            } else {
                EntryState::RollbackFailed
            },
        );
        tx.entries[0].source_path = source;
        tx.entries[0].destination_path = destination;
        tx.entries[0].identity = identity;
        tx.unknown.insert(
            "synthetic_recovery_evidence".into(),
            serde_json::json!({"retain": true}),
        );
        SourceFaultFixture {
            dir,
            blocked_parent,
            tx,
        }
    }

    fn source_faults() -> Vec<SourceFault> {
        let mut faults = vec![SourceFault::Loop];
        // Root bypasses mode bits. Do not claim EACCES coverage in that environment.
        if unsafe { libc::geteuid() } != 0 {
            faults.push(SourceFault::PermissionDenied);
        }
        faults
    }

    #[test]
    fn source_lookup_errors_are_uncertain_in_both_recovery_branches() {
        let mut observed = Vec::new();
        for fault in source_faults() {
            for applied in [false, true] {
                let fixture = source_fault_fixture(fault, applied);
                observed.push((
                    format!("fault={fault:?}, applied={applied}"),
                    classify_recovery_cleanup(&fixture.tx),
                ));
            }
        }
        assert!(
            observed
                .iter()
                .all(|(_, state)| *state == RecoveryCleanupClassification::Uncertain),
            "{observed:?}"
        );
    }

    #[test]
    fn individual_archival_refuses_source_lookup_errors_without_hiding_evidence() {
        let mut observed = Vec::new();
        for fault in source_faults() {
            for applied in [false, true] {
                let fixture = source_fault_fixture(fault, applied);
                let journals = fixture.dir.path().join("journals");
                super::super::write_journal(&journals, &fixture.tx).unwrap();
                let journal =
                    super::super::journal_path(&journals, &fixture.tx.transaction_id).unwrap();
                let before = std::fs::read(&journal).unwrap();
                let result = super::super::archive_recovery_transaction(
                    &journals,
                    &fixture.tx.transaction_id,
                );
                let visibility = load_recovery_history_state(&journals).unwrap();
                assert_eq!(std::fs::read(&journal).unwrap(), before);
                observed.push((
                    format!("fault={fault:?}, applied={applied}"),
                    result.is_err(),
                    visibility
                        .archived_transaction_ids
                        .contains(&fixture.tx.transaction_id),
                    recovery_history_state_path(&journals).exists(),
                ));
            }
        }
        assert!(
            observed
                .iter()
                .all(|(_, refused, hidden, sidecar)| *refused && !*hidden && !*sidecar),
            "{observed:?}"
        );
    }

    #[test]
    fn batch_archival_preserves_uncertain_records_and_archives_genuine_stale_records() {
        for fault in source_faults() {
            let fixture = source_fault_fixture(fault, false);
            let journals = fixture.dir.path().join("journals");
            let failed = fixture.tx.clone();
            let mut applied = failed.clone();
            applied.transaction_id = "applied-source-lookup-error".into();
            applied.state = TransactionState::Applied;
            applied.entries[0].state = EntryState::Applied;
            let mut stale = failed.clone();
            stale.transaction_id = "genuinely-stale".into();
            stale.entries[0].source_path = fixture.dir.path().join("genuinely-absent.bin");
            assert_eq!(
                std::fs::symlink_metadata(&stale.entries[0].source_path)
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::NotFound
            );
            let records = [&failed, &applied, &stale];
            let mut before = Vec::new();
            for tx in records {
                super::super::write_journal(&journals, tx).unwrap();
                let path = super::super::journal_path(&journals, &tx.transaction_id).unwrap();
                before.push((path.clone(), std::fs::read(path).unwrap()));
            }
            let ids = records.map(|tx| tx.transaction_id.clone());
            let (state, rejected) =
                super::super::archive_recovery_transactions(&journals, &ids).unwrap();
            assert_eq!(
                state.archived_transaction_ids,
                BTreeSet::from([stale.transaction_id.clone()]),
                "fault={fault:?}, rejected={rejected:?}"
            );
            assert_eq!(rejected.len(), 2);
            assert!(
                rejected
                    .iter()
                    .any(|s| s.starts_with(&failed.transaction_id))
            );
            assert!(
                rejected
                    .iter()
                    .any(|s| s.starts_with(&applied.transaction_id))
            );
            assert_eq!(load_recovery_history_state(&journals).unwrap(), state);
            for (path, bytes) in before {
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            }
            // A genuine NotFound + mismatched destination still permits individual archival.
            assert!(
                super::super::archive_recovery_transaction(&journals, &stale.transaction_id)
                    .is_ok()
            );
        }
    }
    #[test]
    fn injected_lookup_failures_never_authorize_history_archival() {
        for errno in [
            libc::EIO,
            libc::ENODEV,
            libc::ENOTCONN,
            libc::ESTALE,
            libc::ENOTDIR,
            libc::EINVAL,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let journals = dir.path().join("journals");
            let mut tx = transaction(
                "lookup-fault",
                TransactionState::RollbackFailed,
                EntryState::RollbackFailed,
            );
            tx.entries[0].source_path = dir.path().join("uncertain-source.bin");
            tx.entries[0].destination_path = dir.path().join("foreign.bin");
            std::fs::write(&tx.entries[0].destination_path, b"foreign").unwrap();
            super::super::write_journal(&journals, &tx).unwrap();
            let path = super::super::journal_path(&journals, &tx.transaction_id).unwrap();
            let before = std::fs::read(&path).unwrap();
            let _fault = super::super::lookup::Fault::at(&tx.entries[0].source_path, errno);
            assert_eq!(
                classify_recovery_cleanup(&tx),
                RecoveryCleanupClassification::Uncertain,
                "errno={errno}"
            );
            assert!(archive_recovery_transaction(&journals, &tx.transaction_id).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(!recovery_history_state_path(&journals).exists());
        }
    }
}
