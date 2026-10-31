//! Safe, gated application of explicitly approved rename and linked-library
//! plans, with durable transaction journaling and rollback.
//!
//! This is the apply side of the read-only rename planning (PR #14). It may
//! rename a file **only** when every gate holds:
//!
//! - the proposal comes from the current validated rename plan generation;
//! - the proposal is `Suggested`, actionable, collision-free, and explicitly
//!   approved by the user;
//! - at apply time the source is still the very same regular file that was
//!   reviewed (same content digest, full-precision mtime, size and, where
//!   supported, inode/device; never a symlink);
//! - the destination does not exist, shares the source's directory, and lies
//!   inside the configured trusted roots;
//! - every preflight check passes immediately before the rename.
//!
//! # The hard safety boundary
//!
//! The executor is the **only** place rename/link mutations happen; the GUI
//! never calls `std::fs::rename`. Mutations use a no-clobber primitive
//! (`renameat2(RENAME_NOREPLACE)` on Linux) so an existing destination is
//! never overwritten by an ordinary move. There is no copy+delete fallback; a destination that
//! appears between preflight and rename is refused atomically. A batch is
//! journaled durably before the first mutation and updated after every
//! transition, so a crash leaves a recoverable record. Nothing here runs
//! unattended, retries a failed mutation, or resumes automatically.
//!
//! # Unsupported cases
//!
//! Symlink sources, broken symlinks, directories, archive members, cross-
//! filesystem moves, unreviewed overwrites, and any rename outside the
//! trusted roots are never performed. On platforms without a verified
//! no-clobber primitive (everything but Linux in this build) the executor
//! refuses to mutate rather than risk a TOCTOU-prone exists+rename sequence.
//!
//! Explicit `ReplaceExisting` is a separate reviewed operation: Linux atomic
//! exchange preserves the exact original at the staged source path. Both file
//! identities are journalled and checked for publication, recovery and undo.

pub mod copy;
pub(crate) mod dirsync;
pub mod exact_resume;
pub mod executor;
pub mod history;
pub mod identity;
pub mod journal;
mod lookup;
pub mod model;
pub mod noclobber;
pub mod preflight;
pub mod reconcile;
mod replacement;
pub mod rollback;
pub mod seal;
pub mod writer_gate;

pub use exact_resume::{
    ExactResumeError, ExactResumeExecution, ExactResumeInspection, ExactResumeOutcome,
    ExactResumeRefusal, ExactResumeResult, build_envelope, compute_plan_digest, exact_resume_state,
    inspect_exact_resume, resume_exact_transaction,
};
pub use executor::{
    ApplyError, ApplyExecution, ApplyOutcome, HardConflictMode, apply_transaction,
    build_transaction, build_transaction_entries, is_approved,
};
pub use history::{
    RECOVERY_HISTORY_STATE_FILE, RecoveryCleanupClassification, RecoveryHistoryState,
    StaleRecoveryReason, archive_recovery_transaction, archive_recovery_transactions,
    classify_recovery_cleanup, load_recovery_history_state, recovery_history_state_path,
};
pub use identity::{capture_identity, classify_at, content_identity_matches, identity_matches};
pub use journal::{
    LEGACY_RENAME_TRANSACTIONS_DIRECTORY, LiveTransactionGuard, RENAME_TRANSACTIONS_DIRECTORY,
    default_rename_transaction_dir, find_recovery_transactions, find_rollbackable_transactions,
    is_transaction_live, journal_exists, journal_path, legacy_rename_transaction_dir,
    list_journals, mark_transaction_live, new_transaction_id, read_journal, remove_journal,
    rename_transaction_dir_in, resolve_leave_untouched, write_journal,
};
pub use model::{
    EntryState, ExactResumeEnvelope, ExactResumeOperation, ExactResumeState, ObjectFreshness,
    ObjectIdentity, ObjectKind, RecoveryResolution, RenameTransaction, RollbackResult,
    RollbackStatus, TransactionEntry, TransactionOperation, TransactionState, TransactionSummary,
    UnresolvedEntry,
};
pub use noclobber::{NoClobberError, rename_noreplace};
pub use preflight::{
    DirectoryPolicy, PreflightFailure, PreflightOptions, batch_destinations,
    destination_is_confined, run_preflight,
};
pub use reconcile::{RecoveryIssue, RecoveryIssueKind, reconcile_recovery};
pub use rollback::{
    JOURNAL_INTEGRITY_LOST, RollbackOutcome, abandon_unavailable_links, is_journal_integrity_loss,
    link_unavailable_now, reenter_recovery_reconciliation, rollback_transaction,
    rollback_transaction_confined,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod integrity_audit_tests;
#[cfg(test)]
mod live_reconcile_tests;

/// Read-only evidence inspection; never grants recovery authority.
pub mod compact_recovery;

mod private_paths;
pub(crate) use private_paths::{is_private_recovery_path, is_private_recovery_root};

/// Whether this journal is, or cannot be shown not to be, in the compacted
/// recovery format (written and undone only through its own authority).
#[cfg(target_os = "linux")]
pub(crate) fn uses_compact_recovery_format(
    journal_dir: &std::path::Path,
    transaction_id: &str,
) -> bool {
    compact_recovery::is_enrolled(journal_dir, transaction_id)
}
