//! Rollback of applied rename transactions.
//!
//! Rollback reverses only entries the filesystem confirmed as Applied, in the
//! reverse order they were applied. Every step re-verifies that the destination
//! is still the recorded object and that the original source path is free,
//! renames back with the no-clobber primitive, and confirms the source is
//! restored before marking the entry RolledBack. A rollback never claims a full
//! rollback for a partial one, and a repeated rollback request is a safe no-op
//! for entries already rolled back. Cancellation stops the loop with any
//! not-yet-reversed Applied entries left untouched (and retryable); it is an
//! incomplete rollback, never reported as a full one.
//!
//! An entry whose undo was refused is recorded `RollbackFailed` (needs review)
//! with its reason. A cleanly refused entry does not stop the pass: each later
//! entry is still guarded by its own identity check immediately before its
//! no-overwrite mutation (and batch preflight refused any destination that
//! already existed or repeated), so continuing can only restore an entry that
//! proves it is still the recorded object, or refuse it. A failure after the
//! object has already moved, or any replacement entry, still stops the pass.
//!
//! A later request retries a needs-review move only after fresh read-only
//! validation, against the journal as found before the pass writes it, proves
//! the journal still records that exact operation, both parent directories are
//! plain directories, the original path is free, and the moved object is still
//! at its recorded location, is not a directory and keeps its recorded
//! identity. The retry is the same bound-directory no-overwrite reverse rename
//! as a first undo, re-checked immediately before it; it never deletes.
//!
//! Links (symbolic and hard) and copies are never retried. Their checks still
//! run so the refusal names the real blocker, but removal goes by path name and
//! cannot yet exclude an unrelated object swapped in between check and removal
//! (a first undo keeps that existing exposure; a retry must not add to it).
//! Replacements are separate recovery work. Any refusal leaves the entry and
//! the filesystem unchanged and reports the exact blocker.
//!
//! Every Undo first rereads the journal and requires it to be a plain file
//! equal to this session's record before anything is reconciled, moved,
//! deleted or written; every later write in the pass is fenced against the
//! bytes it validated or last wrote. A changed, replaced, missing or unreadable
//! journal refuses the Undo with `JOURNAL_INTEGRITY_LOST` and is left
//! byte-for-byte unchanged; callers quarantine that Undo handle.
//!
//! MAME rebuild and internal repair follow the same contract. Rebuild keeps
//! its staging lock (which refuses a journal that differs from the one held),
//! and every workflow verifies the independent journal seal first.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::identity::{capture_identity, content_identity_matches, identity_matches};
use super::journal::checkpoint_journal as write_journal;
use super::model::{
    EntryState, RenameTransaction, RollbackResult, TransactionOperation, TransactionState,
};
use super::noclobber::NoClobberError;
use crate::safe_read::TrustedRoots;

/// Internal marker: the reverse rename happened but its directory sync failed.
const IN_FLIGHT_MARK: &str = "\u{1}in-flight:";
/// Internal marker: the reverse rename happened but the restored object could
/// not be confirmed. Treated as in-flight.
const MOVED_MARK: &str = "\u{1}moved:";

/// The outcome of a rollback pass (fully / partially / failed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackOutcome {
    pub result: RollbackResult,
    pub transaction: RenameTransaction,
}

/// Rolls back every Applied entry of `transaction`, in reverse order.
///
/// `journal_dir` is where the transaction's journal lives; it is updated
/// durably after every transition. Idempotent: entries already RolledBack (or
/// never Applied) are skipped, so calling this twice never renames anything
/// twice, and a transaction already RolledBack returns a no-op full result.
///
/// Cancellation stops the loop without marking the untouched Applied entries as
/// failed, so they stay eligible for a later rollback. Because the rollback is
/// then incomplete, the result is never `FullyRolledBack` and the transaction
/// is not persisted as `RolledBack`; the leftover entries are reported with a
/// "rollback cancelled" reason.
pub fn rollback_transaction(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<RollbackOutcome, String> {
    rollback_transaction_inner(transaction, journal_dir, cancel, None, false)
}

/// Identical to [`rollback_transaction`], but additionally re-verifies, for
/// every reverse-rename mutation, that the entire ancestor chain of both the
/// current (destination) and restored (source) paths, not just the leaf, is
/// still plain directories confined to `trusted`, immediately before the
/// rename syscall. This closes a gap ordinary `rollback_transaction` does
/// not cover: an intermediate ancestor directory silently replaced with a
/// symlink (e.g. `source_parent` itself) is transparent to normal path
/// resolution, so an unconfined rollback would follow it like any other
/// directory. Used by [`crate::platform_evidence_fusion::plan_transaction`],
/// which already threads a [`TrustedRoots`] through the forward apply path
/// and is the entry point a future real-canary apply will use.
pub fn rollback_transaction_confined(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
    cancel: &AtomicBool,
    trusted: &TrustedRoots,
) -> Result<RollbackOutcome, String> {
    rollback_transaction_inner(transaction, journal_dir, cancel, Some(trusted), false)
}

pub(crate) fn rollback_retrodeck_links(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
    cancel: &AtomicBool,
    trusted: &TrustedRoots,
) -> Result<RollbackOutcome, String> {
    crate::launch::es_de_publish::operation::validate_child(journal_dir, transaction)?;
    rollback_transaction_inner(transaction, journal_dir, cancel, Some(trusted), true)
}

/// The one refusal text for a journal whose Undo was surrendered.
pub(crate) fn undo_surrendered_message(state: TransactionState) -> String {
    if state == TransactionState::Finalised {
        "this replacement was finalised: its original was deleted and Undo is no longer available"
            .into()
    } else {
        "a Finalise Replacement was interrupted: Undo is unavailable until it is completed or cancelled in the MAME rebuild page".into()
    }
}

fn rollback_transaction_inner(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
    cancel: &AtomicBool,
    trusted: Option<&TrustedRoots>,
    coordinated: bool,
) -> Result<RollbackOutcome, String> {
    if !coordinated {
        crate::launch::es_de_publish::operation::require_independent(journal_dir, transaction)?;
    }
    // No Undo while an older, unsealing EmuWiz could rewrite this journal.
    super::writer_gate::check()?;
    // Undo was surrendered on purpose (Finalise Replacement). This is judged
    // on the caller's copy here and again against the journal on disk by the
    // MAME lock below; neither can be bypassed by a stale copy.
    if transaction.state.undo_surrendered() {
        return Err(undo_surrendered_message(transaction.state));
    }
    let _live = super::journal::mark_transaction_live(journal_dir, &transaction.transaction_id);
    let mame_lock = crate::dat::mame_merged_reconstruction::staging_lifecycle::lock_undo(
        transaction,
        journal_dir,
    )?;
    // MAME rebuild holds its staging lock for the whole pass; retry and
    // continuation use the same guarded validation as every other workflow.
    let _mame_lock = mame_lock;
    #[cfg(target_os = "linux")]
    if let Some(result) = super::compact_recovery::fd_undo(journal_dir, transaction, cancel) {
        return result;
    }
    // Every Undo first verifies the journal on disk against its independent
    // seal, and then fences every write of the pass: a changed, replaced,
    // missing or unreadable journal is never overwritten from memory.
    // Compact-enrolled journals are written only through their own
    // checkpoint authority.
    #[cfg(target_os = "linux")]
    let enrolled = super::compact_recovery::is_enrolled(journal_dir, &transaction.transaction_id);
    #[cfg(not(target_os = "linux"))]
    let enrolled = false;
    let mut fence = if !enrolled {
        JournalFence::validate(transaction, journal_dir)?
    } else {
        None
    };
    #[cfg(test)]
    AFTER_JOURNAL_VALIDATION.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    if let Some(fence) = &fence {
        fence.check()?;
    }
    if transaction.state == TransactionState::RolledBack {
        // A `RolledBack` journal is a finished rollback only when its entries
        // agree. One that still records an applied, in-flight or failed-undo
        // entry is contradictory: report it, change nothing on disk and leave
        // the journal exactly as it was found for manual review.
        let contradicting: Vec<(std::path::PathBuf, String)> = transaction
            .entries
            .iter()
            .filter(|entry| entry.blocks_full_rollback())
            .map(|entry| {
                (
                    entry.source_path.clone(),
                    format!(
                        "the journal says rolled back but this entry is recorded as \"{}\"{}; \
                         manual review required",
                        entry.state.label(),
                        entry
                            .failure_reason
                            .as_deref()
                            .map(|reason| format!(" ({reason})"))
                            .unwrap_or_default()
                    ),
                )
            })
            .collect();
        return Ok(RollbackOutcome {
            result: if contradicting.is_empty() {
                RollbackResult::FullyRolledBack
            } else {
                RollbackResult::RollbackFailed {
                    failed: contradicting,
                }
            },
            transaction: transaction.clone(),
        });
    }

    // Reconcile any in-flight entries first: a crash can have left an entry
    // `Applying` (rename may or may not have run) or `RollingBack` (reverse
    // rename may or may not have run). The filesystem decides which. An entry
    // that cannot be cleanly classified stays unresolved and blocks the
    // rollback from claiming completion.
    let issues = super::reconcile::reconcile_owned(transaction, journal_dir)
        .map_err(|error| error.to_string())?;
    if fence.is_some() {
        // Reconciliation persists only this session's record, if anything: the
        // journal must still equal it before this pass writes again.
        fence = JournalFence::validate(transaction, journal_dir)?;
    }
    let unresolved = issues
        .iter()
        .filter(|issue| {
            !matches!(
                issue.kind,
                super::reconcile::RecoveryIssueKind::RenameDidNotHappen
                    | super::reconcile::RecoveryIssueKind::RenameConfirmed
            )
        })
        .count();
    if unresolved > 0 {
        let detail = issues
            .iter()
            .filter(|issue| {
                matches!(
                    issue.kind,
                    super::reconcile::RecoveryIssueKind::BothSourceAndDestination
                        | super::reconcile::RecoveryIssueKind::DirectorySyncFailed
                        | super::reconcile::RecoveryIssueKind::EvidenceUnavailable
                        | super::reconcile::RecoveryIssueKind::BothAbsent
                        | super::reconcile::RecoveryIssueKind::DestinationIdentityChanged
                        | super::reconcile::RecoveryIssueKind::SourceIdentityChanged
                )
            })
            .map(|issue| format!("{}: {}", issue.kind.label(), issue.detail))
            .collect::<Vec<_>>()
            .join("; ");
        transaction.state = TransactionState::RollbackFailed;
        persist(journal_dir, transaction, &mut fence)?;
        return Ok(RollbackOutcome {
            result: RollbackResult::RollbackFailed {
                failed: vec![(
                    transaction
                        .entries
                        .first()
                        .map(|e| e.source_path.clone())
                        .unwrap_or_default(),
                    format!("manual review required: {detail}"),
                )],
            },
            transaction: transaction.clone(),
        });
    }

    // The fence proved the journal equals this record, so retries are
    // validated against it. Without a fence (compact-enrolled) no
    // needs-review entry is retried automatically.
    let review_journal = fence.is_some().then(|| Ok(transaction.clone()));

    transaction.state = TransactionState::RollingBack;
    persist(journal_dir, transaction, &mut fence)?;

    let mut rolled_back: Vec<std::path::PathBuf> = Vec::new();
    let mut failed: Vec<(std::path::PathBuf, String)> = Vec::new();
    // Entries already reported by this pass, by index (several entries may
    // share a source path).
    let mut reported = std::collections::BTreeSet::new();
    let mut stopped_by_cancellation = false;

    // Reverse order of successful operations only.
    for index in (0..transaction.entries.len()).rev() {
        if cancelled(cancel) {
            stopped_by_cancellation = true;
            break;
        }
        if !transaction.entries[index].is_eligible_for_rollback() {
            if !is_review_retry_candidate(&transaction.entries[index]) {
                continue;
            }
            if let Err(blocker) = guarded_review_retry(transaction, index, review_journal.as_ref())
            {
                let entry = &transaction.entries[index];
                reported.insert(index);
                failed.push((
                    entry.source_path.clone(),
                    format!(
                        "needs review: {blocker}; nothing was changed (earlier undo attempt: {})",
                        entry
                            .failure_reason
                            .as_deref()
                            .unwrap_or("an earlier undo of this entry failed")
                    ),
                ));
                continue;
            }
        }

        let catalogue =
            crate::database::organisation_paths::begin(&transaction.entries[index], true, false)?;
        // Durable `RollingBack` checkpoint BEFORE the reverse rename syscall.
        transaction.entries[index].state = EntryState::RollingBack;
        persist(journal_dir, transaction, &mut fence)?;

        // The reverse rename must have durable directory entries before
        // `RolledBack` is recorded. A sync failure leaves the entry in flight
        // (`RollingBack`): the file may already be back at the source, so it
        // is neither retried nor reported as unchanged; a later reconcile
        // decides from the filesystem and re-attempts the sync first.
        let attempted = rollback_mutation(&transaction.entries[index], trusted);
        match attempted {
            Ok(()) => {
                if let Some(catalogue) = catalogue {
                    catalogue.commit()?;
                }
                transaction.entries[index].state = EntryState::RolledBack;
                transaction.entries[index].rolled_back_at_unix =
                    Some(crate::dat::sources::now_unix());
                transaction.entries[index].failure_reason = None;
                rolled_back.push(transaction.entries[index].source_path.clone());
                #[cfg(test)]
                AFTER_REVERSAL.with(|hook| {
                    if let Some(hook) = hook.borrow_mut().as_mut() {
                        hook(rolled_back.len());
                    }
                });
            }
            Err(reason) => {
                let in_flight =
                    reason.starts_with(IN_FLIGHT_MARK) || reason.starts_with(MOVED_MARK);
                let reason = reason
                    .trim_start_matches(IN_FLIGHT_MARK)
                    .trim_start_matches(MOVED_MARK)
                    .to_string();
                transaction.entries[index].state = if in_flight
                    || matches!(
                        transaction.entries[index].operation,
                        TransactionOperation::ReplaceExisting { .. }
                    ) {
                    EntryState::RollingBack
                } else {
                    EntryState::RollbackFailed
                };
                transaction.entries[index].failure_reason = Some(reason.clone());
                reported.insert(index);
                failed.push((transaction.entries[index].source_path.clone(), reason));
                // An in-flight or replacement failure stops the pass; any other
                // refusal lets the independent remaining entries continue.
                if !is_review_retry_candidate(&transaction.entries[index]) {
                    persist(journal_dir, transaction, &mut fence)?;
                    break;
                }
            }
        }
        persist(journal_dir, transaction, &mut fence)?;
    }

    // A rollback is complete only when no entry is left Applied or in-flight
    // (`RollingBack`; unresolved recovery states already blocked the pass
    // above). Cancellation leaves the untouched entries Applied rather than
    // marking them failed, so a later retry can finish them - and it must never
    // be read as a full rollback.
    let remaining_incomplete: Vec<std::path::PathBuf> = transaction
        .entries
        .iter()
        .filter(|entry| matches!(entry.state, EntryState::Applied | EntryState::RollingBack))
        .map(|entry| entry.source_path.clone())
        .collect();

    // An entry still awaiting review (one this pass refused, or did not reach
    // because it stopped) is still
    // unresolved: its recorded state and reason stay in the journal untouched
    // and are reported again here. Without this a repeated request found
    // nothing to process and was recorded as a full rollback.
    for (index, entry) in transaction.entries.iter().enumerate() {
        let already_reported = reported.contains(&index);
        if entry.state == EntryState::RollbackFailed && !already_reported {
            let earlier = entry
                .failure_reason
                .as_deref()
                .unwrap_or("an earlier undo of this entry failed");
            failed.push((
                entry.source_path.clone(),
                if fence.is_some() && is_review_retry_candidate(entry) {
                    format!(
                        "{earlier} (not reached by this Undo; a later Undo retries it only \
                         after fresh checks)"
                    )
                } else {
                    format!(
                        "{earlier} (recorded by an earlier undo attempt and not retried; manual \
                         review required)"
                    )
                },
            ));
        }
    }

    let fully_rolled_back = failed.is_empty() && remaining_incomplete.is_empty();

    transaction.state = if fully_rolled_back {
        TransactionState::RolledBack
    } else {
        TransactionState::RollbackFailed
    };
    persist(journal_dir, transaction, &mut fence)?;

    let result = if fully_rolled_back {
        RollbackResult::FullyRolledBack
    } else if stopped_by_cancellation {
        let mut cancelled_remaining: Vec<(std::path::PathBuf, String)> = remaining_incomplete
            .iter()
            .map(|path| {
                (
                    path.clone(),
                    "rollback cancelled while entries were still applied".to_string(),
                )
            })
            .collect();
        cancelled_remaining.extend(failed);
        if rolled_back.is_empty() {
            RollbackResult::RollbackFailed {
                failed: cancelled_remaining,
            }
        } else {
            RollbackResult::PartiallyRolledBack {
                rolled_back,
                failed: cancelled_remaining,
            }
        }
    } else if rolled_back.is_empty() {
        RollbackResult::RollbackFailed { failed }
    } else {
        RollbackResult::PartiallyRolledBack {
            rolled_back,
            failed,
        }
    };

    Ok(RollbackOutcome {
        result,
        transaction: transaction.clone(),
    })
}

/// A cleanly refused, non-replacement entry: the pass continues around it and a
/// later pass may retry it after fresh validation.
fn is_review_retry_candidate(entry: &super::model::TransactionEntry) -> bool {
    entry.state == EntryState::RollbackFailed
        && !matches!(
            entry.operation,
            TransactionOperation::ReplaceExisting { .. }
        )
}

/// Fresh validation before a needs-review entry is retried. Read-only: returns
/// the exact blocker and changes nothing when any check fails. The retry itself
/// re-checks everything immediately before its guarded mutation.
fn guarded_review_retry(
    transaction: &RenameTransaction,
    index: usize,
    recorded: Option<&Result<RenameTransaction, String>>,
) -> Result<(), String> {
    let entry = &transaction.entries[index];
    let recorded = match recorded {
        Some(Ok(recorded)) => recorded,
        Some(Err(blocker)) => return Err(blocker.clone()),
        None => return Err("the journal was not inspected for this retry".into()),
    };
    let agrees = recorded.transaction_id == transaction.transaction_id
        && recorded.entries.get(index).is_some_and(|recorded| {
            recorded.state == EntryState::RollbackFailed
                && recorded.source_path == entry.source_path
                && recorded.destination_path == entry.destination_path
                && recorded.identity == entry.identity
                && recorded.operation == entry.operation
        });
    if !agrees {
        return Err("the journal no longer records this entry as awaiting review".into());
    }
    let parents: &[&std::path::Path] = match entry.operation {
        TransactionOperation::RenameMove => &[&entry.source_path, &entry.destination_path],
        _ => &[&entry.destination_path],
    };
    for path in parents {
        let plain_parent = path.parent().is_some_and(|parent| {
            std::fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.file_type().is_dir())
        });
        if !plain_parent {
            return Err(format!(
                "the folder containing {} is missing or is no longer a plain directory",
                path.display()
            ));
        }
    }
    match &entry.operation {
        TransactionOperation::RenameMove => guarded_move_retry(entry),
        // Links are checked so the refusal names the real blocker, but are
        // never retried: removal by path cannot yet exclude an unrelated object
        // swapped in between check and removal.
        TransactionOperation::CreateSymlink {
            expected_target,
            destination_root,
        } => Err(link_not_retried(guarded_symlink_retry(
            entry,
            expected_target,
            destination_root,
        ))),
        TransactionOperation::CreateHardlink {
            expected_source,
            destination_root,
        } => Err(link_not_retried(guarded_hardlink_retry(
            entry,
            expected_source,
            destination_root,
        ))),
        TransactionOperation::CreateCopy { .. } => Err(
            "a retry is not supported for copies: the receipt proves the source and the copied \
             content, not that the file now at the destination is the copy this transaction made"
                .into(),
        ),
        TransactionOperation::ReplaceExisting { .. } => {
            Err("a replacement undo is not retried automatically".into())
        }
    }
}

fn link_not_retried(check: Result<(), String>) -> String {
    let limitation = "links are not retried automatically: removing a link by path cannot yet \
                      exclude an unrelated object substituted at that path";
    match check {
        Ok(()) => format!("the link still matches its record, but {limitation}"),
        Err(blocker) => format!("{blocker}; {limitation}"),
    }
}

fn guarded_move_retry(entry: &super::model::TransactionEntry) -> Result<(), String> {
    match std::fs::symlink_metadata(&entry.source_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err("the original path is still occupied".into()),
        Err(error) => return Err(format!("the original path cannot be inspected ({error})")),
    }
    let metadata = std::fs::symlink_metadata(&entry.destination_path)
        .map_err(|_| "the moved file is no longer at its recorded location".to_string())?;
    if metadata.file_type().is_dir() {
        return Err("the recorded location is now a directory; restoring could merge".into());
    }
    let current = capture_identity(&entry.destination_path)
        .map_err(|_| "the moved file at its recorded location cannot be identified".to_string())?;
    if !identity_matches(&entry.identity, &current) {
        return Err(
            "the file at the recorded location is no longer the one that was moved (identity changed)"
                .into(),
        );
    }
    Ok(())
}

/// What occupies a link's recorded path, for a precise refusal.
fn link_path_blocker(path: &Path, what: &str) -> Result<std::fs::Metadata, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(format!(
            "the recorded {what} is missing, so there is nothing of this transaction's to remove"
        )),
        Err(error) => Err(format!(
            "the recorded {what} path cannot be inspected ({error})"
        )),
        Ok(metadata) if metadata.file_type().is_dir() => Err(format!(
            "a directory now occupies the recorded {what} path; it is never removed"
        )),
        Ok(metadata) => Ok(metadata),
    }
}

fn guarded_symlink_retry(
    entry: &super::model::TransactionEntry,
    expected_target: &Path,
    destination_root: &Path,
) -> Result<(), String> {
    if !expected_target.is_absolute()
        || expected_target != entry.source_path
        || !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
    {
        return Err("the journal does not record a valid owned link operation".into());
    }
    if super::executor::link_adoption(entry) == Some(true) {
        // A link this transaction only adopted is left exactly as found.
        return Ok(());
    }
    let metadata = link_path_blocker(&entry.destination_path, "link")?;
    if !metadata.file_type().is_symlink() {
        return Err(
            "a normal file now occupies the recorded link path; it is never removed".into(),
        );
    }
    match std::fs::read_link(&entry.destination_path) {
        Ok(target) if target == expected_target => Ok(()),
        Ok(_) => Err(
            "the link at the recorded path now points somewhere else; it is never removed".into(),
        ),
        Err(error) => Err(format!("the recorded link cannot be read ({error})")),
    }
}

fn guarded_hardlink_retry(
    entry: &super::model::TransactionEntry,
    expected_source: &Path,
    destination_root: &Path,
) -> Result<(), String> {
    if !expected_source.is_absolute()
        || expected_source != entry.source_path
        || !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
    {
        return Err("the journal does not record a valid owned hard link operation".into());
    }
    let metadata = link_path_blocker(&entry.destination_path, "hard link")?;
    if metadata.file_type().is_symlink() {
        return Err("a symbolic link now occupies the recorded hard link path".into());
    }
    let source = capture_identity(&entry.source_path)
        .map_err(|_| "the hard link's source file no longer exists".to_string())?;
    let destination = capture_identity(&entry.destination_path)
        .map_err(|_| "the recorded hard link cannot be identified".to_string())?;
    if !identity_matches(&entry.identity, &source)
        || !identity_matches(&entry.identity, &destination)
    {
        return Err(
            "the recorded hard link no longer names the recorded file (identity changed); \
             it is never removed"
                .into(),
        );
    }
    Ok(())
}

/// Marks every refusal caused by a journal that no longer matches this
/// session's record. Callers must quarantine the Undo handle: another ordinary
/// Undo must not run until the journal has been reviewed and reconciled.
pub const JOURNAL_INTEGRITY_LOST: &str = "journal integrity lost";

/// Whether a rollback error reports lost journal integrity.
pub fn is_journal_integrity_loss(error: &str) -> bool {
    error.contains(JOURNAL_INTEGRITY_LOST) || super::seal::is_seal_refusal(error)
}

fn integrity_lost(detail: &str) -> String {
    format!(
        "{JOURNAL_INTEGRITY_LOST}: {detail}. Nothing further was written or moved; this Undo \
         needs a reviewed recovery of its journal before it can run again"
    )
}

/// Write fencing for a repeat or retry Undo. The journal must be a plain file
/// equal to this session's record before the pass starts, and must still hold
/// exactly the bytes this pass validated or last wrote before every further
/// write. The expected bytes are only ever taken from the validated file or
/// from this session's own record, never from an altered file.
struct JournalFence {
    path: std::path::PathBuf,
    expected: Vec<u8>,
}

impl JournalFence {
    /// `None` only for a transaction that never changed anything and has no
    /// journal yet: there is nothing a journal could be protecting.
    fn validate(
        transaction: &mut RenameTransaction,
        journal_dir: &Path,
    ) -> Result<Option<Self>, String> {
        let path = super::journal::journal_path(journal_dir, &transaction.transaction_id)
            .ok_or_else(|| integrity_lost("the journal cannot be named for this transaction"))?;
        let never_applied = transaction.state == TransactionState::Planned
            && transaction.created_directories.is_empty()
            && transaction.entries.iter().all(|entry| {
                matches!(
                    entry.state,
                    EntryState::Planned | EntryState::PreflightPassed | EntryState::Skipped
                )
            });
        // The independent seal is checked before the journal is trusted.
        match super::seal::verify(journal_dir, &transaction.transaction_id) {
            super::seal::SealCheck::Absent | super::seal::SealCheck::Unavailable(_)
                if never_applied =>
            {
                return Ok(None);
            }
            super::seal::SealCheck::Absent => {
                return Err(integrity_lost("the journal is missing"));
            }
            check if !check.trusted() => {
                return Err(check
                    .refusal()
                    .unwrap_or_else(|| integrity_lost("unverifiable")));
            }
            _ => {}
        }
        let bytes = Self::read_plain(&path)?;
        let recorded: RenameTransaction = serde_json::from_slice(&bytes)
            .map_err(|_| integrity_lost("the journal on disk can no longer be read"))?;
        // The sealed journal is authoritative; the caller's copy is only a
        // cache. A differing cache (a step that failed before its generation
        // was committed, or a newer legitimate generation) is rebuilt from the
        // verified journal, never written over it.
        if receipt_facts(&recorded) != receipt_facts(transaction) {
            *transaction = recorded;
        }
        Ok(Some(Self {
            path,
            expected: bytes,
        }))
    }

    fn read_plain(path: &Path) -> Result<Vec<u8>, String> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Err(integrity_lost("the journal is no longer a plain file")),
            Err(_) => return Err(integrity_lost("the journal is missing")),
        }
        // Re-checked on the opened descriptor, never a blocking open.
        super::journal::read_regular_journal(path).map_err(|error| {
            if super::journal::is_not_regular(&error) {
                integrity_lost("the journal is no longer a plain file")
            } else {
                integrity_lost("the journal can no longer be read")
            }
        })
    }

    fn check(&self) -> Result<(), String> {
        if Self::read_plain(&self.path)? != self.expected {
            return Err(integrity_lost("the journal changed outside this Undo"));
        }
        Ok(())
    }

    /// The bytes this session's record serialises to, as the journal writer
    /// stores them.
    fn advance(&mut self, transaction: &RenameTransaction) -> Result<(), String> {
        let body = serde_json::to_string_pretty(transaction).map_err(|error| error.to_string())?;
        self.expected = format!("{body}\n").into_bytes();
        Ok(())
    }
}

/// The record without per-attempt preflight diagnostics, which a refused
/// re-apply may annotate in memory without persisting: they say nothing about
/// what the transaction changed on disk.
fn receipt_facts(transaction: &RenameTransaction) -> RenameTransaction {
    let mut facts = transaction.clone();
    for entry in &mut facts.entries {
        entry.preflight_passed = false;
        entry.preflight_failures.clear();
    }
    facts
}

/// Explicit recovery route for a caller whose policy is to re-enter recovery
/// reconciliation for entries whose earlier undo failed. It first proves the
/// journal on disk still equals this session's record, then durably records
/// the re-entry (fenced), so the following Undo never overwrites a changed
/// journal from memory.
pub fn reenter_recovery_reconciliation(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
) -> Result<(), String> {
    if transaction.state == TransactionState::RolledBack
        || !transaction
            .entries
            .iter()
            .any(|entry| entry.state == EntryState::RollbackFailed)
    {
        return Ok(());
    }
    let mut fence = JournalFence::validate(transaction, journal_dir)?;
    for entry in &mut transaction.entries {
        if entry.state == EntryState::RollbackFailed {
            entry.state = EntryState::RollingBack;
        }
    }
    persist(journal_dir, transaction, &mut fence)
}

/// Persists this session's record, fenced when the pass is a repeat or retry.
fn persist(
    journal_dir: &Path,
    transaction: &mut RenameTransaction,
    fence: &mut Option<JournalFence>,
) -> Result<(), String> {
    if let Some(fence) = fence.as_ref() {
        fence.check()?;
    }
    write_journal(journal_dir, transaction).map_err(|error| error.to_string())?;
    if let Some(fence) = fence.as_mut() {
        fence.advance(transaction)?;
    }
    Ok(())
}

/// The reverse-rename mutation window for one applied entry. Requires the
/// destination to still be the recorded object and the original source path
/// to be free. Called only after the entry's `RollingBack` state has been
/// durably persisted.
fn rollback_mutation(
    entry: &super::model::TransactionEntry,
    trusted: Option<&TrustedRoots>,
) -> Result<(), String> {
    if matches!(
        entry.operation,
        TransactionOperation::ReplaceExisting { .. }
    ) {
        if let Some(trusted) = trusted
            && !trusted.is_empty()
            && [&entry.source_path, &entry.destination_path]
                .iter()
                .any(|path| {
                    path.parent()
                        .is_none_or(|parent| !ancestor_chain_is_confined(parent, trusted))
                })
        {
            return Err("replacement undo is outside the current trusted roots".into());
        }
        return super::replacement::undo(entry);
    }
    if let TransactionOperation::CreateSymlink {
        expected_target,
        destination_root,
    } = &entry.operation
    {
        if !expected_target.is_absolute()
            || expected_target != &entry.source_path
            || !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
        {
            return Err("rollback refused: invalid journalled symlink target".to_string());
        }
        // The link existed before this transaction and was only adopted:
        // undoing the transaction leaves it exactly as it was found.
        if super::executor::link_adoption(entry) == Some(true) {
            return Ok(());
        }
        let metadata = std::fs::symlink_metadata(&entry.destination_path)
            .map_err(|_| "rollback refused: the link destination no longer exists".to_string())?;
        if !metadata.file_type().is_symlink() {
            return Err(
                "rollback refused: destination is no longer the recorded symlink".to_string(),
            );
        }
        if std::fs::read_link(&entry.destination_path).ok().as_deref() != Some(expected_target) {
            return Err("rollback refused: destination symlink target changed".to_string());
        }
        std::fs::remove_file(&entry.destination_path).map_err(|error| {
            format!("rollback refused: could not remove recorded symlink: {error}")
        })?;
        return Ok(());
    }
    if let TransactionOperation::CreateHardlink {
        expected_source,
        destination_root,
    } = &entry.operation
    {
        if !expected_source.is_absolute()
            || expected_source != &entry.source_path
            || !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
        {
            return Err("rollback refused: invalid journalled hardlink source".to_string());
        }
        let source = capture_identity(&entry.source_path)
            .map_err(|_| "rollback refused: hardlink source no longer exists".to_string())?;
        let destination = capture_identity(&entry.destination_path)
            .map_err(|_| "rollback refused: hardlink destination no longer exists".to_string())?;
        if !identity_matches(&entry.identity, &source)
            || !identity_matches(&entry.identity, &destination)
        {
            return Err("rollback refused: hardlink identity changed".to_string());
        }
        std::fs::remove_file(&entry.destination_path)
            .map_err(|error| format!("rollback refused: could not remove hardlink: {error}"))?;
        return Ok(());
    }
    if let TransactionOperation::CreateCopy {
        expected_source,
        destination_root,
    } = &entry.operation
    {
        if !expected_source.is_absolute()
            || expected_source != &entry.source_path
            || !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
        {
            return Err("rollback refused: invalid journalled copy source".into());
        }
        let source = capture_identity(&entry.source_path)
            .map_err(|_| "rollback refused: copy source no longer exists".to_string())?;
        let destination = capture_identity(&entry.destination_path)
            .map_err(|_| "rollback refused: copy destination no longer exists".to_string())?;
        if !identity_matches(&entry.identity, &source)
            || !content_identity_matches(&entry.identity, &destination)
        {
            return Err("rollback refused: copied destination or source changed".into());
        }
        std::fs::remove_file(&entry.destination_path).map_err(|error| {
            format!("rollback refused: could not remove copied destination: {error}")
        })?;
        return Ok(());
    }
    // Destination must still exist and still be the recorded object.
    match capture_identity(&entry.destination_path) {
        Err(_) => {
            return Err(
                "rollback refused: the destination no longer exists (it was changed externally)"
                    .to_string(),
            );
        }
        Ok(current) if !identity_matches(&entry.identity, &current) => {
            return Err(
                "rollback refused: the destination is no longer the object that was renamed"
                    .to_string(),
            );
        }
        Ok(_) => {}
    }

    // Original source path must be free.
    if std::fs::symlink_metadata(&entry.source_path).is_ok() {
        return Err("rollback refused: the original source path is now occupied".to_string());
    }

    // Ancestor-directory containment, immediately before the mutation. The
    // leaf-only check above (`symlink_metadata` on the source path itself)
    // cannot see an *intermediate* ancestor - the source or destination
    // parent, grandparent, or deeper - that has been replaced with a
    // symlink: ordinary path resolution follows it transparently, exactly
    // as `cd`/`open` would. `ancestor_chain_is_confined` walks every
    // ancestor component up to the trusted root and requires each one to
    // still be a plain directory (never a symlink), regardless of where a
    // substituted symlink's target points - inside the trusted root or
    // outside it makes no difference, since the caller's expected canonical
    // location was replaced either way. Checked right here, immediately
    // before the rename syscall below, so there is no earlier-check/later-
    // mutation gap for a race to exploit.
    if let Some(trusted) = trusted
        && !trusted.is_empty()
    {
        let destination_confined = entry
            .destination_path
            .parent()
            .is_some_and(|parent| ancestor_chain_is_confined(parent, trusted));
        let source_confined = entry
            .source_path
            .parent()
            .is_some_and(|parent| ancestor_chain_is_confined(parent, trusted));
        if !destination_confined || !source_confined {
            return Err(
                "rollback refused: an ancestor directory has been replaced (e.g. with a \
                 symlink) and is no longer a plain directory confined to the trusted root - \
                 manual recovery required"
                    .to_string(),
            );
        }
    }

    // Bind both parents (O_NOFOLLOW + identity) and rename between the descriptors.
    let no_roots = crate::safe_read::TrustedRoots::none();
    let dirs = super::dirsync::RenameDirs::open_in(
        &entry.source_path,
        &entry.destination_path,
        trusted.unwrap_or(&no_roots),
    )
    .map_err(|error| {
        format!(
            "rollback refused: could not bind the parent directories ({error}); nothing was moved"
        )
    })?;
    #[cfg(test)]
    BEFORE_REVERSE_RENAME.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    match dirs.rename_noreplace(true) {
        Ok(()) => {
            #[cfg(test)]
            AFTER_REVERSE_RENAME.with(|hook| {
                if let Some(hook) = hook.borrow_mut().take() {
                    hook();
                }
            });
            // Confirm the source was restored with the recorded identity,
            // inside the bound source directory.
            match capture_identity(&dirs.source_entry()) {
                Ok(current) if identity_matches(&entry.identity, &current) => {
                    // The bound directories must be durable, and still the ones
                    // the paths name, before `RolledBack` may be recorded. A
                    // failure leaves the entry in flight: the file is back at
                    // the original path, so it is never retried nor reported
                    // as unchanged.
                    dirs.sync_and_verify(true).map_err(|error| {
                        IN_FLIGHT_MARK.to_string()
                            + &format!(
                                "the reverse rename completed but its directory entries could \
                                 not be synchronised ({error}); the file is back at the original \
                                 path and the rollback was not recorded - review before any \
                                 further action"
                            )
                    })
                }
                // The file has already moved back: never a clean refusal, so the
                // pass stops and reconciliation decides from the filesystem.
                Ok(_) => Err(MOVED_MARK.to_string()
                    + "rollback renamed but the restored source identity does not match"),
                Err(_) => Err(MOVED_MARK.to_string()
                    + "rollback renamed but the restored source path is not readable"),
            }
        }
        Err(NoClobberError::DestinationExists) => {
            Err("rollback refused: the original source path is occupied (no-overwrite)".to_string())
        }
        Err(error) => Err(format!("rollback rename failed: {error}")),
    }
}

/// Whether every ancestor of `path`, from `path` itself up to and including
/// whichever trusted root contains it, is a plain directory - never a
/// symlink. Uses `symlink_metadata` (never follows the final component of
/// each ancestor), so a substituted symlink is caught by its own file type
/// rather than by comparing where it happens to resolve to; this is why an
/// inside-root and an outside-root substitution are refused identically.
/// `path` must be literally contained in one of `trusted`'s roots (a
/// `starts_with` prefix match on the recorded, non-canonical path) or this
/// returns `false` immediately - a path outside every trusted root was
/// never safe to rename into/out of in the first place.
fn ancestor_chain_is_confined(path: &Path, trusted: &TrustedRoots) -> bool {
    let Some(root) = trusted
        .roots()
        .iter()
        .find(|root| path.starts_with(root.as_path()))
    else {
        return false;
    };
    let mut current = path;
    loop {
        match std::fs::symlink_metadata(current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            _ => return false,
        }
        if current == root.as_path() {
            return true;
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// Test-only, per-thread: invoked with the number of entries reversed so far
/// immediately after each reversal. It lets a test cancel at an exact point
/// instead of racing a second thread against the rollback worker. Compiled out
/// of production builds.
#[cfg(test)]
thread_local! {
    pub(super) static BEFORE_REVERSE_RENAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(super) static AFTER_REVERSE_RENAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(super) static AFTER_REVERSAL: std::cell::RefCell<Option<Box<dyn FnMut(usize)>>> =
        const { std::cell::RefCell::new(None) };
    pub(super) static AFTER_JOURNAL_VALIDATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}
