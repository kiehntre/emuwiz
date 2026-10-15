//! Filesystem reconciliation of in-flight transaction entries after a crash.
//!
//! A durable journal checkpoint marks an entry `Applying` (or, during rollback,
//! `RollingBack`) **before** the corresponding rename syscall. If the process
//! dies after the checkpoint but before the terminal state is persisted, the
//! journal cannot say whether the syscall ran. Recovery reconciles the entry
//! against the filesystem - read-only, never resuming the rename - and
//! classifies it:
//!
//! - only at the source (identity matches, destination absent): the rename (or
//!   reverse rename) did not happen;
//! - only at the destination (identity matches, source absent): the rename did
//!   happen and is filesystem-confirmed;
//! - both present, or both absent, or an identity mismatch: unsafe or unknown -
//!   the entry is left unresolved for manual review, never guessed.
//!
//! The reconciled state is persisted to the journal before it is exposed, so
//! `applied_count()` reflects reality and rollback can act on entries the
//! filesystem proved were renamed.

use std::path::Path;

use super::identity::{capture_identity, content_identity_matches, identity_matches};
use super::journal::checkpoint_journal as write_journal;
use super::model::{
    EntryState, RenameTransaction, TransactionEntry, TransactionOperation, TransactionState,
};

/// Why an in-flight entry could not be cleanly classified, or what it was
/// reconciled to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryIssueKind {
    /// The rename (or reverse rename) did not happen; reconciled as not
    /// applied / rolled back.
    RenameDidNotHappen,
    /// The rename did happen; reconciled as filesystem-confirmed Applied (or
    /// RolledBack for an in-flight rollback).
    RenameConfirmed,
    /// Source and destination both exist. Unsafe; left unresolved.
    BothSourceAndDestination,
    /// Neither source nor destination exists. Unknown; left unresolved.
    BothAbsent,
    /// The filesystem shows the rename happened (or was reversed) but its
    /// directory entries could not be synchronised, so completion cannot be
    /// recorded. Left unresolved; never retried automatically.
    DirectorySyncFailed,
    /// The destination exists but is not the recorded object. Unsafe external
    /// change; left unresolved.
    DestinationIdentityChanged,
    /// The source exists but is not the recorded object. Unsafe external
    /// change; left unresolved.
    SourceIdentityChanged,
}

impl RecoveryIssueKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::RenameDidNotHappen => "Rename did not happen",
            Self::RenameConfirmed => "Rename confirmed by the filesystem",
            Self::BothSourceAndDestination => "Source and destination both exist",
            Self::DirectorySyncFailed => "Directory entries could not be synchronised",
            Self::BothAbsent => "Neither source nor destination exists",
            Self::DestinationIdentityChanged => "Destination identity changed",
            Self::SourceIdentityChanged => "Source identity changed",
        }
    }
}

/// One reconciliation finding for one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryIssue {
    pub entry_index: usize,
    pub kind: RecoveryIssueKind,
    pub detail: String,
}

/// Reconciles every in-flight (`Applying` / `RollingBack`) entry of
/// `transaction` against the filesystem, persists the reconciled journal, and
/// returns the findings.
///
/// Read-only with respect to files (only `symlink_metadata`/`read`-style
/// identity capture); never resumes a rename. Entries that cannot be cleanly
/// classified are left unresolved so manual review is required.
pub fn reconcile_recovery(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
) -> Result<Vec<RecoveryIssue>, String> {
    #[cfg(target_os = "linux")]
    if transaction.entries.iter().any(|entry| {
        super::identity::fd_publication_probe::contract::enrolled(entry)
            && entry
                .unknown
                .contains_key(super::identity::fd_publication_probe::contract::KEY)
    }) {
        return Err("FD holding evidence requires signed FD recovery; legacy two-name reconciliation refused".into());
    }
    super::journal::verify_applied_containment(transaction).map_err(|e| e.to_string())?;
    let mut issues = Vec::new();
    let mut changed = false;
    let original_state = transaction.state;
    let mut published_bindings = Vec::new();

    for index in 0..transaction.entries.len() {
        let state = transaction.entries[index].state;
        if !matches!(state, EntryState::Applying | EntryState::RollingBack) {
            continue;
        }
        // A rename's parent directories are opened and identity-checked FIRST,
        // and the entry is classified THROUGH those descriptors, so the object
        // examined, the directory synced and the directory verified are one and
        // the same: a parent replaced between "observe" and "sync" can neither
        // make an unrelated file look like the journalled one nor receive the
        // durability sync in its place.
        let rename_move = matches!(
            transaction.entries[index].operation,
            TransactionOperation::RenameMove
        );
        let bound = if rename_move {
            // Anchored at the journalled collection root when the entry lies below
            // it; otherwise the path must be its own real path. A redirected,
            // moved or symlink-substituted ancestor cannot bind, so the entry stays
            // unresolved and is never promoted on file identity alone.
            let anchor = std::path::Path::new(&transaction.source_scan_root);
            super::dirsync::RenameDirs::open_anchored(
                &transaction.entries[index].source_path,
                &transaction.entries[index].destination_path,
                Some(anchor),
            )
            .ok()
        } else {
            None
        };
        let mut issue = match &bound {
            Some(dirs) => classify_entry_bound(&transaction.entries[index], index, dirs),
            None => classify_entry(&transaction.entries[index], index),
        };
        // A rename's directories that could not be bound (redirected or moved
        // ancestor, symlink substitution, outside the anchor) never promote an
        // entry: "the object looks right" through an unverified path proves
        // nothing about where it is.  Applying and RollingBack alike.
        if rename_move && bound.is_none() && issue.kind == RecoveryIssueKind::RenameConfirmed {
            issue = RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::DirectorySyncFailed,
                detail: "the parent directories could not be bound inside the journalled \
                         collection root (redirected, moved or substituted ancestor, or the \
                         path is not its own real path); containment is unproven so the \
                         entry is left unresolved"
                    .to_string(),
            };
        }
        // Promoting an entry on the strength of a rename the filesystem shows
        // must not outrun the durability of that rename's directory entries.
        // Nothing was mutated for "did not happen" while Applying.
        let mutated = match issue.kind {
            RecoveryIssueKind::RenameConfirmed => state == EntryState::Applying,
            RecoveryIssueKind::RenameDidNotHappen => state == EntryState::RollingBack,
            _ => false,
        };
        if mutated && rename_move {
            // Applying + confirmed: the entry is at the destination.
            // RollingBack + not happened: the reverse happened, entry at source.
            let synced = match &bound {
                Some(dirs) => dirs.sync_and_verify(state == EntryState::RollingBack),
                None => Err(std::io::Error::other(
                    "the parent directories could not be bound and verified",
                )),
            };
            if let Err(error) = synced {
                issue = RecoveryIssue {
                    entry_index: index,
                    kind: RecoveryIssueKind::DirectorySyncFailed,
                    detail: format!(
                        "the filesystem shows the rename happened but its directory entries \
                         could not be synchronised: {error}"
                    ),
                };
            }
        }
        match issue.kind {
            RecoveryIssueKind::RenameDidNotHappen => {
                // Applying: the rename did not happen. RollingBack: the reverse
                // rename did happen (the file is back at source).
                if state == EntryState::Applying {
                    transaction.entries[index].state = EntryState::Skipped;
                } else {
                    transaction.entries[index].state = EntryState::RolledBack;
                    transaction.entries[index].rolled_back_at_unix =
                        Some(crate::dat::sources::now_unix());
                }
                changed = true;
            }
            RecoveryIssueKind::RenameConfirmed => {
                // Applying: the rename happened. RollingBack: it did not (the
                // file is still applied) - back to Applied so rollback can act.
                transaction.entries[index].state = EntryState::Applied;
                if state == EntryState::Applying {
                    transaction.entries[index].applied_at_unix =
                        Some(crate::dat::sources::now_unix());
                }
                changed = true;
            }
            _ => {
                // Unsafe or unknown: leave unresolved for manual review.
            }
        }
        if matches!(
            issue.kind,
            RecoveryIssueKind::RenameConfirmed | RecoveryIssueKind::RenameDidNotHappen
        ) {
            if let Some(dirs) = bound {
                published_bindings.push((index, state, dirs));
            }
        }
        issues.push(issue);
    }

    changed |= reconcile_transaction_level_state(transaction);

    if changed {
        write_journal(journal_dir, transaction).map_err(|error| error.to_string())?;
        #[cfg(test)]
        AFTER_PUBLICATION.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let mut invalid = false;
        for (index, state, dirs) in published_bindings {
            if let Err(error) = dirs.verify_bound() {
                invalid = true;
                transaction.entries[index].state = state;
                transaction.entries[index].failure_reason = Some(format!(
                    "containment lost during recovery publication: {error}"
                ));
                if let Some(issue) = issues.iter_mut().find(|i| i.entry_index == index) {
                    issue.kind = RecoveryIssueKind::DirectorySyncFailed;
                    issue.detail = transaction.entries[index].failure_reason.clone().unwrap();
                }
            }
        }
        if invalid {
            transaction.state = if original_state == TransactionState::RollingBack {
                original_state
            } else {
                TransactionState::Applying
            };
            // One correction attempt, never a claim of atomic exclusion. If it
            // fails, return an error with unresolved in-memory state. Reload
            // checks redirected Applied paths independently of this write.
            write_journal(journal_dir, transaction).map_err(|error| format!(
                "recovery containment lost; unresolved correction could not be persisted; journal may still record historical completion: {error}"
            ))?;
        }
    }
    Ok(issues)
}

/// Repairs a transaction-level `state` left stuck at `Applying` by a final
/// journal write that never landed, even though every entry's own (already
/// durable, and by this point already entry-reconciled) state proves the
/// batch actually finished.
///
/// [`super::executor::apply_transaction`] durably checkpoints `Applying`
/// before its per-entry loop, then - unconditionally, as the very next
/// statement once the loop ends with no failure - bumps `transaction.state`
/// to `Applied` and writes that. Every *failure* branch inside that loop
/// instead sets `transaction.state = ApplyFailed` and writes it in the same
/// synchronous step as the entry's own failed state, so a durably observed
/// entry failure is always paired with a durably observed `ApplyFailed` at
/// the transaction level - the two can never disagree. That leaves exactly
/// one way for `transaction.state` to still read `Applying` once every
/// entry has settled: the final unconditional write above ran and (for
/// whatever reason - a transient I/O failure) did not persist. This
/// reconstructs the outcome that final write was always going to record,
/// using the same "every entry settled cleanly" rule the executor itself
/// already relies on - never a new state machine, never a guess.
///
/// Fails closed: an entry left in any state other than `Applied`/`Skipped`
/// (including one this same call's entry-reconciliation pass could not
/// safely resolve, so it is still `Applying`/`RollingBack`) means the batch
/// cannot be proven complete, so `transaction.state` is left exactly as it
/// was for manual review - never guessed at, never promoted.
fn reconcile_transaction_level_state(transaction: &mut RenameTransaction) -> bool {
    if transaction.state != TransactionState::Applying {
        return false;
    }
    let every_entry_settled_clean = transaction
        .entries
        .iter()
        .all(|entry| matches!(entry.state, EntryState::Applied | EntryState::Skipped));
    if !every_entry_settled_clean {
        return false;
    }
    transaction.state = TransactionState::Applied;
    true
}

/// [`classify_entry`], but every path is resolved inside the retained parent
/// descriptors (`/proc/self/fd/N/name`), and the journalled identity is
/// compared with the object found THERE. Details are reported with the
/// original paths.
fn classify_entry_bound(
    entry: &TransactionEntry,
    index: usize,
    dirs: &super::dirsync::RenameDirs,
) -> RecoveryIssue {
    let mut probe = entry.clone();
    probe.source_path = dirs.source_entry();
    probe.destination_path = dirs.destination_entry();
    let mut issue = classify_entry(&probe, index);
    issue.detail = issue
        .detail
        .replace(
            &probe.source_path.display().to_string(),
            &entry.source_path.display().to_string(),
        )
        .replace(
            &probe.destination_path.display().to_string(),
            &entry.destination_path.display().to_string(),
        );
    issue
}

/// Classifies one in-flight entry against the live filesystem.
pub(super) fn classify_entry(entry: &TransactionEntry, index: usize) -> RecoveryIssue {
    if matches!(
        entry.operation,
        TransactionOperation::ReplaceExisting { .. }
    ) {
        let (kind, detail) = match super::replacement::published(entry) {
            Ok(true) => (
                RecoveryIssueKind::RenameConfirmed,
                "replacement and preserved original confirmed".into(),
            ),
            Ok(false) => (
                RecoveryIssueKind::RenameDidNotHappen,
                "original target and staged replacement confirmed".into(),
            ),
            Err(reason) => (RecoveryIssueKind::DestinationIdentityChanged, reason),
        };
        return RecoveryIssue {
            entry_index: index,
            kind,
            detail,
        };
    }
    if let TransactionOperation::CreateSymlink {
        expected_target,
        destination_root,
    } = &entry.operation
    {
        if !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
            || !expected_target.is_absolute()
            || expected_target != &entry.source_path
        {
            return RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::DestinationIdentityChanged,
                detail:
                    "journalled link destination authority is invalid; manual review is required"
                        .to_string(),
            };
        }
        let source_matches = capture_identity(&entry.source_path)
            .ok()
            .is_some_and(|identity| identity_matches(&entry.identity, &identity));
        let exact_link = std::fs::symlink_metadata(&entry.destination_path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
            && std::fs::read_link(&entry.destination_path).ok().as_deref() == Some(expected_target);
        return if !source_matches {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::SourceIdentityChanged,
                detail: "link source changed or disappeared; manual review is required".to_string(),
            }
        } else if exact_link {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::RenameConfirmed,
                detail: "link creation confirmed; source intentionally remains present".to_string(),
            }
        } else if std::fs::symlink_metadata(&entry.destination_path).is_err() {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::RenameDidNotHappen,
                detail: "link destination is absent; link creation did not happen".to_string(),
            }
        } else {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::DestinationIdentityChanged,
                detail:
                    "link destination differs from the journalled target; manual review is required"
                        .to_string(),
            }
        };
    }
    if let TransactionOperation::CreateHardlink {
        expected_source,
        destination_root,
    } = &entry.operation
    {
        if !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
            || !expected_source.is_absolute()
            || expected_source != &entry.source_path
        {
            return RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::DestinationIdentityChanged,
                detail: "journalled hardlink destination authority is invalid; manual review is required".to_string(),
            };
        }
        let source_matches = capture_identity(&entry.source_path)
            .ok()
            .is_some_and(|identity| identity_matches(&entry.identity, &identity));
        let destination_matches = capture_identity(&entry.destination_path)
            .ok()
            .is_some_and(|identity| identity_matches(&entry.identity, &identity));
        return if source_matches && destination_matches {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::RenameConfirmed,
                detail: "hardlink creation confirmed; source intentionally remains present"
                    .to_string(),
            }
        } else if !source_matches {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::SourceIdentityChanged,
                detail: "hardlink source changed or disappeared; manual review is required"
                    .to_string(),
            }
        } else if std::fs::symlink_metadata(&entry.destination_path).is_err() {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::RenameDidNotHappen,
                detail: "hardlink destination is absent; link creation did not happen".to_string(),
            }
        } else {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::DestinationIdentityChanged,
                detail: "hardlink destination identity changed; manual review is required"
                    .to_string(),
            }
        };
    }
    if let TransactionOperation::CreateCopy {
        expected_source,
        destination_root,
    } = &entry.operation
    {
        if !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
            || !expected_source.is_absolute()
            || expected_source != &entry.source_path
        {
            return RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::DestinationIdentityChanged,
                detail:
                    "journalled copy destination authority is invalid; manual review is required"
                        .into(),
            };
        }
        let source_matches = capture_identity(&entry.source_path)
            .ok()
            .is_some_and(|identity| identity_matches(&entry.identity, &identity));
        let destination_matches = capture_identity(&entry.destination_path)
            .ok()
            .is_some_and(|identity| content_identity_matches(&entry.identity, &identity));
        return if source_matches && destination_matches {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::RenameConfirmed,
                detail: "copy creation confirmed; source intentionally remains present".into(),
            }
        } else if !source_matches {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::SourceIdentityChanged,
                detail: "copy source changed or disappeared; manual review is required".into(),
            }
        } else if std::fs::symlink_metadata(&entry.destination_path).is_err() {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::RenameDidNotHappen,
                detail: "copy destination is absent; copy did not happen".into(),
            }
        } else {
            RecoveryIssue {
                entry_index: index,
                kind: RecoveryIssueKind::DestinationIdentityChanged,
                detail: "copy destination differs from the journalled content; manual review is required".into(),
            }
        };
    }
    let source_present = std::fs::symlink_metadata(&entry.source_path).is_ok();
    let destination_present = std::fs::symlink_metadata(&entry.destination_path).is_ok();
    let source_matches = source_present
        .then(|| capture_identity(&entry.source_path).ok())
        .flatten()
        .is_some_and(|identity| identity_matches(&entry.identity, &identity));
    let destination_matches = destination_present
        .then(|| capture_identity(&entry.destination_path).ok())
        .flatten()
        .is_some_and(|identity| identity_matches(&entry.identity, &identity));

    if destination_present && !destination_matches {
        RecoveryIssue {
            entry_index: index,
            kind: RecoveryIssueKind::DestinationIdentityChanged,
            detail: "the destination exists but is not the recorded object; manual review is \
                     required"
                .to_string(),
        }
    } else if source_present && !source_matches {
        RecoveryIssue {
            entry_index: index,
            kind: RecoveryIssueKind::SourceIdentityChanged,
            detail: "the source exists but is not the recorded object; manual review is required"
                .to_string(),
        }
    } else if source_matches && !destination_present {
        RecoveryIssue {
            entry_index: index,
            kind: RecoveryIssueKind::RenameDidNotHappen,
            detail: "the source is intact and the destination is absent; no rename happened"
                .to_string(),
        }
    } else if !source_present && destination_matches {
        RecoveryIssue {
            entry_index: index,
            kind: RecoveryIssueKind::RenameConfirmed,
            detail: "the source is gone and the destination matches the recorded identity; the \
                     rename happened"
                .to_string(),
        }
    } else if source_present && destination_present {
        RecoveryIssue {
            entry_index: index,
            kind: RecoveryIssueKind::BothSourceAndDestination,
            detail: "source and destination both exist; refusing to guess which is intended"
                .to_string(),
        }
    } else {
        RecoveryIssue {
            entry_index: index,
            kind: RecoveryIssueKind::BothAbsent,
            detail: "neither the source nor the destination exists; manual review is required"
                .to_string(),
        }
    }
}

#[cfg(test)]
thread_local! {
    static AFTER_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::super::journal::write_journal;
    use super::*;

    fn entry(source: &Path, destination: &Path, state: EntryState) -> TransactionEntry {
        TransactionEntry {
            source_path: source.to_path_buf(),
            destination_path: destination.to_path_buf(),
            original_basename: source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            proposed_basename: destination
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            identity: capture_identity(source).unwrap(),
            operation: Default::default(),
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }
    }

    fn transaction_with(entry: TransactionEntry) -> RenameTransaction {
        transaction_with_entries(vec![entry])
    }

    fn transaction_with_entries(entries: Vec<TransactionEntry>) -> RenameTransaction {
        RenameTransaction {
            transaction_id: "reconcile-test".to_string(),
            plan_generation: 1,
            classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
            created_at_unix: 1,
            source_scan_root: "/tmp/roms".to_string(),
            state: super::super::model::TransactionState::Applying,
            entries,
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    #[test]
    fn a_source_only_applying_entry_is_not_applied() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        std::fs::write(&source, b"data").unwrap();
        let destination = dir.path().join("b.bin");
        let mut tx = transaction_with(entry(&source, &destination, EntryState::Applying));
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let issues = reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(tx.entries[0].state, EntryState::Skipped);
        assert_eq!(issues[0].kind, RecoveryIssueKind::RenameDidNotHappen);
    }

    #[test]
    fn a_destination_only_applying_entry_is_confirmed_applied() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        std::fs::write(&source, b"data").unwrap();
        let destination = dir.path().join("b.bin");
        let mut tx = transaction_with(entry(&source, &destination, EntryState::Applying));
        // Simulate the rename having happened, with no journal update.
        std::fs::rename(&source, &destination).unwrap();
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let issues = reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(tx.entries[0].state, EntryState::Applied);
        assert_eq!(issues[0].kind, RecoveryIssueKind::RenameConfirmed);
    }

    #[test]
    fn a_rolling_back_entry_with_source_restored_is_rolled_back() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        std::fs::write(&source, b"data").unwrap();
        let destination = dir.path().join("b.bin");
        let mut tx = transaction_with(entry(&source, &destination, EntryState::RollingBack));
        // Reverse rename already happened; journal still says RollingBack.
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let issues = reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(tx.entries[0].state, EntryState::RolledBack);
        assert_eq!(issues[0].kind, RecoveryIssueKind::RenameDidNotHappen);
    }

    #[test]
    fn a_rolling_back_entry_still_at_destination_is_back_to_applied() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        std::fs::write(&source, b"data").unwrap();
        let destination = dir.path().join("b.bin");
        let mut tx = transaction_with(entry(&source, &destination, EntryState::RollingBack));
        std::fs::rename(&source, &destination).unwrap();
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let issues = reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(tx.entries[0].state, EntryState::Applied);
        assert_eq!(issues[0].kind, RecoveryIssueKind::RenameConfirmed);
    }

    #[test]
    fn both_present_is_an_unresolved_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        std::fs::write(&source, b"data").unwrap();
        let destination = dir.path().join("b.bin");
        // A hard link shares the inode, so both paths exist with matching
        // identity - an indeterminate state reconciliation must not resolve.
        std::fs::hard_link(&source, &destination).unwrap();
        let mut tx = transaction_with(entry(&source, &destination, EntryState::Applying));
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let issues = reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(tx.entries[0].state, EntryState::Applying, "left unresolved");
        assert_eq!(issues[0].kind, RecoveryIssueKind::BothSourceAndDestination);
    }

    #[test]
    fn both_absent_is_an_unresolved_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("gone.bin");
        let destination = dir.path().join("gone2.bin");
        // Build identity from a temporary stand-in so the entry is well-formed;
        // neither the source nor the destination path ever exists.
        let stand_in = dir.path().join("standin.bin");
        std::fs::write(&stand_in, b"data").unwrap();
        let mut entry = entry(&stand_in, &destination, EntryState::Applying);
        entry.identity = capture_identity(&stand_in).unwrap();
        entry.source_path = source;
        let mut tx = transaction_with(entry);
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let issues = reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(tx.entries[0].state, EntryState::Applying, "left unresolved");
        assert_eq!(issues[0].kind, RecoveryIssueKind::BothAbsent);
    }

    #[test]
    fn destination_identity_change_is_unresolved_and_manual() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        std::fs::write(&source, b"data").unwrap();
        let destination = dir.path().join("b.bin");
        let mut tx = transaction_with(entry(&source, &destination, EntryState::Applying));
        std::fs::rename(&source, &destination).unwrap();
        // Replace the destination with a different object.
        std::fs::remove_file(&destination).unwrap();
        std::fs::write(&destination, b"replaced").unwrap();
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let issues = reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(tx.entries[0].state, EntryState::Applying, "left unresolved");
        assert_eq!(
            issues[0].kind,
            RecoveryIssueKind::DestinationIdentityChanged
        );
    }

    // -------------------------------------------------------------------
    // Transaction-level reconciliation: a `transaction.state` stuck at
    // `Applying` after every entry has already durably settled (the final
    // journal write that would have bumped it to `Applied` never landed).
    // -------------------------------------------------------------------

    /// A journal whose transaction-level state is `Applying` but whose sole
    /// entry is already durably `Applied` must be reconciled to
    /// `Applied` - and that correction must itself be persisted to disk,
    /// not just held in memory.
    #[test]
    fn a_transaction_stuck_applying_with_an_applied_entry_is_reconciled_to_applied() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        let destination = dir.path().join("b.bin");
        // A placeholder so the `entry()` builder's own identity capture
        // succeeds; overwritten below with the destination's real identity,
        // exactly as `both_absent_is_an_unresolved_unknown` already does for
        // the same reason.
        std::fs::write(&source, b"placeholder").unwrap();
        std::fs::write(&destination, b"data").unwrap();
        let mut applied_entry = entry(&source, &destination, EntryState::Applied);
        // The entry's own recorded identity must match the (already renamed)
        // destination for it to be a genuinely durable `Applied` entry, not
        // just a label - `capture_identity` was taken from `source` before
        // it existed, so retake it from the real, already-renamed object.
        applied_entry.identity = capture_identity(&destination).unwrap();
        let mut tx = transaction_with(applied_entry);
        assert_eq!(tx.state, TransactionState::Applying);

        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        write_journal(&journal, &tx).unwrap();

        let issues = reconcile_recovery(&mut tx, &journal).unwrap();

        assert_eq!(tx.state, TransactionState::Applied);
        // The entry itself needed no reconciliation (it was never
        // `Applying`/`RollingBack`), so there is nothing to report about it.
        assert!(issues.is_empty());

        // The correction is durable, not just an in-memory patch.
        let path = super::super::journal::journal_path(&journal, &tx.transaction_id).unwrap();
        let reread = super::super::journal::read_journal(&path).unwrap();
        assert_eq!(reread.state, TransactionState::Applied);
    }

    /// The same shape, but with a `Skipped` entry alongside the `Applied`
    /// one (a `SkipUnsafeSubset` batch) - `Skipped` is just as settled as
    /// `Applied`, and must not block the transaction-level correction.
    #[test]
    fn a_transaction_stuck_applying_with_applied_and_skipped_entries_is_reconciled_to_applied() {
        let dir = tempfile::tempdir().unwrap();
        let applied_source = dir.path().join("a.bin");
        let applied_destination = dir.path().join("a-renamed.bin");
        std::fs::write(&applied_source, b"placeholder").unwrap();
        std::fs::write(&applied_destination, b"data").unwrap();
        let mut applied_entry = entry(&applied_source, &applied_destination, EntryState::Applied);
        applied_entry.identity = capture_identity(&applied_destination).unwrap();

        let skipped_source = dir.path().join("c.bin");
        std::fs::write(&skipped_source, b"never touched").unwrap();
        let skipped_destination = dir.path().join("c-renamed.bin");
        let mut skipped_entry = entry(&skipped_source, &skipped_destination, EntryState::Skipped);
        skipped_entry.identity = capture_identity(&skipped_source).unwrap();

        let mut tx = transaction_with_entries(vec![applied_entry, skipped_entry]);
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();

        reconcile_recovery(&mut tx, &journal).unwrap();

        assert_eq!(tx.state, TransactionState::Applied);
    }

    /// Fail closed: a transaction whose overall state is `Applying` but
    /// which still has a genuinely unresolvable in-flight entry (this
    /// reconciliation pass could not prove what happened to it) must never
    /// be promoted to `Applied` - the batch cannot be proven complete.
    #[test]
    fn a_transaction_with_a_genuinely_unresolved_entry_is_not_promoted_to_applied() {
        let dir = tempfile::tempdir().unwrap();

        let applied_source = dir.path().join("a.bin");
        let applied_destination = dir.path().join("a-renamed.bin");
        std::fs::write(&applied_source, b"placeholder").unwrap();
        std::fs::write(&applied_destination, b"data").unwrap();
        let mut applied_entry = entry(&applied_source, &applied_destination, EntryState::Applied);
        applied_entry.identity = capture_identity(&applied_destination).unwrap();

        // A hard-linked source+destination pair is the same unresolvable
        // "both present, matching identity" shape the existing
        // `both_present_is_an_unresolved_conflict` test already proves stays
        // `Applying` at the entry level.
        let unresolved_source = dir.path().join("d.bin");
        std::fs::write(&unresolved_source, b"ambiguous").unwrap();
        let unresolved_destination = dir.path().join("d-renamed.bin");
        std::fs::hard_link(&unresolved_source, &unresolved_destination).unwrap();
        let unresolved_entry = entry(
            &unresolved_source,
            &unresolved_destination,
            EntryState::Applying,
        );

        let mut tx = transaction_with_entries(vec![applied_entry, unresolved_entry]);
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();

        let issues = reconcile_recovery(&mut tx, &journal).unwrap();

        assert_eq!(
            tx.entries[1].state,
            EntryState::Applying,
            "the ambiguous entry must stay unresolved"
        );
        assert_eq!(issues[0].kind, RecoveryIssueKind::BothSourceAndDestination);
        assert_eq!(
            tx.state,
            TransactionState::Applying,
            "the batch cannot be proven complete while one entry is still unresolved, so the \
             transaction-level state must never be promoted"
        );
    }

    /// A reconciled `Applied` transaction is exactly as rollbackable as one
    /// that reached `Applied` through the normal apply path - the
    /// reconciliation must never leave it in a state the rest of the system
    /// treats differently.
    #[test]
    fn a_reconciled_transaction_remains_rollbackable() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        let destination = dir.path().join("b.bin");
        std::fs::write(&source, b"placeholder").unwrap();
        std::fs::write(&destination, b"data").unwrap();
        let mut applied_entry = entry(&source, &destination, EntryState::Applied);
        applied_entry.identity = capture_identity(&destination).unwrap();
        let mut tx = transaction_with(applied_entry);

        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        reconcile_recovery(&mut tx, &journal).unwrap();

        assert_eq!(tx.state, TransactionState::Applied);
        assert!(
            tx.is_rollbackable(),
            "a reconciled Applied transaction with an applied entry must still be rollbackable"
        );
    }

    /// Existing `RollingBack` recovery semantics are unchanged: a
    /// transaction whose overall state is `RollingBack` (never `Applying`)
    /// must never have its transaction-level state touched by the new
    /// reconciliation step, even after its entries settle.
    #[test]
    fn a_rolling_back_transaction_level_state_is_never_touched_by_the_new_step() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.bin");
        std::fs::write(&source, b"data").unwrap();
        let destination = dir.path().join("b.bin");
        let mut tx = transaction_with(entry(&source, &destination, EntryState::RollingBack));
        tx.state = super::super::model::TransactionState::RollingBack;
        // Reverse rename already happened; journal still says RollingBack.
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();

        reconcile_recovery(&mut tx, &journal).unwrap();

        assert_eq!(tx.entries[0].state, EntryState::RolledBack);
        assert_eq!(
            tx.state,
            TransactionState::RollingBack,
            "only the entry settles here; the transaction-level state is a separate concern the \
             rollback executor itself owns, and the new Applying-only reconciliation step must \
             never touch a RollingBack transaction"
        );
    }
    #[cfg(unix)]
    fn residual_fixture() -> (tempfile::TempDir, RenameTransaction, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("roms");
        let parent = root.join("sub");
        let journal = d.path().join("journal");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::create_dir(&journal).unwrap();
        let source = root.join("a");
        let destination = parent.join("b");
        std::fs::write(&source, b"original bytes").unwrap();
        let mut tx = transaction_with(entry(&source, &destination, EntryState::Applying));
        tx.source_scan_root = root.to_string_lossy().into_owned();
        std::fs::rename(&source, &destination).unwrap();
        write_journal(&journal, &tx).unwrap();
        (d, tx, journal)
    }

    #[test]
    #[cfg(unix)]
    fn residual_root_symlink_cannot_establish_recovery_anchor() {
        let (d, mut tx, journal) = residual_fixture();
        let root = d.path().join("roms");
        let moved = d.path().join("moved");
        std::fs::rename(&root, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &root).unwrap();
        let issues = reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(issues[0].kind, RecoveryIssueKind::DirectorySyncFailed);
        assert_eq!(tx.entries[0].state, EntryState::Applying);
        assert_eq!(
            std::fs::read(moved.join("sub/b")).unwrap(),
            b"original bytes"
        );
    }

    #[test]
    #[cfg(unix)]
    fn residual_late_swap_corrected_and_reload_remains_unresolved() {
        residual_late_swap(false);
    }

    #[test]
    #[cfg(unix)]
    fn residual_failed_correction_rejects_stale_applied_on_reload_and_undo() {
        residual_late_swap(true);
    }

    #[cfg(unix)]
    fn residual_late_swap(fail_correction: bool) {
        let (d, mut tx, journal) = residual_fixture();
        let parent = d.path().join("roms/sub");
        let moved = d.path().join("moved");
        let old_journal = d.path().join("preserved-journal");
        let hook_journal = journal.clone();
        let hook_old = old_journal.clone();
        let hook_moved = moved.clone();
        AFTER_PUBLICATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&parent, &hook_moved).unwrap();
                std::os::unix::fs::symlink(&hook_moved, &parent).unwrap();
                if fail_correction {
                    std::fs::rename(&hook_journal, &hook_old).unwrap();
                    std::fs::write(&hook_journal, b"blocked correction").unwrap();
                }
            }))
        });
        let result = reconcile_recovery(&mut tx, &journal);
        assert_eq!(tx.entries[0].state, EntryState::Applying);
        assert_eq!(tx.state, TransactionState::Applying);
        assert_eq!(std::fs::read(moved.join("b")).unwrap(), b"original bytes");
        let saved = if fail_correction {
            &old_journal
        } else {
            &journal
        };
        let path = super::super::journal::journal_path(saved, &tx.transaction_id).unwrap();
        let raw: RenameTransaction =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if fail_correction {
            assert!(result.unwrap_err().contains("could not be persisted"));
            assert_eq!(raw.entries[0].state, EntryState::Applied);
            assert!(super::super::journal::read_journal(&path).is_err());
            assert_eq!(
                super::super::history::classify_recovery_cleanup(&raw),
                super::super::history::RecoveryCleanupClassification::Uncertain
            );
            let mut raw = raw;
            let before = std::fs::read(&path).unwrap();
            assert!(
                super::super::rollback::rollback_transaction(
                    &mut raw,
                    saved,
                    &std::sync::atomic::AtomicBool::new(false)
                )
                .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        } else {
            assert_eq!(
                result.unwrap()[0].kind,
                RecoveryIssueKind::DirectorySyncFailed
            );
            assert_eq!(raw.entries[0].state, EntryState::Applying);
            let mut reloaded = super::super::journal::read_journal(&path).unwrap();
            assert_eq!(
                reconcile_recovery(&mut reloaded, saved).unwrap()[0].kind,
                RecoveryIssueKind::DirectorySyncFailed
            );
            assert_eq!(reloaded.entries[0].state, EntryState::Applying);
        }
        assert_eq!(std::fs::read(moved.join("b")).unwrap(), b"original bytes");
    }

    #[test]
    #[cfg(unix)]
    fn residual_normal_recovery_and_reload_still_work() {
        let (_d, mut tx, journal) = residual_fixture();
        assert_eq!(
            reconcile_recovery(&mut tx, &journal).unwrap()[0].kind,
            RecoveryIssueKind::RenameConfirmed
        );
        assert_eq!(tx.state, TransactionState::Applied);
        let path = super::super::journal::journal_path(&journal, &tx.transaction_id).unwrap();
        assert_eq!(super::super::journal::read_journal(&path).unwrap(), tx);
    }
    #[test]
    #[cfg(unix)]
    fn residual_root_swap_during_nested_open_is_refused() {
        let (d, mut tx, journal) = residual_fixture();
        tx.entries[0].source_path = d.path().join("roms/sub/a");
        let root = d.path().join("roms");
        let moved = d.path().join("moved");
        let destination = moved.join("sub/b");
        let mut fired = false;
        super::super::dirsync::set_open_hook(Some(Box::new(move |_| {
            if !fired {
                fired = true;
                std::fs::rename(&root, &moved).unwrap();
                std::os::unix::fs::symlink(&moved, &root).unwrap();
            }
        })));
        let result = reconcile_recovery(&mut tx, &journal);
        super::super::dirsync::set_open_hook(None);
        assert_eq!(
            result.unwrap()[0].kind,
            RecoveryIssueKind::DirectorySyncFailed
        );
        assert_eq!(tx.entries[0].state, EntryState::Applying);
        assert_eq!(std::fs::read(destination).unwrap(), b"original bytes");
    }

    #[test]
    #[cfg(unix)]
    fn residual_preconfigured_symlink_uses_original_resolved_root() {
        let (d, tx, _journal) = residual_fixture();
        let alias = d.path().join("configured-alias");
        let root = d.path().join("roms");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let trusted = crate::safe_read::TrustedRoots::from_paths([&alias]);
        let dirs = super::super::dirsync::RenameDirs::open_in(
            &tx.entries[0].source_path,
            &tx.entries[0].destination_path,
            &trusted,
        )
        .unwrap();
        // Retained resolved-root identity is independent of a later alias change.
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(d.path(), &alias).unwrap();
        dirs.verify_bound().unwrap();
    }
}
