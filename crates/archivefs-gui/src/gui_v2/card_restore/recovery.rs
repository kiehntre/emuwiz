//! Blue view of PS2 restore records: interrupted-restore recovery and an
//! explicit, evidence-preserving review of records parked for a person.
//!
//! * Discovery and recovery are Core's (`discover_ps2_restore_journals`,
//!   `recover_all_interrupted_ps2_restores`), unchanged. Recovery never
//!   writes the card; Blue additionally requires PCSX2 to be closed so the
//!   card is not judged while it may be changing.
//! * Nothing here deletes, moves or edits a journal, backup or PSU. A review
//!   is a create-only `<journal>.reviewed` sidecar bound to the journal's
//!   SHA-256; Core and the Save Vault ignore it and keep showing the record.
//! * A readable parked record may be marked reviewed only while the card's
//!   pre-restore content provably still exists (the live card is unchanged
//!   from before the restore, or the verified backup is intact). Otherwise it
//!   is explained and kept blocking.
//! * An unreadable record can never be marked reviewed: its card and
//!   recovery state are unknown, so it keeps every card paused (owner
//!   decision, 2026-11-05) and is shown with the evidence to inspect.

use super::sha256_hex;
use archivefs_core::{
    memory_card_inventory::restore_guard::{
        Ps2RecoveryOutcome, Ps2RestoreJournal, Ps2RestoreJournalSummary,
        discover_ps2_restore_journals, load_ps2_restore_journal,
        recover_all_interrupted_ps2_restores,
    },
    save_snapshots::tree_restore::safety::{EmulatorQuiescence, QuiescenceProvider},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const REVIEW_SUFFIX: &str = ".reviewed";
const REVIEW_VERSION: u32 = 1;
/// Largest journal Core writes is 256 KiB; anything bigger is not hashed.
const MAX_JOURNAL_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum RecordKind {
    /// A restore or Undo was in flight; Core recovery can judge it.
    Unfinished,
    /// The record cannot be read; the card it concerns is unknown.
    Unreadable,
    /// Parked for a person; Core will not retry it.
    NeedsReview,
    /// A completed restore whose Undo is still available.
    UndoAvailable,
    /// Finished (undone, rolled back or abandoned); kept as history.
    Finished,
}

/// A person's recorded review of one journal, valid only while the journal
/// bytes are exactly the reviewed ones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in super::super) struct Review {
    pub(in super::super) version: u32,
    pub(in super::super) journal_sha256: String,
    pub(in super::super) reviewed_unix_seconds: u64,
    pub(in super::super) evidence: String,
}

#[derive(Clone, Debug)]
pub(in super::super) struct RecordRow {
    pub(in super::super) path: PathBuf,
    pub(in super::super) kind: RecordKind,
    pub(in super::super) card_path: Option<PathBuf>,
    pub(in super::super) folder: Option<String>,
    pub(in super::super) detail: Option<String>,
    pub(in super::super) review: Option<Review>,
}

impl RecordRow {
    /// Whether this record stops a new restore to `card` from Blue.
    pub(in super::super) fn blocks(&self, card: &Path) -> bool {
        match self.kind {
            // Card and recovery state unknown: every card, always. A review
            // can never lift this; only a record Core can read again can.
            RecordKind::Unreadable => true,
            // Recoverable by Core recovery; never waived by a review.
            RecordKind::Unfinished => true,
            // A review lifts the block only while its evidence still holds,
            // re-checked every time (a forged sidecar proves nothing).
            RecordKind::NeedsReview => {
                (self.review.is_none() || !review_evidence_holds(&self.path))
                    && concerns_card(&self.path, card)
            }
            RecordKind::UndoAvailable | RecordKind::Finished => false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(in super::super) struct RecordsReport {
    pub(in super::super) rows: Vec<RecordRow>,
    /// Why the list may be incomplete; empty only when it is complete.
    pub(in super::super) problems: Vec<String>,
}

impl RecordsReport {
    pub(in super::super) fn complete(&self) -> bool {
        self.problems.is_empty()
    }
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && metadata.len() <= MAX_JOURNAL_BYTES)
        .then(|| fs::read(path).ok())
        .flatten()
}

fn review_path(journal: &Path) -> PathBuf {
    let mut name = journal.file_name().unwrap_or_default().to_os_string();
    name.push(REVIEW_SUFFIX);
    journal.with_file_name(name)
}

/// The valid review for `journal`, if its sidecar matches the current bytes.
/// Only readable records can carry a review; see [`RecordRow::blocks`].
fn valid_review(journal: &Path) -> Option<Review> {
    load_ps2_restore_journal(journal).ok()?;
    let bytes = read_bounded(journal)?;
    let review: Review = serde_json::from_slice(&read_bounded(&review_path(journal))?).ok()?;
    (review.version == REVIEW_VERSION && review.journal_sha256 == sha256_hex(&bytes))
        .then_some(review)
}

fn kind_of(summary: &Ps2RestoreJournalSummary) -> RecordKind {
    if summary.error.is_some() {
        RecordKind::Unreadable
    } else if summary.needs_recovery {
        RecordKind::Unfinished
    } else if summary.needs_attention {
        RecordKind::NeedsReview
    } else if summary.undo_available {
        RecordKind::UndoAvailable
    } else {
        RecordKind::Finished
    }
}

/// Worker: every PS2 restore record, read-only.
pub(in super::super) fn load_records(journal_dir: &Path) -> RecordsReport {
    let discovery = discover_ps2_restore_journals(journal_dir);
    let rows = discovery
        .records
        .iter()
        .map(|summary| {
            let journal = load_ps2_restore_journal(&summary.path).ok();
            RecordRow {
                path: summary.path.clone(),
                kind: kind_of(summary),
                card_path: summary.card_path.clone(),
                folder: journal.as_ref().map(|j| j.save_display_name.clone()),
                detail: summary.error.clone().or_else(|| summary.detail.clone()),
                review: valid_review(&summary.path),
            }
        })
        .collect();
    RecordsReport {
        rows,
        problems: discovery
            .problems
            .iter()
            .map(|problem| format!("{problem:?}"))
            .collect(),
    }
}

fn outcome_message(outcome: &Ps2RecoveryOutcome) -> String {
    match outcome {
        Ps2RecoveryOutcome::NothingToDo(_) => "Nothing was in progress.".into(),
        Ps2RecoveryOutcome::AbandonedBeforePublication | Ps2RecoveryOutcome::NotPublished => {
            "The restore had not changed the card. Its backup is kept.".into()
        }
        Ps2RecoveryOutcome::ConfirmedPublished => {
            "The restore had finished and the card was verified. Undo is available.".into()
        }
        Ps2RecoveryOutcome::UndoNotApplied => {
            "The Undo had not changed the card. Undo is available again.".into()
        }
        Ps2RecoveryOutcome::UndoCompleted => {
            "The Undo had finished before the interruption.".into()
        }
        Ps2RecoveryOutcome::NeedsAttention(detail) => {
            format!("Needs manual review; nothing was changed: {detail}")
        }
    }
}

/// Worker: Core's recovery for every unfinished record. Never writes a card;
/// refused unless PCSX2 is proven closed so the card is judged at rest.
pub(in super::super) fn recover(
    journal_dir: &Path,
    observer: &dyn QuiescenceProvider,
    now_unix_seconds: u64,
) -> Result<Vec<String>, String> {
    let binding = archivefs_core::memory_card_inventory::restore_guard::ps2_card_binding(
        journal_dir,
        "restore recovery",
    );
    match observer.observe(&binding) {
        EmulatorQuiescence::Closed => {}
        EmulatorQuiescence::Running => {
            return Err("PCSX2 is running. Close it, then check interrupted restores again. Nothing was changed.".into());
        }
        EmulatorQuiescence::CardInUse => {
            return Err("Restore records are in use. Another application may have them open. Close it, then check interrupted restores again. Nothing was changed.".into());
        }
        EmulatorQuiescence::Unknown => {
            return Err("EmuWiz could not confirm PCSX2 is closed, so interrupted restores were not checked. Nothing was changed.".into());
        }
    }
    let run = recover_all_interrupted_ps2_restores(journal_dir, now_unix_seconds);
    let mut messages: Vec<String> = run
        .outcomes
        .iter()
        .map(|(path, outcome)| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            match outcome {
                Ok(outcome) => format!("{name}: {}", outcome_message(outcome)),
                Err(error) => format!(
                    "{name}: could not be checked and is kept as it is ({})",
                    crate::pcsx2_page::psu_restore_error_message(error)
                ),
            }
        })
        .collect();
    if messages.is_empty() {
        messages.push("No interrupted restore needed checking.".into());
    }
    if !run.problems.is_empty() {
        messages.push("Some restore records could not be listed, so this check may be incomplete. Nothing was removed.".into());
    }
    Ok(messages)
}

/// What a person sees before marking a record reviewed. `allowed` explains
/// why it may or may not be marked; nothing is written by building this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in super::super) struct ReviewPreview {
    pub(in super::super) journal: PathBuf,
    pub(in super::super) journal_sha256: String,
    pub(in super::super) facts: Vec<String>,
    pub(in super::super) allowed: Result<String, String>,
}

fn file_sha(path: &Path) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata
        .is_file()
        .then(|| fs::read(path).ok().map(|bytes| sha256_hex(&bytes)))
        .flatten()
}

fn readable_evidence(journal: &Ps2RestoreJournal) -> (Vec<String>, Result<String, String>) {
    let mut facts = vec![
        format!("Memory card: {}", journal.card_path.display()),
        format!("Save folder: {}", journal.save_display_name),
        format!("Recorded state: {:?}", journal.phase),
    ];
    if let Some(detail) = &journal.detail {
        facts.push(format!("Recorded reason: {detail}"));
    }
    let card = file_sha(&journal.card_path);
    let card_state = match card.as_deref() {
        None => "missing or unreadable",
        Some(sha) if sha == journal.original_sha256 => "the same as before the restore",
        Some(sha) if Some(sha) == journal.post_sha256.as_deref() => "the restored version",
        Some(_) => "changed since (for example by playing)",
    };
    facts.push(format!("The card is now {card_state}."));
    // Core writes the backup as the pre-restore card; both must agree.
    let backup_ok = journal.backup_sha256.as_deref().is_some_and(|expected| {
        expected == journal.original_sha256
            && file_sha(&journal.backup_path).as_deref() == Some(expected)
    });
    facts.push(format!(
        "Backup of the card from before the restore: {} ({})",
        journal.backup_path.display(),
        if backup_ok {
            "present and verified"
        } else {
            "missing or not verified"
        }
    ));
    // Only the verified backup is lasting proof. "The card is unchanged" stops
    // holding at the next restore, which would silently re-park the card
    // (including the Undo of that restore), so it is never a basis on its own.
    let allowed = if backup_ok {
        Ok("Your card from before the restore is safe in the verified backup above. Marking this record reviewed lets Blue restore to this card again. The record and backup are kept.".to_string())
    } else {
        Err("EmuWiz cannot prove your card from before the restore is kept safe: the backup is missing or does not verify. This record is kept and keeps restores to this card paused. Inspect the card and backup yourself (Save Vault, Advanced).".to_string())
    };
    (facts, allowed)
}

/// Worker: evidence for reviewing one parked or unreadable record.
pub(in super::super) fn review_preview(journal_path: &Path) -> Result<ReviewPreview, String> {
    let bytes = read_bounded(journal_path)
        .ok_or("This restore record can no longer be read as a file. It is kept.")?;
    let journal_sha256 = sha256_hex(&bytes);
    let (facts, allowed) = match load_ps2_restore_journal(journal_path) {
        Ok(journal) if journal.phase.needs_recovery() => {
            return Err("This restore was interrupted. Use Check interrupted restores; it is not marked reviewed by hand.".into());
        }
        Ok(journal) => readable_evidence(&journal),
        Err(error) => {
            let metadata = fs::symlink_metadata(journal_path).ok();
            (
                vec![
                    format!("Record: {}", journal_path.display()),
                    format!(
                        "Size: {} bytes · SHA-256: {journal_sha256}",
                        metadata.map_or(0, |m| m.len())
                    ),
                    format!("It cannot be read: {error}"),
                    "Evidence to inspect: this record file, your PCSX2 memory cards, and the card backups and exported save folders listed under Restore storage (all are kept).".into(),
                ],
                Err("EmuWiz cannot tell which memory card this record concerns or whether its restore finished, so restores to every memory card stay paused. This cannot be marked reviewed. The record and every backup are kept; they need manual investigation.".to_string()),
            )
        }
    };
    Ok(ReviewPreview {
        journal: journal_path.to_path_buf(),
        journal_sha256,
        facts,
        allowed,
    })
}

/// Worker: record the review, only if the evidence is still exactly what was
/// previewed. Create-only; the journal itself is never touched.
pub(in super::super) fn mark_reviewed(
    preview: &ReviewPreview,
    now_unix_seconds: u64,
) -> Result<(), String> {
    let current = review_preview(&preview.journal)?;
    if &current != preview {
        return Err("The record or its evidence changed since you reviewed it. Review it again. Nothing was changed.".into());
    }
    let evidence = current.allowed.clone()?;
    let review = Review {
        version: REVIEW_VERSION,
        journal_sha256: current.journal_sha256.clone(),
        reviewed_unix_seconds: now_unix_seconds,
        evidence: format!("{} {}", current.facts.join(" | "), evidence),
    };
    let path = review_path(&preview.journal);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            format!("The review could not be recorded ({error}). Nothing was changed.")
        })?;
    let bytes = serde_json::to_vec_pretty(&review).map_err(|error| error.to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("The review could not be recorded ({error})."))?;
    if let Some(parent) = path.parent() {
        let _ = fs::File::open(parent).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

/// Whether a readable record's pre-restore card content still provably
/// exists (the condition a review was allowed under).
fn review_evidence_holds(journal_path: &Path) -> bool {
    load_ps2_restore_journal(journal_path).is_ok_and(|journal| {
        !journal.phase.needs_recovery() && readable_evidence(&journal).1.is_ok()
    })
}

/// Whether a journal concerns the physical card at `card`. Anything that
/// cannot be established counts as "yes".
pub(in super::super) fn concerns_card(journal_path: &Path, card: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(journal) = load_ps2_restore_journal(journal_path) else {
        return true;
    };
    let Ok(live) = fs::metadata(card) else {
        return true;
    };
    let same_path = journal.card_path == card
        || matches!(
            (fs::canonicalize(&journal.card_path), fs::canonicalize(card)),
            (Ok(a), Ok(b)) if a == b
        );
    let same_file = [
        Some(journal.original_identity),
        journal.post_identity,
        journal.undo_identity,
    ]
    .into_iter()
    .flatten()
    .any(|identity| identity.device == live.dev() && identity.inode == live.ino());
    same_path || same_file
}
