//! Collection reconciliation: what happened to library records whose file is
//! not where the library last saw it.
//!
//! A library row is keyed by its path. When a file is moved or renamed
//! outside EmuWiz the scanner adds a row for the new path and flags the old
//! one "file not found"; nothing connects the two. This module connects them
//! from evidence, without changing anything:
//!
//! * three sets of records that never overlap: rows flagged missing, rows
//!   that are not flagged but whose file is absent, and rows removed by
//!   Forget Missing (read from its export);
//! * graded evidence, strongest first ([`EvidenceTier`]): EmuWiz's own move
//!   record plus the same inode, the same file object by filesystem record,
//!   a metadata match, and weaker leads that are shown and never accepted;
//! * every symbolic link in the library folders, counted apart from records.
//!
//! It is read-only. It opens the library database read-only, lists folders
//! with metadata calls only, and reads file contents only when asked to
//! compare a file with a hash recorded for its old path. It writes nothing:
//! no record, flag, link or game file is changed, and nothing is kept between
//! runs.
//!
//! Identity, not content: nothing below the confirmed tiers says two files
//! hold the same bytes. A matching name, size and date is recorded as exactly
//! that. Design notes, the investigation that produced the rules and the
//! planned scan-time file identity are in `docs/COLLECTION_RECONCILIATION.md`.

mod inventory;
mod links;
mod model;
mod reconcile;
mod sources;
#[cfg(test)]
mod tests;

pub use inventory::{
    DEFAULT_MAX_ENTRIES, Entry, EntryKind, Inventory, InventoryError, InventoryRequest,
    take_inventory,
};
pub use links::audit_links;
pub use model::*;
pub use reconcile::{
    ContentVerdict, ContentVerifier, CurrentRow, JournalMove, OldRecord, ReconcileInputs,
    RecordedHashes, ScanRun, classify_records, looks_like_game_file,
};
pub use sources::{Progress, ReconcileError, ReconcileRequest, reconcile};
