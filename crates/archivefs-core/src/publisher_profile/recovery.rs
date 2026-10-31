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

/// Where `root` really is: its resolved path and filesystem identity, or null
/// when that cannot be established. The root may be reached through a symlinked
/// parent, so comparing this, not the path text, shows it is still the same
/// folder.
fn destination_identity(root: &Path) -> serde_json::Value {
    let Ok(real) = std::fs::canonicalize(root) else {
        return serde_json::Value::Null;
    };
    let Ok(metadata) = std::fs::metadata(&real) else {
        return serde_json::Value::Null;
    };
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = {
        let _ = metadata;
        (0_u64, 0_u64)
    };
    match serde_json::to_value(&real) {
        Ok(real_path) => serde_json::json!({"real_path": real_path, "dev": dev, "ino": ino}),
        Err(_) => serde_json::Value::Null,
    }
}

/// The value stamped when a Publisher transaction is built.
pub(super) fn export_marker(
    destination_root: &Path,
    link_mode: PublisherLinkMode,
) -> serde_json::Value {
    serde_json::json!({
        "destination_root": destination_root.to_string_lossy(),
        "destination_identity": destination_identity(destination_root),
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

/// What the journal folder shows: exports to offer, and what could not be shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PublisherRecoveryListing {
    pub exports: Vec<RecoverableExport>,
    /// Marked journals left out because their seal did not verify.
    pub unverified: usize,
    /// Journals too damaged to say what they record.
    pub unparseable: usize,
    /// Why the folder itself could not be read.
    pub unreadable: Option<String>,
}

/// Why a recovered Undo ended without an outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherUndoFailure {
    /// Refused while opening the journal: nothing was touched.
    Refused(String),
    /// Stopped inside rollback, possibly after links were already removed.
    Interrupted(String),
}

/// Whether `transaction` is a Publisher export: the plain workflow, a marker
/// naming the destination root and link mode, the source root Apply requires,
/// created directories inside the root, and every entry the matching link
/// operation confined to that root. Older exports have no marker and are
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
        && Path::new(&transaction.source_scan_root) == root
        && transaction
            .created_directories
            .iter()
            .all(|directory| directory.starts_with(root))
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

/// Entries an earlier rollback refused because the link is no longer what was
/// created. These are never retried, so they need the user's attention rather
/// than another Undo. Links refused only because a folder or device was
/// unavailable stay `Applied` and are retried by the next Undo instead.
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
        let reason = transaction
            .entries
            .iter()
            .find(|entry| entry.state == EntryState::RollbackFailed)
            .and_then(|entry| entry.failure_reason.as_deref())
            .unwrap_or("no reason was recorded");
        return Some(format!(
            "{}: {} link(s) were not undone and were left in place; check them by hand. Recorded reason: {reason}",
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

/// A marker's destination root, for a record `is_publisher_export` accepted.
fn export_root(transaction: &RenameTransaction) -> Option<&Path> {
    transaction
        .unknown
        .get(PUBLISHER_EXPORT_MARKER)?
        .get("destination_root")?
        .as_str()
        .map(Path::new)
}

/// Whether the root still resolves to the folder recorded at export. Rollback
/// checks containment from the root down, so this covers the path above it, and
/// an empty mount point in the folder's place. Unrecorded identity never matches.
fn destination_unchanged(transaction: &RenameTransaction, root: &Path) -> bool {
    let recorded = transaction
        .unknown
        .get(PUBLISHER_EXPORT_MARKER)
        .and_then(|marker| marker.get("destination_identity"));
    let current = destination_identity(root);
    !current.is_null() && recorded == Some(&current)
}

/// Why Undo should wait. Waiting is advice, not protection: a link refused only
/// because a folder was unavailable stays undoable, but trying now cannot finish
/// and says nothing new. A single missing source folder is not an outage, so it
/// stays with the per-entry partial rollback.
pub fn availability_blocker(transaction: &RenameTransaction) -> Option<String> {
    let id = &transaction.transaction_id;
    let root = export_root(transaction)?;
    if !root.is_dir() {
        return Some(format!(
            "{id} cannot be undone right now: {} is not available. Reconnect it, then choose Check again.",
            root.display()
        ));
    }
    if reversible_entries(transaction).all(|entry| {
        entry
            .source_path
            .parent()
            .is_none_or(|parent| !parent.is_dir())
    }) {
        return Some(format!(
            "{id} cannot be undone right now: its source folders are not available. Reconnect them, then choose Check again."
        ));
    }
    (!destination_unchanged(transaction, root)).then(|| {
        format!(
            "{id} cannot be undone right now: {} no longer leads to the folder it was exported to. Reconnect it, then choose Check again.",
            root.display()
        )
    })
}

/// A hardlink has no identity of its own, so a link at a path recorded again by
/// a later journal cannot be told from the later one. Symlinks carry creation
/// proof, so they keep their per-link refusal and partial rollback. A journal
/// that cannot be read may be such a record, so it counts too.
fn superseded_blocker(transaction: &RenameTransaction, records: &JournalRecords) -> Option<String> {
    let hardlinks: Vec<&Path> = reversible_entries(transaction)
        .filter(|entry| matches!(entry.operation, TransactionOperation::CreateHardlink { .. }))
        .map(|entry| entry.destination_path.as_path())
        .collect();
    if !hardlinks.is_empty() && records.unreadable > 0 {
        return Some(format!(
            "{} cannot be undone: {} journal(s) in this folder could not be read and may record the same destination paths.",
            transaction.transaction_id, records.unreadable
        ));
    }
    records
        .readable
        .iter()
        .filter(|other| {
            other.transaction_id != transaction.transaction_id
                && other.created_at_unix >= transaction.created_at_unix
        })
        .any(|other| {
            other.entries.iter().any(|entry| {
                !matches!(
                    entry.state,
                    EntryState::Planned | EntryState::PreflightPassed | EntryState::Skipped
                ) && hardlinks.contains(&entry.destination_path.as_path())
            })
        })
        .then(|| {
            format!(
                "{} cannot be undone: a newer transaction recorded the same destination paths, so these links cannot be told apart.",
                transaction.transaction_id
            )
        })
}

fn export_blocker(transaction: &RenameTransaction, records: &JournalRecords) -> Option<String> {
    undo_blocker(transaction)
        .or_else(|| availability_blocker(transaction))
        .or_else(|| superseded_blocker(transaction, records))
}

/// Every journal in the folder, verified or not, so an unverifiable later
/// record still counts as ambiguity.
struct JournalRecords {
    readable: Vec<RenameTransaction>,
    unreadable: usize,
}

fn journal_records(evidence: &[seal::JournalEvidence]) -> JournalRecords {
    JournalRecords {
        readable: evidence
            .iter()
            .filter_map(|item| item.transaction.clone())
            .collect(),
        unreadable: evidence
            .iter()
            .filter(|item| item.transaction.is_none())
            .count(),
    }
}

/// Marked exports in `journal_dir` that rollback could still act on, or that an
/// earlier rollback left needing attention, newest first. Read-only. Only
/// sealed records in this folder are offered, so legacy folders and
/// unverifiable journals never are; the latter are counted instead.
pub fn recoverable_publisher_exports(journal_dir: &Path) -> PublisherRecoveryListing {
    let unreadable = std::fs::read_dir(journal_dir).err().and_then(|error| {
        (error.kind() != std::io::ErrorKind::NotFound).then(|| error.to_string())
    });
    let evidence = seal::history_evidence(journal_dir, &[]);
    let records = journal_records(&evidence);
    let unverified = evidence
        .iter()
        .filter(|item| !item.status.trusted())
        .filter_map(|item| item.transaction.as_ref())
        .filter(|transaction| transaction.unknown.contains_key(PUBLISHER_EXPORT_MARKER))
        .count();
    let mut exports: Vec<RecoverableExport> = evidence
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
            blocker: export_blocker(&transaction, &records),
            transaction,
        })
        .collect();
    exports.sort_by_key(|export| std::cmp::Reverse(export.transaction.created_at_unix));
    PublisherRecoveryListing {
        exports,
        unverified,
        unparseable: records.unreadable,
        unreadable,
    }
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
    match export_blocker(
        &journal,
        &journal_records(&seal::history_evidence(journal_dir, &[])),
    ) {
        Some(blocker) => Err(blocker),
        None => Ok(journal),
    }
}

/// A plain account of where an Undo stands after `outcome`, from the record it
/// left: what was undone, what a later Undo will retry, and what needs the
/// user. Never says "complete" unless every entry is rolled back.
pub fn describe_undo_outcome(
    transaction: &RenameTransaction,
    outcome: &PlanRollbackOutcome,
) -> String {
    let id = &transaction.transaction_id;
    let count = |state: EntryState| {
        transaction
            .entries
            .iter()
            .filter(|entry| entry.state == state)
            .count()
    };
    let undone = count(EntryState::RolledBack);
    let retryable = reversible_entries(transaction).count();
    let refused = refused_entry_count(transaction);
    if retryable == 0 && refused == 0 {
        return format!("Export {id}: every link was undone.");
    }
    let mut parts = vec![format!(
        "Undo of {id} is not complete: {undone} link(s) undone."
    )];
    if retryable > 0 {
        let why = transaction
            .entries
            .iter()
            .find(|entry| entry.state == EntryState::Applied && entry.failure_reason.is_some())
            .and_then(|entry| entry.failure_reason.as_deref());
        parts.push(match why {
            Some(reason) => format!(
                "{retryable} link(s) were not touched because a folder or device was not available ({reason}). Reconnect it, then roll back again: every check is repeated first."
            ),
            None => format!(
                "{retryable} link(s) are still to undo. Roll back again; every check is repeated first."
            ),
        });
    }
    if refused > 0 {
        let reason = transaction
            .entries
            .iter()
            .find(|entry| entry.state == EntryState::RollbackFailed)
            .and_then(|entry| entry.failure_reason.as_deref())
            .unwrap_or("no reason was recorded");
        parts.push(format!(
            "{refused} link(s) are no longer what this export created and were left in place; check them by hand. Recorded reason: {reason}"
        ));
    }
    if !outcome.directories_remaining.is_empty() {
        parts.push(format!(
            "{} created folder(s) were kept because they are not empty.",
            outcome.directories_remaining.len()
        ));
    }
    parts.join(" ")
}

/// Rolls back exactly the journal of `record` through the shared plan rollback,
/// returning the updated record with the outcome. Trust is rebuilt only from
/// the sealed journal, the same roots the page uses for its held transaction.
pub fn rollback_publisher_export(
    journal_dir: &Path,
    record: &RenameTransaction,
    cancel: &AtomicBool,
) -> Result<(RenameTransaction, PlanRollbackOutcome), PublisherUndoFailure> {
    let mut journal = open_publisher_export_for_undo(journal_dir, record)
        .map_err(PublisherUndoFailure::Refused)?;
    let mut roots = vec![PathBuf::from(&journal.source_scan_root)];
    roots.extend(
        journal
            .entries
            .iter()
            .filter_map(|entry| entry.source_path.parent().map(Path::to_path_buf)),
    );
    let trusted = TrustedRoots::from_paths(roots);
    let outcome = rollback_plan_transaction(&mut journal, journal_dir, cancel, &trusted)
        .map_err(PublisherUndoFailure::Interrupted)?;
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
            "source_scan_root": "/out",
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
    fn a_marked_record_outside_what_apply_builds_is_not_an_export() {
        let marker = export_marker(Path::new("/out"), PublisherLinkMode::Symlink);
        let mut moved = record(symlink("/out"), Some(marker.clone()));
        moved.source_scan_root = "/elsewhere".into();
        assert!(!is_publisher_export(&moved));
        let mut escaping = record(symlink("/out"), Some(marker));
        escaping.created_directories.push("/home/user".into());
        assert!(!is_publisher_export(&escaping));
    }

    #[cfg(unix)]
    #[test]
    fn a_path_json_cannot_hold_has_no_identity_and_does_not_panic() {
        use std::os::unix::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(std::ffi::OsStr::from_bytes(b"bad\xff"));
        std::fs::create_dir(&root).unwrap();
        assert!(destination_identity(&root).is_null());
        let marker = export_marker(&root, PublisherLinkMode::Hardlink);
        assert!(marker["destination_identity"].is_null());
    }

    #[test]
    fn unrecorded_destination_identity_never_matches() {
        let temp = tempfile::tempdir().unwrap();
        let root_text = temp.path().to_str().unwrap();
        let mut marker = export_marker(temp.path(), PublisherLinkMode::Hardlink);
        let mut export = record(hardlink(root_text), Some(marker.clone()));
        assert!(destination_unchanged(&export, temp.path()));
        marker["destination_identity"] = serde_json::Value::Null;
        export
            .unknown
            .insert(PUBLISHER_EXPORT_MARKER.into(), marker);
        assert!(!destination_unchanged(&export, temp.path()));
    }

    #[test]
    fn only_losing_every_source_folder_counts_as_an_outage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        std::fs::create_dir(&root).unwrap();
        let root_text = root.to_str().unwrap();
        let mut gone = record(
            hardlink(root_text),
            Some(export_marker(&root, PublisherLinkMode::Hardlink)),
        );
        gone.source_scan_root = root_text.into();
        gone.entries[0].source_path = temp.path().join("offline/a");
        assert!(
            availability_blocker(&gone)
                .unwrap()
                .contains("source folders")
        );
        let mut partial = gone.clone();
        let mut present = partial.entries[0].clone();
        present.source_path = temp.path().join("a");
        partial.entries.push(present);
        assert_eq!(availability_blocker(&partial), None);
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
