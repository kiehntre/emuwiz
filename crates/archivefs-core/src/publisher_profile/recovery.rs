//! Durable Undo for Publisher exports. A Publisher transaction carries a sealed
//! typed marker, so its journal alone can reopen rollback after Preview plan or
//! a restart. This path is separate from Repair History's shared classifier and
//! adds no rollback logic: `rollback_plan_transaction` re-verifies the seal,
//! fences writes and refuses links that no longer match what was created.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::dat::rename_apply::model::{EntryState, TransactionOperation};
use crate::dat::rename_apply::{
    RenameTransaction, TransactionEntry, journal_path, read_journal, seal,
};
use crate::platform_evidence_fusion::plan_transaction::{
    PlanRollbackOutcome, rollback_plan_transaction,
};
use crate::safe_read::TrustedRoots;

use super::execution::PublisherLinkMode;

/// The transaction-level `unknown` key marking a Publisher export. It is part
/// of the sealed record, so it cannot be added to or removed from a journal
/// without breaking the seal.
pub const PUBLISHER_EXPORT_MARKER: &str = "publisher_export_v1";

const MODE_HARDLINK: &str = "hardlink";
const MODE_SYMLINK: &str = "symlink";

/// The value stamped when a Publisher transaction is built.
pub(super) fn export_marker(
    destination_root: &Path,
    link_mode: PublisherLinkMode,
) -> serde_json::Value {
    serde_json::json!({
        "destination_root": destination_root,
        "link_mode": match link_mode {
            PublisherLinkMode::Hardlink => MODE_HARDLINK,
            PublisherLinkMode::Symlink => MODE_SYMLINK,
        },
    })
}

/// A marked export the journal says rollback can still act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableExport {
    pub transaction: RenameTransaction,
    pub item_count: usize,
    /// Why Undo is not offered, when the record is short of evidence.
    pub blocker: Option<String>,
}

/// Whether `transaction` is a Publisher export: the plain workflow, a marker
/// naming the destination root and link mode, and every entry the matching
/// link operation confined to that root. Older exports have no marker and are
/// indistinguishable from other link records, so they never qualify.
pub fn is_publisher_export(transaction: &RenameTransaction) -> bool {
    let Some(marker) = transaction.unknown.get(PUBLISHER_EXPORT_MARKER) else {
        return false;
    };
    let (Some(root), Some(mode)) = (
        marker.get("destination_root").and_then(|v| v.as_str()),
        marker.get("link_mode").and_then(|v| v.as_str()),
    ) else {
        return false;
    };
    let root = Path::new(root);
    !transaction.entries.is_empty()
        && seal::workflow_of(transaction) == "rename_apply"
        && transaction
            .entries
            .iter()
            .all(|entry| match &entry.operation {
                TransactionOperation::CreateHardlink {
                    destination_root, ..
                } => mode == MODE_HARDLINK && destination_root == root,
                TransactionOperation::CreateSymlink {
                    destination_root, ..
                } => mode == MODE_SYMLINK && destination_root == root,
                _ => false,
            })
}

/// Entries rollback would act on: applied, or in flight and reconciled.
fn reversible_entries(transaction: &RenameTransaction) -> impl Iterator<Item = &TransactionEntry> {
    transaction.entries.iter().filter(|entry| {
        matches!(
            entry.state,
            EntryState::Applied | EntryState::Applying | EntryState::RollingBack
        )
    })
}

/// Entries an earlier rollback refused. Links are never retried, so these need
/// the user's attention rather than another Undo.
fn refused_entry_count(transaction: &RenameTransaction) -> usize {
    transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::RollbackFailed)
        .count()
}

/// Why Undo must not be offered, or `None` when the record supports it. The
/// shared rollback refuses these too; saying so here means no one is asked to
/// confirm a rollback that cannot run.
fn undo_blocker(transaction: &RenameTransaction) -> Option<String> {
    if transaction.is_rollbackable()
        && refused_entry_count(transaction) > 0
        && reversible_entries(transaction).next().is_none()
    {
        return Some(format!(
            "{}: {} link(s) could not be undone because they are no longer what EmuWiz created. They were left in place; check them by hand.",
            transaction.transaction_id,
            refused_entry_count(transaction)
        ));
    }
    if !transaction.is_rollbackable() || reversible_entries(transaction).next().is_none() {
        return Some(format!(
            "{} has nothing left to roll back.",
            transaction.transaction_id
        ));
    }
    // A symlink is owned through its recorded creation proof. A hardlink shares
    // its source's identity, which every entry records and rollback compares.
    reversible_entries(transaction)
        .any(|entry| {
            matches!(entry.operation, TransactionOperation::CreateSymlink { .. })
                && entry.created_link_identity().is_none()
        })
        .then(|| {
            format!(
                "{} cannot be undone: its journal has no proof of what it created, so its links cannot be told from your own files.",
                transaction.transaction_id
            )
        })
}

/// Marked exports in `journal_dir` that rollback could still act on, or that an
/// earlier rollback left needing attention, newest first. Read-only. Only sealed records in this folder are considered, so
/// legacy folders and unverifiable journals are never offered.
pub fn recoverable_publisher_exports(journal_dir: &Path) -> Vec<RecoverableExport> {
    let mut exports: Vec<RecoverableExport> = seal::history_evidence(journal_dir, &[])
        .into_iter()
        .filter(|item| item.status.trusted())
        .filter_map(|item| {
            item.transaction
                .filter(|transaction| transaction.transaction_id == item.file_id)
        })
        .filter(is_publisher_export)
        .filter(|transaction| {
            transaction.is_rollbackable()
                && (reversible_entries(transaction).next().is_some()
                    || refused_entry_count(transaction) > 0)
        })
        .map(|transaction| RecoverableExport {
            item_count: reversible_entries(&transaction).count(),
            blocker: undo_blocker(&transaction),
            transaction,
        })
        .collect();
    exports.sort_by_key(|export| std::cmp::Reverse(export.transaction.created_at_unix));
    exports
}

/// Loads the exact journal of `record` for Undo, and returns it only when all
/// of these hold: it is not compact-format; its independent seal verifies; it
/// records the same id; it is a Publisher export; it equals `record` by seal
/// digest; and it has proof of what it created. Read-only.
pub fn open_publisher_export_for_undo(
    journal_dir: &Path,
    record: &RenameTransaction,
) -> Result<RenameTransaction, String> {
    let id = record.transaction_id.as_str();
    let path = journal_path(journal_dir, id)
        .ok_or_else(|| format!("{id} is not a valid transaction id."))?;
    #[cfg(target_os = "linux")]
    if crate::dat::rename_apply::uses_compact_recovery_format(journal_dir, id) {
        return Err(format!(
            "{id} uses, or cannot be shown to avoid, the compacted recovery format, so it was not opened for Undo."
        ));
    }
    let check = seal::verify(journal_dir, id);
    if !check.trusted() {
        return Err(check.refusal().unwrap_or_else(|| {
            format!("The journal for {id} could not be verified, so it was not opened for Undo.")
        }));
    }
    let journal = read_journal(&path)
        .map_err(|error| format!("The journal for {id} could not be read: {error}"))?;
    if journal.transaction_id != id {
        return Err(format!(
            "The journal found for {id} records {} instead.",
            journal.transaction_id
        ));
    }
    if !is_publisher_export(&journal) {
        return Err(format!("{id} is not a Publisher export."));
    }
    if seal::digest(&journal) != seal::digest(record) {
        return Err(format!(
            "The journal for {id} no longer matches this record. It was not opened for Undo."
        ));
    }
    match undo_blocker(&journal) {
        Some(blocker) => Err(blocker),
        None => Ok(journal),
    }
}

/// Rolls back exactly the journal of `record` through the shared plan rollback,
/// returning the updated record with the outcome. Trust is rebuilt only from
/// the sealed journal, the same roots the page uses for its held transaction.
pub fn rollback_publisher_export(
    journal_dir: &Path,
    record: &RenameTransaction,
    cancel: &AtomicBool,
) -> Result<(RenameTransaction, PlanRollbackOutcome), String> {
    let mut journal = open_publisher_export_for_undo(journal_dir, record)?;
    let mut roots = vec![PathBuf::from(&journal.source_scan_root)];
    roots.extend(
        journal
            .entries
            .iter()
            .filter_map(|entry| entry.source_path.parent().map(Path::to_path_buf)),
    );
    let trusted = TrustedRoots::from_paths(roots);
    let outcome = rollback_plan_transaction(&mut journal, journal_dir, cancel, &trusted)?;
    Ok((journal, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::rename_apply::TransactionState;

    fn record(
        operation: TransactionOperation,
        marker: Option<serde_json::Value>,
    ) -> RenameTransaction {
        let mut transaction: RenameTransaction = serde_json::from_value(serde_json::json!({
            "transaction_id": "1-0",
            "plan_generation": 1,
            "created_at_unix": 0,
            "source_scan_root": "/src",
            "state": "applied",
            "entries": [],
        }))
        .unwrap();
        let mut entry: TransactionEntry = serde_json::from_value(serde_json::json!({
            "source_path": "/src/a",
            "destination_path": "/out/a",
            "original_basename": "a",
            "proposed_basename": "a",
            "identity": {"size_bytes": 0, "modified_unix": 0, "kind": "regular_file"},
        }))
        .unwrap();
        entry.operation = operation;
        entry.state = EntryState::Applied;
        transaction.entries.push(entry);
        if let Some(marker) = marker {
            transaction
                .unknown
                .insert(PUBLISHER_EXPORT_MARKER.into(), marker);
        }
        transaction
    }

    fn symlink(root: &str) -> TransactionOperation {
        TransactionOperation::CreateSymlink {
            expected_target: "/src/a".into(),
            destination_root: root.into(),
        }
    }

    fn hardlink(root: &str) -> TransactionOperation {
        TransactionOperation::CreateHardlink {
            expected_source: "/src/a".into(),
            destination_root: root.into(),
        }
    }

    #[test]
    fn only_a_matching_marker_identifies_an_export() {
        let root = Path::new("/out");
        let marker = export_marker(root, PublisherLinkMode::Symlink);
        assert!(is_publisher_export(&record(
            symlink("/out"),
            Some(marker.clone())
        )));
        for (name, transaction) in [
            ("no marker", record(symlink("/out"), None)),
            (
                "other root",
                record(symlink("/elsewhere"), Some(marker.clone())),
            ),
            ("other mode", record(hardlink("/out"), Some(marker.clone()))),
            (
                "rename",
                record(TransactionOperation::RenameMove, Some(marker.clone())),
            ),
            (
                "malformed marker",
                record(symlink("/out"), Some(serde_json::json!(true))),
            ),
        ] {
            assert!(!is_publisher_export(&transaction), "{name}");
        }
        let mut empty = record(symlink("/out"), Some(marker.clone()));
        empty.entries.clear();
        assert!(!is_publisher_export(&empty), "no entries");
        let mut foreign = record(symlink("/out"), Some(marker));
        foreign
            .unknown
            .insert("workflow".into(), serde_json::json!("other"));
        assert!(!is_publisher_export(&foreign), "foreign workflow");
    }

    #[test]
    fn a_symlink_without_creation_proof_blocks_undo_but_a_hardlink_does_not() {
        let root = Path::new("/out");
        let linked = record(
            symlink("/out"),
            Some(export_marker(root, PublisherLinkMode::Symlink)),
        );
        assert!(undo_blocker(&linked).unwrap().contains("no proof"));
        let hard = record(
            hardlink("/out"),
            Some(export_marker(root, PublisherLinkMode::Hardlink)),
        );
        assert_eq!(undo_blocker(&hard), None);
        let mut finished = hard;
        finished.state = TransactionState::RolledBack;
        assert!(undo_blocker(&finished).unwrap().contains("nothing left"));
    }
}
