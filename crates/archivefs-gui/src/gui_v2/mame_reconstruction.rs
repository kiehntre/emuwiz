//! GUI-v2 MAME merged-set reconstruction: select a machine, inspect its
//! dependencies, preview, confirm, publish, inspect the result and Undo.
//!
//! This module is orchestration and presentation only. Every filesystem step
//! is the core backend: planning and packed ZIP-member discovery
//! ([`archivefs_core::dat::mame_merged_reconstruction`]), the source- and
//! target-bound publication review and its journalled apply, and the shared
//! [`archivefs_core::dat::rename_apply`] rollback. What lives here:
//!
//! - [`publish_gate`]: the typed reasons a reviewed plan is not publishable;
//! - [`MameRefusal`]: backend refusals mapped to distinct, honest states;
//! - [`run_publish`] / [`run_undo`]: worker bodies with injectable journal
//!   locations so they run against synthetic fixtures;
//! - [`MameReconstructionPanel`]: the page state, which runs every evidence
//!   refresh, preview, publish and Undo on a background thread and never on
//!   the GUI thread.
//!
//! None of the backend steps can be cancelled once started, so no Cancel
//! control is offered for a running job. A preview is read-only. Publishing
//! always rebuilds the plan and refuses when it differs from the one the
//! person reviewed; it never substitutes a newer plan behind their back.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use archivefs_core::dat::mame_arcade_join::{ArcadeJoinEvidence, refresh_mame_member_evidence};
use archivefs_core::dat::mame_merged_reconstruction::staging_lifecycle::{
    self, FINALISE_CONFIRMATION, Finalise, StagingItem, finalise_marker,
};
use archivefs_core::dat::mame_merged_reconstruction::{
    CatalogueReference, DeviceMemberSupply, ExcludedMemberReason, MAME_RECONSTRUCTION_WORKFLOW,
    MameMergedReconstructionPlan, RECONSTRUCTION_RUNNABILITY_NOTE, ReconstructionDependencies,
    ReconstructionFamilyShape, ReconstructionPublicationAction, ReplacementMemberChanges,
    build_merged_reconstruction_plan, discover_packed_zip_sources, reconstruction_dependencies,
    reconstruction_family_names, reconstruction_family_shape, replacement_member_changes,
    reserved_transaction_id, review_reconstruction_publication, verify_staged_output,
};
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::rename_apply::model::{
    EntryState, ObjectIdentity, RenameTransaction, RollbackResult, TransactionOperation,
    TransactionState,
};
use archivefs_core::dat::rename_apply::{
    RollbackOutcome, capture_identity, default_rename_transaction_dir, identity_matches,
    list_journals, rollback_transaction_confined,
};
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::{Database, default_database_path};
use eframe::egui::{self, RichText};

use crate::dat_catalogue_picker::{DatCataloguePickerState, DatCatalogueWorkflow};
use crate::ui::theme;
use archivefs_core::dat::catalogue_selection::CatalogueRef;
mod catalogue;
mod cheats;
mod playing;
use catalogue::{CatalogueBinding, load_selected};

pub(super) fn publish_confirmation_phrase(outputs: usize) -> String {
    format!("PUBLISH MAME {outputs} OUTPUTS")
}

/// Names the exact file being replaced.
pub(super) fn replace_confirmation_phrase(destination: &Path) -> String {
    format!(
        "REPLACE {}",
        destination.file_name().map_or_else(
            || destination.display().to_string(),
            |name| name.to_string_lossy().into_owned()
        )
    )
}

pub(super) fn undo_confirmation_phrase(parent: &str) -> String {
    format!("UNDO MAME {parent}")
}

/// Typed to surrender Undo and delete the preserved original.
pub(super) fn finalise_confirmation_phrase(parent: &str) -> String {
    format!("DELETE ORIGINAL AND FINALISE MAME {parent}")
}

// ---------------------------------------------------------------------------
// Transaction evidence. Same rules as the Organisation page on main (470010cb):
// a state label alone never proves a publication or an Undo.
// ---------------------------------------------------------------------------

pub(super) fn is_mame_reconstruction(transaction: &RenameTransaction) -> bool {
    transaction
        .unknown
        .get("workflow")
        .and_then(serde_json::Value::as_str)
        == Some(MAME_RECONSTRUCTION_WORKFLOW)
}

/// A publication succeeded only when its transaction ended `Applied` with
/// every entry applied.
pub(super) fn publication_succeeded(transaction: &RenameTransaction) -> bool {
    transaction.state == TransactionState::Applied
        && !transaction.entries.is_empty()
        && transaction
            .entries
            .iter()
            .all(|entry| entry.state == EntryState::Applied)
}

pub(super) fn failure_reasons(transaction: &RenameTransaction) -> String {
    let reasons = transaction
        .entries
        .iter()
        .flat_map(|entry| {
            entry
                .failure_reason
                .iter()
                .chain(entry.preflight_failures.iter())
        })
        .cloned()
        .collect::<Vec<_>>();
    if reasons.is_empty() {
        String::new()
    } else {
        format!(": {}", reasons.join("; "))
    }
}

/// Whether an Undo has anything it could act on: an applied output, an entry
/// an interrupted run left in flight (the rollback reconciles those against
/// the filesystem before touching anything), or a refused move
/// awaiting review (the rollback retries it only after fresh checks and
/// otherwise reports the exact blocker).
pub(super) fn undo_actionable(transaction: &RenameTransaction) -> bool {
    // A finalised (or half-finalised) replacement has surrendered Undo; the
    // backend refuses it too, so never offer it.
    finalise_marker(transaction).is_none()
        && transaction.is_rollbackable()
        && transaction.entries.iter().any(|entry| {
            matches!(
                entry.state,
                EntryState::Applied | EntryState::Applying | EntryState::RollingBack
            ) || (entry.state == EntryState::RollbackFailed
                && entry.operation == TransactionOperation::RenameMove)
        })
}

/// A verified, still-Undoable replacement is the only thing that can be
/// finalised: Finalise Replacement deletes the original that Undo would restore.
pub(super) fn finalise_actionable(transaction: &RenameTransaction) -> bool {
    is_mame_reconstruction(transaction)
        && is_replacement(transaction)
        && publication_succeeded(transaction)
        && finalise_marker(transaction).is_none()
}

/// A Finalise Replacement that was recorded but not finished.
pub(super) fn finalise_interrupted(transaction: &RenameTransaction) -> bool {
    matches!(
        finalise_marker(transaction),
        Some(Finalise::Finalising { .. })
    )
}

/// An Undo is verified only when the journal state and every entry agree and
/// at least one output was actually reversed.
pub(super) fn rollback_verified(transaction: &RenameTransaction) -> bool {
    transaction.is_fully_rolled_back()
        && transaction
            .entries
            .iter()
            .any(|entry| entry.state == EntryState::RolledBack)
}

fn entry_states(transaction: &RenameTransaction) -> String {
    if transaction.entries.is_empty() {
        return "no entries".into();
    }
    transaction
        .entries
        .iter()
        .map(|entry| entry.state.label())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether the transaction replaced an existing archive (its original is
/// preserved at the entry's source path) rather than creating a new one.
pub(super) fn is_replacement(transaction: &RenameTransaction) -> bool {
    transaction.entries.iter().any(|entry| {
        matches!(
            entry.operation,
            TransactionOperation::ReplaceExisting { .. }
        )
    })
}

fn preserved_original(transaction: &RenameTransaction) -> String {
    transaction
        .entries
        .first()
        .map_or_else(String::new, |entry| entry.source_path.display().to_string())
}

/// One-line status of a history row, from the transaction state and the
/// entry receipts together.
pub(super) fn history_status(transaction: &RenameTransaction) -> String {
    match transaction.state {
        TransactionState::Finalised
            if transaction
                .entries
                .iter()
                .all(|e| e.state == EntryState::Applied)
                && !transaction.entries.is_empty() =>
        {
            return "Finalised: the original archive was deleted on your confirmation, so Undo is no longer available. The replacement is the published file. This record is kept as history and cannot be changed.".into();
        }
        TransactionState::Finalised => {
            return "This record says \"Finalised\" but its entries do not confirm a published replacement; manual review is required.".into();
        }
        TransactionState::Finalising => {
            return "Finalising this replacement was interrupted. Undo is unavailable until you finish or cancel it below; the original may or may not have been deleted.".into();
        }
        _ => {}
    }
    match transaction.state {
        TransactionState::Applied if publication_succeeded(transaction) && is_replacement(transaction) => {
            format!(
                "Verified replacement complete. The original archive is preserved, unchanged, at {}; undo puts it back.",
                preserved_original(transaction)
            )
        }
        TransactionState::Applied if publication_succeeded(transaction) => {
            "Verified publication complete; undo is available.".into()
        }
        TransactionState::Applied => format!(
            "This record says \"Applied\" but its entries do not confirm a publication (entries: {}). It is not treated as published; review is required.",
            entry_states(transaction)
        ),
        TransactionState::RolledBack if rollback_verified(transaction) && is_replacement(transaction) => {
            format!(
                "Rolled back; the original archive was restored. The rebuilt copy is kept at {}.",
                preserved_original(transaction)
            )
        }
        TransactionState::RolledBack if rollback_verified(transaction) => {
            "Rolled back; the generated destination is removed.".into()
        }
        TransactionState::RolledBack if transaction.is_fully_rolled_back() => {
            "Closed without a publication: this transaction published nothing, so nothing was rolled back.".into()
        }
        TransactionState::RolledBack => format!(
            "This record says \"Rolled back\" but its entries do not confirm it (entries: {}){}. The undo is not verified and the destination may still be present; manual review is required.",
            entry_states(transaction),
            failure_reasons(transaction)
        ),
        TransactionState::RollbackFailed => format!(
            "Undo did not complete{}. The destination was left as it is and may still be present; manual review is required before this transaction can be considered complete.",
            failure_reasons(transaction)
        ),
        TransactionState::ApplyFailed if !undo_actionable(transaction) => {
            "Publication did not complete: nothing was published, so there is nothing to undo. Recovery is required before this transaction can be considered complete.".into()
        }
        _ => "Recovery is required before this transaction can be considered complete.".into(),
    }
}

fn mame_parent(transaction: &RenameTransaction) -> &str {
    transaction
        .unknown
        .get("mame_parent")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("SET")
}

/// Every MAME reconstruction journal, plus any journal that could not be read.
pub(super) fn load_history(journal_dir: &Path) -> (Vec<RenameTransaction>, Vec<String>) {
    let (transactions, problems) = list_journals(journal_dir);
    let mut history = transactions
        .into_iter()
        .filter(is_mame_reconstruction)
        .collect::<Vec<_>>();
    history.sort_by(|a, b| {
        a.created_at_unix
            .cmp(&b.created_at_unix)
            .then(a.transaction_id.cmp(&b.transaction_id))
    });
    (history, problems)
}

// ---------------------------------------------------------------------------
// Publish gate and refusals (recovered from feat/mame-reconstruction-gui-v1).
// ---------------------------------------------------------------------------

/// Why a reviewed plan cannot be published. Computed from the plan alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BlockReason {
    MissingMembers(usize),
    WrongHash(usize),
    DuplicateOwnership(usize),
    UnresolvedOwnership(usize),
    DestinationCollision(usize),
    NothingRequired,
    SourcesIncomplete {
        have: usize,
        need: usize,
    },
    /// The backend marked the plan blocked for a reason none of the above name.
    NotReady,
}

impl BlockReason {
    pub(super) fn describe(&self) -> String {
        match self {
            Self::MissingMembers(n) => format!("{n} required file(s) are missing"),
            Self::WrongHash(n) => format!("{n} file(s) have the wrong checksum"),
            Self::DuplicateOwnership(n) => {
                format!("{n} file(s) have more than one possible source")
            }
            Self::UnresolvedOwnership(n) => {
                format!("{n} file(s) have parent/clone/BIOS ownership the catalogue does not prove")
            }
            Self::DestinationCollision(n) => {
                format!("{n} output(s) would collide with something already there")
            }
            Self::NothingRequired => "the plan requires no files".into(),
            Self::SourcesIncomplete { have, need } => {
                format!("only {have} of {need} required files have a verified source")
            }
            Self::NotReady => "EmuWiz has not marked this plan ready".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PublishGate {
    Ready,
    Blocked(Vec<BlockReason>),
}

impl PublishGate {
    pub(super) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Publish is enabled only for a plan the backend marked ready and whose every
/// required member has exactly one verified source. A preview existing is
/// never enough.
pub(super) fn publish_gate(plan: &MameMergedReconstructionPlan) -> PublishGate {
    let mut reasons = Vec::new();
    if !plan.missing_members.is_empty() {
        reasons.push(BlockReason::MissingMembers(plan.missing_members.len()));
    }
    if !plan.hash_mismatches.is_empty() {
        reasons.push(BlockReason::WrongHash(plan.hash_mismatches.len()));
    }
    if !plan.duplicate_candidates.is_empty() {
        reasons.push(BlockReason::DuplicateOwnership(
            plan.duplicate_candidates.len(),
        ));
    }
    if !plan.unresolved_ownership.is_empty() {
        reasons.push(BlockReason::UnresolvedOwnership(
            plan.unresolved_ownership.len(),
        ));
    }
    if !plan.collisions.is_empty() {
        reasons.push(BlockReason::DestinationCollision(plan.collisions.len()));
    }
    if plan.required_members.is_empty() {
        reasons.push(BlockReason::NothingRequired);
    } else if plan.sources.len() != plan.required_members.len() {
        reasons.push(BlockReason::SourcesIncomplete {
            have: plan.sources.len(),
            need: plan.required_members.len(),
        });
    }
    if reasons.is_empty() && !plan.ready_to_apply {
        reasons.push(BlockReason::NotReady);
    }
    if reasons.is_empty() {
        PublishGate::Ready
    } else {
        PublishGate::Blocked(reasons)
    }
}

/// A backend refusal, kept distinct instead of flattened into "failed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MameRefusal {
    PlanStale(String),
    SourceChanged(String),
    DestinationChanged(String),
    DestinationCollision(String),
    PlanBlocked(String),
    VerificationFailed(String),
    PackedMemberUnsupported(String),
    NoEvidence(String),
    /// A stage exists without a publication journal after an interrupted write.
    StagingInterrupted(String),
    /// An earlier MAME transaction or staging leftover needs Recovery first.
    RecoveryRequired(String),
    /// Another MAME publication holds the journal lock.
    Busy(String),
    Preflight(String),
    JournalUnavailable(String),
    Other(String),
}

impl MameRefusal {
    pub(super) fn headline(&self) -> &'static str {
        match self {
            Self::PlanStale(_) => "The reviewed plan is out of date",
            Self::SourceChanged(_) => "A source file changed after the preview",
            Self::DestinationChanged(_) => "The destination changed after the preview",
            Self::DestinationCollision(_) => "The destination is not safe to publish over",
            Self::PlanBlocked(_) => "The plan is blocked",
            Self::VerificationFailed(_) => "Verification did not pass",
            Self::PackedMemberUnsupported(_) => {
                "Some required members cannot be read from this archive layout"
            }
            Self::NoEvidence(_) => "EmuWiz has no verified evidence for this set yet",
            Self::StagingInterrupted(_) => "MAME staging was interrupted",
            Self::RecoveryRequired(_) => "An earlier MAME operation needs Recovery first",
            Self::Busy(_) => "Another MAME publication is running",
            Self::Preflight(_) => "The safety check refused this publication",
            Self::JournalUnavailable(_) => "The recovery journal is unavailable",
            Self::Other(_) => "The publication was refused",
        }
    }

    pub(super) fn detail(&self) -> &str {
        match self {
            Self::PlanStale(d)
            | Self::SourceChanged(d)
            | Self::DestinationChanged(d)
            | Self::DestinationCollision(d)
            | Self::PlanBlocked(d)
            | Self::VerificationFailed(d)
            | Self::PackedMemberUnsupported(d)
            | Self::NoEvidence(d)
            | Self::StagingInterrupted(d)
            | Self::RecoveryRequired(d)
            | Self::Busy(d)
            | Self::Preflight(d)
            | Self::JournalUnavailable(d)
            | Self::Other(d) => d,
        }
    }

    /// What the person should do next. Every refusal here happened before the
    /// destination was touched; an attempt the backend refused after staging
    /// keeps its staged file and diagnostic, which the detail names.
    pub(super) fn next_step(&self) -> &'static str {
        match self {
            Self::PlanStale(_)
            | Self::SourceChanged(_)
            | Self::DestinationChanged(_)
            | Self::NoEvidence(_) => {
                "Nothing was published. Refresh the evidence and preview again."
            }
            Self::DestinationCollision(_) => {
                "Nothing was published. Choose a different MAME folder or move the existing file yourself."
            }
            Self::PlanBlocked(_) | Self::PackedMemberUnsupported(_) => {
                "Nothing was published. Resolve the blockers shown in the preview first."
            }
            Self::RecoveryRequired(_) => {
                "Nothing was published by this attempt. Review History and Recovery before trying again; do not move staged files by hand."
            }
            Self::StagingInterrupted(_) => {
                "Nothing was published. Review the retained file in Results and Undo, then preview again for a fresh attempt. The old staged file will not be reused or removed."
            }
            Self::Busy(_) => "Nothing was published. Wait for it to finish, then try again.",
            Self::VerificationFailed(_) => {
                "Nothing was published. The staged copy did not match the catalogue."
            }
            Self::Preflight(_) | Self::JournalUnavailable(_) | Self::Other(_) => {
                "Nothing was published."
            }
        }
    }

    /// Maps a backend error string to a typed refusal. The backend reports
    /// errors as text; this is the single place that interprets it.
    pub(super) fn from_backend(message: &str) -> Self {
        let text = message.to_string();
        let lower = message.to_ascii_lowercase();
        if lower.contains("stale reconstruction source")
            || lower.contains("source archive") && lower.contains("changed since the preview")
            || lower.contains("source member size changed")
            || lower.contains("changed or could not be fully verified")
        {
            Self::SourceChanged(text)
        } else if lower.contains("stale reconstruction destination")
            || lower.contains("stale reconstruction target")
        {
            Self::DestinationChanged(text)
        } else if lower.contains("another mame publication is in progress") {
            Self::Busy(text)
        } else if lower.contains("may remain after interruption") {
            Self::StagingInterrupted(text)
        } else if lower.contains("requires recovery before another publication")
            || lower.contains("leftover")
            || lower.contains("malformed recovery journal")
        {
            Self::RecoveryRequired(text)
        } else if lower.contains("reserve mame journal")
            || lower.contains("reserve a mame journal")
            || lower.contains("journal directory unavailable")
            || lower.contains("require separate directories")
        {
            Self::JournalUnavailable(text)
        } else if lower.contains("destination collision")
            || lower.contains("target is not a regular archive")
            || lower.contains("foreign or unproven existing target")
            || lower.contains("existing target needs")
        {
            Self::DestinationCollision(text)
        } else if lower.contains("family plan remains blocked")
            || lower.contains("reconstruction plan is blocked")
        {
            Self::PlanBlocked(text)
        } else if lower.contains("packed mame source") && lower.contains("refused") {
            Self::PackedMemberUnsupported(text)
        } else if lower.contains("preflight refused") {
            Self::Preflight(text)
        } else if lower.contains("staged sha-1/crc/size mismatch")
            || lower.contains("staged member inventory")
            || lower.contains("staged unexpected member")
            || lower.contains("staged duplicate member")
            || lower.contains("staged reconstruction changed during verification")
            || lower.contains("zip inspection")
        {
            Self::VerificationFailed(text)
        } else if lower.contains("no persisted verified mame evidence") {
            Self::NoEvidence(text)
        } else {
            Self::Other(text)
        }
    }
}

// ---------------------------------------------------------------------------
// Preview.
// ---------------------------------------------------------------------------

/// What the person chose. A preview is only current for the inputs it was
/// built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MameInputs {
    pub root: PathBuf,
    pub dat: PathBuf,
    pub reference: CatalogueRef,
    pub trusted: bool,
    pub set: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DestinationStatus {
    Absent,
    /// The destination already holds exactly this set, checksum-verified.
    AlreadyPublished,
    /// An existing parent ZIP that the backend's publication review allows
    /// replacing, judged from checksum evidence read from that ZIP. The
    /// original is kept, exactly, for Undo.
    Replaceable {
        original: Box<ObjectIdentity>,
        changes: ReplacementMemberChanges,
    },
    /// An existing parent ZIP the backend would replace, but whose members
    /// or dependencies are not all accounted for. Replacement is refused.
    ReplacementRefused {
        changes: ReplacementMemberChanges,
        reasons: Vec<String>,
    },
    /// Something is there that cannot be replaced.
    Occupied(String),
}

/// Why replacing an existing parent ZIP is not allowed. One matching ROM is
/// discovery evidence, not authority: every existing member must have an
/// established identity, and every dependency must resolve in the catalogue.
pub(super) fn replacement_refusals(
    changes: &ReplacementMemberChanges,
    dependencies: &ReconstructionDependencies,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if !changes.changed.is_empty() {
        reasons.push(format!(
            "{} member(s) carry a required name but different content: {}",
            changes.changed.len(),
            changes.changed.join(", ")
        ));
    }
    if !changes.unidentified.is_empty() {
        reasons.push(format!(
            "{} member(s) are not identified by the catalogue and would be dropped: {}",
            changes.unidentified.len(),
            changes.unidentified.join(", ")
        ));
    }
    if !changes.support_unverified.is_empty() {
        reasons.push(format!(
            "{} BIOS or support-set ROM(s) have no verified copy in their own set's ZIP in this folder, so replacing would leave them only in the preserved original: {}",
            changes.support_unverified.len(),
            changes
                .support_unverified
                .iter()
                .map(|(member, provider)| format!("{member} ({provider}.zip)"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !dependencies.unresolved_members.is_empty() {
        reasons.push(format!(
            "{} member(s) have ownership the catalogue does not prove",
            dependencies.unresolved_members.len()
        ));
    }
    reasons.extend(dependencies.unresolved_relationships.iter().cloned());
    let unresolved_sets = dependencies
        .external_sets
        .iter()
        .filter(|set| set.catalogue != CatalogueReference::Unique)
        .map(|set| set.name.as_str())
        .collect::<Vec<_>>();
    if !unresolved_sets.is_empty() {
        reasons.push(format!(
            "support set(s) not resolved in the catalogue: {}",
            unresolved_sets.join(", ")
        ));
    }
    reasons
}

#[derive(Debug, Clone)]
pub(super) struct MamePreview {
    pub inputs: MameInputs,
    catalogue: Option<CatalogueBinding>,
    pub plan: MameMergedReconstructionPlan,
    pub dependencies: ReconstructionDependencies,
    pub destination: DestinationStatus,
    /// The gate for the action on offer. For a replacement this is the
    /// backend's reviewed plan, which no longer counts the reviewed target as
    /// a collision.
    pub gate: PublishGate,
    /// Whether each plan source is an extracted folder, recorded on the worker
    /// so rendering never touches the filesystem.
    pub source_is_extracted: Vec<bool>,
}

/// Persisted joins first; packed evidence discovered from a ZIP is added
/// only for paths the database does not already cover. Discovered evidence
/// therefore never overrides (or duplicates) a persisted join, including one
/// the backend would refuse as ambiguous or not found.
pub(super) fn combine_joins(
    mut persisted: Vec<(PathBuf, ArcadeJoinEvidence)>,
    discovered: Vec<(PathBuf, ArcadeJoinEvidence)>,
) -> Vec<(PathBuf, ArcadeJoinEvidence)> {
    let known = persisted
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<std::collections::BTreeSet<_>>();
    persisted.extend(
        discovered
            .into_iter()
            .filter(|(path, _)| !known.contains(path)),
    );
    persisted
}

/// The evidence for the existing destination ZIP among combined joins. It is
/// matched by path only; the backend review decides whether it authorises a
/// replacement. With packed-only evidence, that authority is the backend's
/// threshold: at least one live, checksum-proven, parent-owned member.
pub(super) fn target_evidence(
    joins: &[(PathBuf, ArcadeJoinEvidence)],
    plan: &MameMergedReconstructionPlan,
) -> Option<ArcadeJoinEvidence> {
    joins
        .iter()
        .find(|(path, _)| path == &plan.destination)
        .map(|(_, evidence)| evidence.clone())
}

impl MamePreview {
    pub(super) fn gate(&self) -> PublishGate {
        self.gate.clone()
    }

    pub(super) fn is_replacement(&self) -> bool {
        matches!(self.destination, DestinationStatus::Replaceable { .. })
    }

    /// Read-only: classifies an existing destination through the backend's
    /// publication review without staging or writing anything.
    pub(super) fn from_plan(
        inputs: MameInputs,
        plan: MameMergedReconstructionPlan,
        dependencies: ReconstructionDependencies,
        target: Option<&ArcadeJoinEvidence>,
        dat: &ParsedDat,
    ) -> Self {
        let mut gate = publish_gate(&plan);
        let destination = if std::fs::symlink_metadata(&plan.destination).is_err() {
            DestinationStatus::Absent
        } else {
            match review_reconstruction_publication(&plan, None) {
                Ok(review)
                    if matches!(
                        review.action(),
                        ReconstructionPublicationAction::Unchanged { .. }
                    ) =>
                {
                    DestinationStatus::AlreadyPublished
                }
                Ok(_) => DestinationStatus::Occupied("an existing file is there".into()),
                Err(first) => match target {
                    None => DestinationStatus::Occupied(first),
                    Some(evidence) => {
                        match review_reconstruction_publication(&plan, Some(evidence)) {
                            Ok(review) => match review.action() {
                                ReconstructionPublicationAction::ReplaceExisting { original } => {
                                    match replacement_member_changes(&plan, dat) {
                                        Ok(changes) => {
                                            let mut reasons =
                                                replacement_refusals(&changes, &dependencies);
                                            // The accounting must describe the
                                            // exact object review captured.
                                            if !capture_identity(&plan.destination).is_ok_and(
                                                |after| identity_matches(original, &after),
                                            ) {
                                                reasons.push(
                                                    "the archive changed while it was being checked; preview again".into(),
                                                );
                                            }
                                            if reasons.is_empty() {
                                                gate = publish_gate(review.plan());
                                                DestinationStatus::Replaceable {
                                                    original: Box::new(original.clone()),
                                                    changes,
                                                }
                                            } else {
                                                DestinationStatus::ReplacementRefused {
                                                    changes,
                                                    reasons,
                                                }
                                            }
                                        }
                                        Err(error) => DestinationStatus::Occupied(error),
                                    }
                                }
                                _ => {
                                    DestinationStatus::Occupied("an existing file is there".into())
                                }
                            },
                            Err(error) => DestinationStatus::Occupied(error),
                        }
                    }
                },
            }
        };
        let source_is_extracted = plan
            .sources
            .iter()
            .map(|source| source.archive_path.is_dir())
            .collect();
        Self {
            inputs,
            catalogue: None,
            plan,
            dependencies,
            destination,
            gate,
            source_is_extracted,
        }
    }
}

/// Where the background jobs read evidence and keep recovery journals.
/// `None` resolves the application defaults on the worker thread.
#[derive(Debug, Clone, Default)]
pub(super) struct MameEnvironment {
    pub database_path: Option<PathBuf>,
    pub journal_dir: Option<PathBuf>,
    #[cfg(test)]
    pub catalogue_inventory: Option<crate::dat_catalogue_picker::CatalogueInventorySnapshot>,
}

impl MameEnvironment {
    pub(super) fn journal_dir(&self) -> Result<PathBuf, String> {
        match &self.journal_dir {
            Some(path) => Ok(path.clone()),
            None => default_rename_transaction_dir().map_err(|error| error.to_string()),
        }
    }

    fn database_path(&self) -> Result<PathBuf, String> {
        match &self.database_path {
            Some(path) => Ok(path.clone()),
            None => default_database_path().map_err(|error| error.to_string()),
        }
    }
}

pub(super) fn staging_root(root: &Path) -> PathBuf {
    root.join(".emuwiz-mame-staging")
}

/// A valid parent with no clones: there is nothing to merge.
#[derive(Debug, Clone)]
pub(super) struct StandaloneGame {
    pub inputs: MameInputs,
    pub parent: String,
    pub dependencies: ReconstructionDependencies,
}

#[derive(Debug, Clone)]
pub(super) enum PreviewOutcome {
    Ready(Box<MamePreview>),
    NotNeeded(Box<StandaloneGame>),
}

enum CurrentFamily {
    Plan(
        Box<MameMergedReconstructionPlan>,
        ReconstructionDependencies,
        Option<Box<ArcadeJoinEvidence>>,
        Box<ParsedDat>,
    ),
    Standalone(String, ReconstructionDependencies),
}

/// The current plan and dependency report for `inputs`, from the verified
/// explicitly selected version-bound catalogue, family evidence and packed ZIPs.
/// A clone-free parent is classified before any evidence is read.
fn current_preview(
    inputs: &MameInputs,
    environment: &MameEnvironment,
) -> Result<(CurrentFamily, CatalogueBinding), String> {
    let set = inputs.set.trim();
    if set.is_empty() {
        return Err("Enter a parent or clone set name before previewing.".into());
    }
    let selected = load_selected(inputs, environment)?;
    let dat = selected.dat;
    let binding = selected.binding;
    match reconstruction_family_shape(&dat.parsed, set)? {
        ReconstructionFamilyShape::ParentWithClones { .. } => {}
        ReconstructionFamilyShape::StandaloneParent { parent } => {
            let dependencies = reconstruction_dependencies(&dat.parsed, set)?;
            return Ok((CurrentFamily::Standalone(parent, dependencies), binding));
        }
        ReconstructionFamilyShape::IncompleteIdentity { reason } => {
            return Err(format!(
                "The catalogue does not prove this set's parent/clone identity: {reason}. Nothing can be rebuilt for it."
            ));
        }
    }
    let family = reconstruction_family_names(&dat.parsed, set)?;
    let database = Database::open_read_only(&environment.database_path()?)
        .map_err(|error| error.to_string())?;
    let persisted = database
        .mame_arcade_join_paths_for_dat_sets(&dat.sha256, Some(&family))
        .map_err(|error| error.to_string())?;
    let joins = combine_joins(
        persisted,
        discover_packed_zip_sources(&inputs.root, &dat.parsed, set, &dat.sha256)?,
    );
    if joins.is_empty() {
        return Err(
            "no persisted verified MAME evidence is available; refresh this family first".into(),
        );
    }
    let plan =
        build_merged_reconstruction_plan(&inputs.root, &dat.parsed, &joins, set, &dat.sha256)?;
    let dependencies = reconstruction_dependencies(&dat.parsed, set)?;
    let target = target_evidence(&joins, &plan).map(Box::new);
    Ok((
        CurrentFamily::Plan(Box::new(plan), dependencies, target, Box::new(dat.parsed)),
        binding,
    ))
}

// ---------------------------------------------------------------------------
// Publish.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) enum PublishOutcome {
    /// Staged, verified, published through the journal, every entry applied,
    /// and the published copy re-verified.
    Published {
        destination: PathBuf,
        transaction: Box<RenameTransaction>,
    },
    /// The destination already holds exactly this set; nothing was written.
    AlreadyPublished { destination: PathBuf },
    /// Refused before the destination was touched.
    Refused(MameRefusal),
    /// The journal began and the outcome is not a verified publication. The
    /// transaction (when one can be found) is the recovery record.
    Failed {
        reason: String,
        transaction: Option<Box<RenameTransaction>>,
    },
}

#[derive(Debug, Clone)]
pub(super) struct PublishResult {
    pub outcome: PublishOutcome,
    /// A freshly built plan to show when the reviewed one was stale.
    pub refreshed_plan: Option<MameMergedReconstructionPlan>,
}

pub(super) struct MameContext<'a> {
    pub staging_root: &'a Path,
    pub journal_dir: &'a Path,
}

/// Publishes the reviewed preview, or refuses. `fresh` is the plan and target
/// evidence the backend would build now; any plan difference is a stale
/// refusal. A replacement also requires the existing archive to be the exact
/// object the person reviewed.
pub(super) fn run_publish(
    reviewed: &MamePreview,
    fresh: Result<(MameMergedReconstructionPlan, Option<ArcadeJoinEvidence>), String>,
    context: &MameContext<'_>,
) -> PublishResult {
    let refuse = |refusal: MameRefusal| PublishResult {
        outcome: PublishOutcome::Refused(refusal),
        refreshed_plan: None,
    };
    let reviewed_original = match &reviewed.destination {
        DestinationStatus::Absent => None,
        DestinationStatus::Replaceable { original, .. } => Some(original.as_ref()),
        DestinationStatus::AlreadyPublished => {
            return PublishResult {
                outcome: PublishOutcome::AlreadyPublished {
                    destination: reviewed.plan.destination.clone(),
                },
                refreshed_plan: None,
            };
        }
        DestinationStatus::Occupied(reason) => {
            return refuse(MameRefusal::DestinationCollision(reason.clone()));
        }
        DestinationStatus::ReplacementRefused { reasons, .. } => {
            return refuse(MameRefusal::DestinationCollision(reasons.join("; ")));
        }
    };
    if let PublishGate::Blocked(reasons) = reviewed.gate() {
        return refuse(MameRefusal::PlanBlocked(
            reasons
                .iter()
                .map(BlockReason::describe)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    let (fresh, fresh_target) = match fresh {
        Ok(fresh) => fresh,
        Err(error) => {
            return refuse(match MameRefusal::from_backend(&error) {
                MameRefusal::Other(detail) => MameRefusal::PlanStale(format!(
                    "the plan could not be rebuilt to compare with what you reviewed: {detail}"
                )),
                other => other,
            });
        }
    };
    if fresh != reviewed.plan {
        return PublishResult {
            outcome: PublishOutcome::Refused(MameRefusal::PlanStale(
                "its evidence, sources or destination changed since you reviewed it".into(),
            )),
            refreshed_plan: Some(fresh),
        };
    }
    // Source- and target-bound review. Target evidence is supplied only for a
    // replacement the person reviewed, so a create can never replace.
    let target = match (reviewed_original, &fresh_target) {
        (None, _) => None,
        (Some(_), Some(target)) => Some(target),
        (Some(_), None) => {
            return refuse(MameRefusal::DestinationChanged(
                "checksum evidence for the existing archive is no longer available".into(),
            ));
        }
    };
    let publication = match review_reconstruction_publication(&fresh, target) {
        Ok(publication) => publication,
        Err(error) => {
            return refuse(match MameRefusal::from_backend(&error) {
                // At review time only an existing destination is inspected as
                // a ZIP, so an inspection failure is a collision.
                MameRefusal::VerificationFailed(detail) => {
                    MameRefusal::DestinationCollision(format!(
                        "the existing file at the destination is not safe to publish over ({detail})"
                    ))
                }
                other => other,
            });
        }
    };
    let destination = fresh.destination.clone();
    match (reviewed_original, publication.action()) {
        (_, ReconstructionPublicationAction::Unchanged { .. }) => {
            return PublishResult {
                outcome: PublishOutcome::AlreadyPublished { destination },
                refreshed_plan: None,
            };
        }
        (None, ReconstructionPublicationAction::Create) => {}
        (Some(expected), ReconstructionPublicationAction::ReplaceExisting { original })
            if identity_matches(expected, original) => {}
        (Some(_), ReconstructionPublicationAction::ReplaceExisting { .. }) => {
            return refuse(MameRefusal::DestinationChanged(
                "the existing archive changed after you reviewed its replacement".into(),
            ));
        }
        (Some(_), ReconstructionPublicationAction::Create) => {
            return refuse(MameRefusal::DestinationChanged(
                "the archive you reviewed for replacement is no longer there".into(),
            ));
        }
        (None, ReconstructionPublicationAction::ReplaceExisting { .. }) => {
            return refuse(MameRefusal::DestinationCollision(
                "replacing an existing archive needs its own review".into(),
            ));
        }
    }
    let applied = match publication.apply(context.staging_root, context.journal_dir) {
        Ok(applied) => applied,
        Err(error) => {
            // An error that names a reserved journal is not "nothing
            // changed": that transaction is the recovery record. An error
            // without one was raised before any journal existed, so the
            // destination was never touched.
            let Some(id) = reserved_transaction_id(&error) else {
                return refuse(MameRefusal::from_backend(&error));
            };
            let (history, _) = load_history(context.journal_dir);
            let recorded = history.into_iter().find(|t| t.transaction_id == id);
            let reason = match &recorded {
                Some(t) => format!(
                    "{error}. Transaction {id} is recorded as \"{}\"",
                    t.state.label()
                ),
                None => format!(
                    "{error}. Its recovery journal {id} could not be read, so the outcome is uncertain; check Recovery before trying again"
                ),
            };
            return PublishResult {
                outcome: PublishOutcome::Failed {
                    reason,
                    transaction: recorded.map(Box::new),
                },
                refreshed_plan: None,
            };
        }
    };
    let Some(applied) = applied else {
        return PublishResult {
            outcome: PublishOutcome::AlreadyPublished { destination },
            refreshed_plan: None,
        };
    };
    let transaction = applied.transaction;
    if !publication_succeeded(&transaction) {
        return PublishResult {
            outcome: PublishOutcome::Failed {
                reason: format!(
                    "transaction {} ended as \"{}\"{}",
                    transaction.transaction_id,
                    transaction.state.label(),
                    failure_reasons(&transaction)
                ),
                transaction: Some(Box::new(transaction)),
            },
            refreshed_plan: None,
        };
    }
    // Success is never claimed from the journal alone.
    if let Err(error) = verify_staged_output(&fresh, &destination) {
        return PublishResult {
            outcome: PublishOutcome::Failed {
                reason: format!(
                    "the published copy failed re-verification ({error}); it can be undone from the history below"
                ),
                transaction: Some(Box::new(transaction)),
            },
            refreshed_plan: None,
        };
    }
    PublishResult {
        outcome: PublishOutcome::Published {
            destination,
            transaction: Box::new(transaction),
        },
        refreshed_plan: None,
    }
}

// ---------------------------------------------------------------------------
// Undo.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) enum UndoOutcome {
    /// Every receipt agrees an output was reversed.
    RolledBack { transaction: Box<RenameTransaction> },
    /// The transaction had published nothing; it is now closed.
    ClosedWithoutPublication { transaction: Box<RenameTransaction> },
    /// The rollback ran and did not verifiably complete.
    Incomplete {
        message: String,
        transaction: Box<RenameTransaction>,
    },
    /// Refused before anything moved.
    Refused(String),
    /// The rollback stopped with an error; the journal is the record.
    Failed(String),
}

/// Undoes one reconstruction through the shared journalled rollback. The
/// transaction is re-read from its journal so the decision uses the durable
/// record, not a copy cached by the page.
pub(super) fn run_undo(transaction_id: &str, journal_dir: &Path) -> UndoOutcome {
    let (history, _) = load_history(journal_dir);
    let Some(mut transaction) = history
        .into_iter()
        .find(|transaction| transaction.transaction_id == transaction_id)
    else {
        return UndoOutcome::Refused(
            "That MAME transaction is no longer in the recovery journal.".into(),
        );
    };
    if !transaction.is_rollbackable() {
        return UndoOutcome::Refused(format!(
            "Undo is unavailable for this MAME transaction; its recorded state is {}.",
            transaction.state.label()
        ));
    }
    if !undo_actionable(&transaction) {
        return UndoOutcome::Refused(format!(
            "Undo is unavailable for this MAME transaction: its recorded state is \"{}\" and it has no published output an undo could act on{}. Nothing was changed; the transaction and its journal are kept for review.",
            transaction.state.label(),
            failure_reasons(&transaction)
        ));
    }
    let Some(destination_parent) = transaction
        .entries
        .first()
        .and_then(|entry| entry.destination_path.parent())
        .map(Path::to_path_buf)
    else {
        return UndoOutcome::Refused(
            "Undo is unavailable because the journal has no destination.".into(),
        );
    };
    let staging = PathBuf::from(&transaction.source_scan_root);
    let cancel = AtomicBool::new(false);
    let trusted = TrustedRoots::from_paths([staging.as_path(), destination_parent.as_path()]);
    match rollback_transaction_confined(&mut transaction, journal_dir, &cancel, &trusted) {
        Ok(outcome) => classify_rollback(outcome),
        Err(error) => UndoOutcome::Failed(format!(
            "MAME undo stopped safely and needs review: {error}"
        )),
    }
}

// ---------------------------------------------------------------------------
// Finalise Replacement.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FinaliseAction {
    /// Delete the preserved original and mark the replacement final; also
    /// resumes an interrupted finalisation.
    Finish,
    /// Drop an interrupted finalisation record so Undo works again.
    CancelInterrupted,
}

#[derive(Debug, Clone)]
pub(super) enum FinaliseOutcome {
    /// The original is gone and the journal records the replacement as final.
    Finalised {
        transaction_id: String,
        deleted: PathBuf,
    },
    /// The interrupted finalisation was cancelled; Undo is available again.
    Cancelled { transaction_id: String },
    /// Refused or stopped; the message states what was and was not changed.
    Refused(String),
}

/// Runs one Finalise Replacement step against the durable journal and a fresh
/// staging inventory, never the page's cached copy. `exclusive_ack` is the
/// person's statement that nothing else is using staging or the journals.
pub(super) fn run_finalise(
    transaction_id: &str,
    action: FinaliseAction,
    exclusive_ack: bool,
    journal_dir: &Path,
) -> FinaliseOutcome {
    let (history, _) = load_history(journal_dir);
    let Some(transaction) = history
        .into_iter()
        .find(|transaction| transaction.transaction_id == transaction_id)
    else {
        return FinaliseOutcome::Refused(
            "That MAME transaction is no longer in the recovery journal. Nothing was changed."
                .into(),
        );
    };
    let staging = PathBuf::from(&transaction.source_scan_root);
    match action {
        FinaliseAction::CancelInterrupted => {
            match staging_lifecycle::cancel_interrupted_finalise(
                &staging,
                journal_dir,
                transaction_id,
            ) {
                Ok(()) => FinaliseOutcome::Cancelled {
                    transaction_id: transaction_id.into(),
                },
                Err(error) => FinaliseOutcome::Refused(error),
            }
        }
        FinaliseAction::Finish => {
            if !finalise_actionable(&transaction) && !finalise_interrupted(&transaction) {
                return FinaliseOutcome::Refused(
                    "This transaction is not a verified, un-finalised MAME replacement. Nothing was changed.".into(),
                );
            }
            if !exclusive_ack {
                return FinaliseOutcome::Refused(
                    "Confirm that nothing else is using the MAME staging folder or journals. Nothing was changed.".into(),
                );
            }
            let Some(path) = transaction.entries.first().map(|e| e.source_path.clone()) else {
                return FinaliseOutcome::Refused(
                    "The journal has no entry. Nothing was changed.".into(),
                );
            };
            let items = match staging_lifecycle::inventory(&staging, journal_dir) {
                Ok(items) => items,
                Err(error) => {
                    return FinaliseOutcome::Refused(format!(
                        "The staging inventory could not be proven: {error}. Nothing was changed."
                    ));
                }
            };
            let item = items.into_iter().find(|item| item.path == path);
            match item {
                Some(item) if item.finalise_eligible => staging_lifecycle::finalise_replacement(
                    &staging,
                    journal_dir,
                    &item,
                    FINALISE_CONFIRMATION,
                    "I HAVE EXCLUSIVE STAGING AND JOURNAL ACCESS",
                )
                .map(|report| FinaliseOutcome::Finalised {
                    transaction_id: report.transaction_id,
                    deleted: report.deleted_original,
                })
                .unwrap_or_else(FinaliseOutcome::Refused),
                // The interrupted run already removed the original: only the
                // journal still needs to say so.
                _ if finalise_interrupted(&transaction) => {
                    staging_lifecycle::complete_interrupted_finalise(
                        &staging,
                        journal_dir,
                        transaction_id,
                    )
                    .map(|report| FinaliseOutcome::Finalised {
                        transaction_id: report.transaction_id,
                        deleted: report.deleted_original,
                    })
                    .unwrap_or_else(FinaliseOutcome::Refused)
                }
                Some(item) => FinaliseOutcome::Refused(format!(
                    "Not finalised: {}. Nothing was changed.",
                    item.retention_reason
                )),
                None => FinaliseOutcome::Refused(
                    "The preserved original is not in the staging inventory. Nothing was changed."
                        .into(),
                ),
            }
        }
    }
}

/// The result of a rollback that ran. It is called rolled back only when the
/// rollback result, the transaction state and every entry receipt agree
/// (the 470010cb rule).
pub(super) fn classify_rollback(outcome: RollbackOutcome) -> UndoOutcome {
    let receipts_agree = matches!(outcome.result, RollbackResult::FullyRolledBack)
        && outcome.transaction.is_fully_rolled_back();
    if receipts_agree && rollback_verified(&outcome.transaction) {
        UndoOutcome::RolledBack {
            transaction: Box::new(outcome.transaction),
        }
    } else if receipts_agree {
        UndoOutcome::ClosedWithoutPublication {
            transaction: Box::new(outcome.transaction),
        }
    } else {
        let failed = outcome
            .result
            .failed()
            .into_iter()
            .map(|(path, reason)| format!("{} ({reason})", path.display()))
            .collect::<Vec<_>>();
        let recorded = if outcome.transaction.state == TransactionState::RolledBack {
            format!(
                "is recorded as \"Rolled back\" but its entries do not confirm that (entries: {})",
                entry_states(&outcome.transaction)
            )
        } else {
            format!("ended as \"{}\"", outcome.transaction.state.label())
        };
        UndoOutcome::Incomplete {
            message: format!(
                "MAME undo did not complete: transaction {} {recorded}{}. Any output that was not undone was left as it is; review this transaction before trying again. The source archives were not touched.",
                outcome.transaction.transaction_id,
                if failed.is_empty() {
                    failure_reasons(&outcome.transaction)
                } else {
                    format!(": {}", failed.join("; "))
                }
            ),
            transaction: Box::new(outcome.transaction),
        }
    }
}

// ---------------------------------------------------------------------------
// Background jobs.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) enum MameRequest {
    /// Re-reads ROM checksums into the evidence database. ROMs are not changed.
    RefreshEvidence {
        inputs: MameInputs,
        only_family: bool,
    },
    Preview {
        inputs: MameInputs,
    },
    Publish {
        preview: Box<MamePreview>,
    },
    Undo {
        transaction_id: String,
    },
    Finalise {
        transaction_id: String,
        action: FinaliseAction,
        exclusive_ack: bool,
    },
    LoadHistory {
        root: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MameJobKind {
    RefreshEvidence,
    Preview,
    Publish,
    Undo,
    Finalise,
    LoadHistory,
}

impl MameRequest {
    pub(super) fn kind(&self) -> MameJobKind {
        match self {
            Self::RefreshEvidence { .. } => MameJobKind::RefreshEvidence,
            Self::Preview { .. } => MameJobKind::Preview,
            Self::Publish { .. } => MameJobKind::Publish,
            Self::Undo { .. } => MameJobKind::Undo,
            Self::Finalise { .. } => MameJobKind::Finalise,
            Self::LoadHistory { .. } => MameJobKind::LoadHistory,
        }
    }
}

impl MameJobKind {
    fn working_label(self) -> &'static str {
        match self {
            Self::RefreshEvidence => "Re-checking the files inside your sets",
            Self::Preview => "Building the reconstruction preview",
            Self::Publish => "Publishing the reconstructed set",
            Self::Undo => "Undoing the reconstruction",
            Self::Finalise => "Finalising the replacement",
            Self::LoadHistory => "Reading the MAME rebuild history",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum MameResult {
    Evidence(Result<String, String>),
    Preview(Result<PreviewOutcome, String>),
    Publish(Box<PublishResult>),
    Undo(UndoOutcome),
    Finalise(FinaliseOutcome),
    History,
}

/// What a finished job hands back: its result plus the journal history read
/// after it ran (`None` when the job does not touch journals).
/// Readable MAME journals plus the journals that could not be read, or why
/// the journal directory itself was unavailable.
pub(super) type HistoryRead = Result<(Vec<RenameTransaction>, Vec<String>), String>;

#[derive(Debug, Clone)]
pub(super) struct MameJobReport {
    pub result: MameResult,
    pub history: Option<HistoryRead>,
    pub staging: Option<(PathBuf, Result<Vec<StagingItem>, String>)>,
}

pub(super) type MameExecutor =
    Arc<dyn Fn(MameRequest, &MameEnvironment, &mut dyn FnMut(&str)) -> MameJobReport + Send + Sync>;

fn reload_history(environment: &MameEnvironment) -> Option<HistoryRead> {
    Some(environment.journal_dir().map(|dir| load_history(&dir)))
}

fn reload_staging(
    root: &Path,
    environment: &MameEnvironment,
) -> (PathBuf, Result<Vec<StagingItem>, String>) {
    (
        root.to_owned(),
        environment
            .journal_dir()
            .and_then(|journal| staging_lifecycle::inventory(&staging_root(root), &journal)),
    )
}

/// Production worker body, run on a background thread.
pub(super) fn execute(
    request: MameRequest,
    environment: &MameEnvironment,
    phase: &mut dyn FnMut(&str),
) -> MameJobReport {
    match request {
        MameRequest::RefreshEvidence {
            inputs,
            only_family,
        } => {
            phase("Reading the verified MAME catalogue");
            let result = (|| {
                let dat = load_selected(&inputs, environment)?.dat;
                let mut database = Database::open_or_create(&environment.database_path()?)
                    .map_err(|error| format!("Could not open the evidence database: {error}"))?;
                phase("Hashing the ROM files inside your sets");
                let family = only_family.then(|| inputs.set.trim().to_string());
                let report = refresh_mame_member_evidence(
                    &mut database,
                    &dat,
                    &inputs.root,
                    family.as_deref(),
                )
                .map_err(|error| format!("Evidence refresh stopped safely: {error}"))?;
                Ok(format!(
                    "Evidence refresh complete: {} sets, {} members; ROM files were not changed.",
                    report.sets_published, report.members_seen
                ))
            })();
            MameJobReport {
                result: MameResult::Evidence(result),
                history: None,
                staging: None,
            }
        }
        MameRequest::Preview { inputs } => {
            phase("Reading the catalogue, evidence and packed sets");
            let result =
                current_preview(&inputs, environment).map(|(family, binding)| match family {
                    CurrentFamily::Plan(plan, dependencies, target, dat) => {
                        phase("Checking the destination");
                        let mut preview = MamePreview::from_plan(
                            inputs,
                            *plan,
                            dependencies,
                            target.as_deref(),
                            &dat,
                        );
                        preview.catalogue = Some(binding);
                        PreviewOutcome::Ready(Box::new(preview))
                    }
                    CurrentFamily::Standalone(parent, dependencies) => {
                        PreviewOutcome::NotNeeded(Box::new(StandaloneGame {
                            inputs,
                            parent,
                            dependencies,
                        }))
                    }
                });
            MameJobReport {
                result: MameResult::Preview(result),
                history: None,
                staging: None,
            }
        }
        MameRequest::Publish { preview } => {
            let journal_dir = match environment.journal_dir() {
                Ok(path) => path,
                Err(error) => {
                    return MameJobReport {
                        result: MameResult::Publish(Box::new(PublishResult {
                            outcome: PublishOutcome::Refused(MameRefusal::JournalUnavailable(
                                error,
                            )),
                            refreshed_plan: None,
                        })),
                        history: None,
                        staging: None,
                    };
                }
            };
            phase("Checking the reviewed plan is still current");
            let fresh = current_preview(&preview.inputs, environment).and_then(|(family, binding)| {
                if preview.catalogue.as_ref() != Some(&binding) {
                    return Err("Selected catalogue identity, version or digest changed; nothing was staged. Select it and Preview again.".into());
                }
                match family {
                    CurrentFamily::Plan(plan, _, target, _) => Ok((*plan, target.map(|t| *t))),
                    CurrentFamily::Standalone(parent, _) => Err(format!("{parent} no longer has clones in the selected catalogue")),
                }
            });
            phase("Staging and verifying the new set, then publishing it");
            // Catalogue refusal precedes ALL backend shortcuts, including
            // "already published"; keep a stale selection from claiming success.
            let fresh = fresh.and_then(|fresh| {
                if preview
                    .catalogue
                    .as_ref()
                    .is_some_and(|binding| binding.source.still_matches())
                {
                    Ok(fresh)
                } else {
                    Err("Selected catalogue changed before staging; Preview again.".into())
                }
            });
            let fresh = match fresh {
                Ok(fresh) => fresh,
                Err(error) => {
                    let refusal = match MameRefusal::from_backend(&error) {
                        MameRefusal::Other(detail) => MameRefusal::PlanStale(detail),
                        refusal => refusal,
                    };
                    return MameJobReport {
                        result: MameResult::Publish(Box::new(PublishResult {
                            outcome: PublishOutcome::Refused(refusal),
                            refreshed_plan: None,
                        })),
                        history: None,
                        staging: None,
                    };
                }
            };
            let staging = staging_root(&preview.inputs.root);
            let result = run_publish(
                &preview,
                Ok(fresh),
                &MameContext {
                    staging_root: &staging,
                    journal_dir: &journal_dir,
                },
            );
            MameJobReport {
                result: MameResult::Publish(Box::new(result)),
                history: reload_history(environment),
                staging: Some(reload_staging(&preview.inputs.root, environment)),
            }
        }
        MameRequest::Undo { transaction_id } => {
            phase("Checking the published copy and undoing it");
            let outcome = match environment.journal_dir() {
                Ok(dir) => run_undo(&transaction_id, &dir),
                Err(error) => {
                    UndoOutcome::Failed(format!("MAME recovery journal is unavailable: {error}"))
                }
            };
            MameJobReport {
                result: MameResult::Undo(outcome),
                history: reload_history(environment),
                staging: None,
            }
        }
        MameRequest::Finalise {
            transaction_id,
            action,
            exclusive_ack,
        } => {
            phase("Re-checking the published copy and the preserved original");
            let outcome = match environment.journal_dir() {
                Ok(dir) => run_finalise(&transaction_id, action, exclusive_ack, &dir),
                Err(error) => FinaliseOutcome::Refused(format!(
                    "MAME recovery journal is unavailable: {error}. Nothing was changed."
                )),
            };
            MameJobReport {
                result: MameResult::Finalise(outcome),
                history: reload_history(environment),
                staging: None,
            }
        }
        MameRequest::LoadHistory { root } => MameJobReport {
            result: MameResult::History,
            history: reload_history(environment),
            staging: root
                .as_deref()
                .map(|root| reload_staging(root, environment)),
        },
    }
}

enum JobEvent {
    Phase(String),
    Done(Box<MameJobReport>),
}

struct RunningJob {
    kind: MameJobKind,
    phase: String,
    events: mpsc::Receiver<JobEvent>,
}

// ---------------------------------------------------------------------------
// Page state.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Info,
    Success,
    Warning,
    Danger,
}

impl Tone {
    fn colour(self, ui: &egui::Ui) -> egui::Color32 {
        match self {
            Self::Info => theme::info(ui),
            Self::Success => theme::success(ui),
            Self::Warning => theme::warning(ui),
            Self::Danger => theme::danger(ui),
        }
    }
}

const NO_FOLDER_SELECTED: &str =
    "Choose a MAME folder first. Re-check and Preview are unavailable until one is selected.";
const NO_CATALOGUE_SELECTED: &str =
    "Select a MAME catalogue first. Re-check and Preview compare the folder against it.";
const NO_SET_SELECTED: &str =
    "Enter a parent or clone set name to re-check that family or preview it.";
const BUSY: &str = "Available when the current MAME step has finished.";

/// Why Re-check and Preview cannot run yet, checked in the page's reading
/// order. Set selection is separate: only the family actions need one.
pub(super) fn selection_blocker(has_folder: bool, has_catalogue: bool) -> Option<&'static str> {
    if !has_folder {
        Some(NO_FOLDER_SELECTED)
    } else if !has_catalogue {
        Some(NO_CATALOGUE_SELECTED)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Notice {
    pub tone: Tone,
    pub headline: String,
    pub lines: Vec<String>,
}

impl Notice {
    fn new(tone: Tone, headline: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            tone,
            headline: headline.into(),
            lines,
        }
    }
}

pub(super) struct MameReconstructionPanel {
    pub root: Option<PathBuf>,
    pub dat: Option<PathBuf>,
    pub catalogue: Option<CatalogueRef>,
    catalogue_trusted: bool,
    catalogue_picker: DatCataloguePickerState,
    pub set: String,
    pub(super) preview: Option<MamePreview>,
    pub(super) standalone: Option<StandaloneGame>,
    pub(super) confirmation: String,
    pub(super) publish_pending: bool,
    pub(super) undo_pending: Option<String>,
    pub(super) finalise_pending: Option<String>,
    pub(super) finalise_ack: bool,
    pub(super) history: Vec<RenameTransaction>,
    pub(super) history_problems: Vec<String>,
    pub(super) unjournaled_stages: Vec<StagingItem>,
    pub(super) staging_problem: Option<String>,
    history_requested: bool,
    history_root: Option<PathBuf>,
    pub(super) notice: Option<Notice>,
    job: Option<RunningJob>,
    environment: MameEnvironment,
    executor: MameExecutor,
    playing: playing::Panel,
}

impl Default for MameReconstructionPanel {
    fn default() -> Self {
        Self::with_executor(MameEnvironment::default(), Arc::new(execute))
    }
}

impl MameReconstructionPanel {
    pub(super) fn with_executor(environment: MameEnvironment, executor: MameExecutor) -> Self {
        Self {
            root: None,
            dat: None,
            catalogue: None,
            catalogue_trusted: false,
            catalogue_picker: DatCataloguePickerState::default(),
            set: String::new(),
            preview: None,
            standalone: None,
            confirmation: String::new(),
            publish_pending: false,
            undo_pending: None,
            finalise_pending: None,
            finalise_ack: false,
            history: Vec::new(),
            history_problems: Vec::new(),
            unjournaled_stages: Vec::new(),
            staging_problem: None,
            history_requested: false,
            history_root: None,
            notice: None,
            job: None,
            environment,
            executor,
            playing: playing::Panel::default(),
        }
    }

    pub(super) fn is_busy(&self) -> bool {
        self.job.is_some() || self.playing.launch_active()
    }

    #[cfg(test)]
    pub(super) fn running_kind(&self) -> Option<MameJobKind> {
        self.job.as_ref().map(|job| job.kind)
    }

    fn inputs(&self) -> Option<MameInputs> {
        Some(MameInputs {
            root: self.root.clone()?,
            dat: self.dat.clone()?,
            reference: self.catalogue.clone()?,
            trusted: self.catalogue_trusted,
            set: self.set.trim().to_string(),
        })
    }

    /// A preview is current only while the folder, DAT and set are unchanged.
    pub(super) fn preview_is_current(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|preview| self.inputs().as_ref() == Some(&preview.inputs))
    }

    /// Publish may be confirmed only for a current, ready preview whose
    /// destination is free, and never while another job runs.
    pub(super) fn can_publish(&self) -> bool {
        !self.is_busy()
            && self.catalogue_trusted
            && self.preview_is_current()
            && self.preview.as_ref().is_some_and(|preview| {
                preview.gate().is_ready()
                    && matches!(
                        preview.destination,
                        DestinationStatus::Absent | DestinationStatus::Replaceable { .. }
                    )
            })
    }

    /// The typed phrase for the action the current preview offers: creating a
    /// missing parent ZIP, or replacing the named existing one.
    pub(super) fn required_phrase(&self) -> Option<String> {
        let preview = self.preview.as_ref()?;
        Some(if preview.is_replacement() {
            replace_confirmation_phrase(&preview.plan.destination)
        } else {
            publish_confirmation_phrase(1)
        })
    }

    /// Starts `request` on a background thread. Refused while a job runs.
    pub(super) fn start(&mut self, request: MameRequest) -> bool {
        if self.is_busy() {
            return false;
        }
        let kind = request.kind();
        let (sender, events) = mpsc::channel();
        let executor = Arc::clone(&self.executor);
        let environment = self.environment.clone();
        let spawned = std::thread::Builder::new()
            .name("emuwiz-mame-reconstruction".into())
            .spawn(move || {
                let phase_sender = sender.clone();
                let mut phase = |text: &str| {
                    let _ = phase_sender.send(JobEvent::Phase(text.to_string()));
                };
                let report = executor(request, &environment, &mut phase);
                let _ = sender.send(JobEvent::Done(Box::new(report)));
            });
        if let Err(error) = spawned {
            self.notice = Some(Notice::new(
                Tone::Danger,
                "EmuWiz could not start the background job",
                vec![format!("{error}. Nothing was changed.")],
            ));
            return false;
        }
        self.job = Some(RunningJob {
            kind,
            phase: kind.working_label().into(),
            events,
        });
        true
    }

    /// Drains job events. Returns true when a job finished this call.
    pub(super) fn poll(&mut self) -> bool {
        self.playing.poll();
        let Some(job) = self.job.as_mut() else {
            return false;
        };
        loop {
            match job.events.try_recv() {
                Ok(JobEvent::Phase(text)) => job.phase = text,
                Ok(JobEvent::Done(report)) => {
                    self.job = None;
                    self.finish(*report);
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let kind = job.kind;
                    self.job = None;
                    self.notice = Some(Notice::new(
                        Tone::Danger,
                        "The background job stopped unexpectedly",
                        vec![if matches!(
                            kind,
                            MameJobKind::Publish | MameJobKind::Undo | MameJobKind::Finalise
                        ) {
                            "Its outcome is unknown. Nothing is reported as published or undone; check the history below and Recovery before trying again.".into()
                        } else {
                            "Nothing was changed.".into()
                        }],
                    ));
                    self.history_requested = false;
                    return true;
                }
            }
        }
    }

    fn finish(&mut self, report: MameJobReport) {
        if let Some((_, staging)) = report
            .staging
            .filter(|(root, _)| self.root.as_ref() == Some(root))
        {
            match staging {
                Ok(items) => {
                    self.unjournaled_stages = items
                        .into_iter()
                        .filter(|item| {
                            item.transaction_id.is_none()
                                && item
                                    .path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .is_some_and(|name| name.ends_with(".zip.staged"))
                        })
                        .collect();
                    self.staging_problem = None;
                }
                Err(error) => {
                    self.unjournaled_stages.clear();
                    self.staging_problem = Some(error);
                }
            }
        }
        if let Some(history) = report.history {
            match history {
                Ok((history, problems)) => {
                    self.history = history;
                    self.history_problems = problems;
                }
                Err(error) => {
                    self.history_problems =
                        vec![format!("The recovery journal is unavailable: {error}")];
                }
            }
        }
        match report.result {
            MameResult::Evidence(Ok(message)) => {
                // New evidence makes any earlier preview out of date.
                self.preview = None;
                self.standalone = None;
                self.notice = Some(Notice::new(Tone::Info, message, Vec::new()));
            }
            MameResult::Evidence(Err(message)) => {
                self.notice = Some(Notice::new(Tone::Warning, message, Vec::new()));
            }
            MameResult::Preview(Ok(PreviewOutcome::NotNeeded(game))) => {
                self.preview = None;
                self.publish_pending = false;
                self.confirmation.clear();
                self.notice = Some(Notice::new(
                    Tone::Info,
                    "No merged parent/clone reconstruction is needed for this game",
                    vec![
                        format!(
                            "{} is a parent set with no clones in the selected catalogue, so its merged, split and non-merged sets hold the same ROM files. There is nothing to merge, and EmuWiz does not rebuild standalone sets here.",
                            game.parent
                        ),
                        "To check whether its files are complete, use MAME Collection Health. Its dependencies are listed below.".into(),
                    ],
                ));
                self.standalone = Some(*game);
            }
            MameResult::Preview(Ok(PreviewOutcome::Ready(preview))) => {
                self.standalone = None;
                self.preview = Some(*preview);
                self.publish_pending = false;
                self.confirmation.clear();
                self.notice = None;
            }
            MameResult::Preview(Err(error)) => {
                self.preview = None;
                self.standalone = None;
                self.notice = Some(Notice::new(
                    Tone::Warning,
                    "No preview could be built",
                    vec![error, "Nothing was changed.".into()],
                ));
            }
            MameResult::Publish(result) => self.finish_publish(*result),
            MameResult::Undo(outcome) => self.finish_undo(outcome),
            MameResult::Finalise(outcome) => self.finish_finalise(outcome),
            MameResult::History => {}
        }
    }

    fn finish_publish(&mut self, result: PublishResult) {
        // Any publication outcome invalidates cached readiness. Successful
        // changes are re-inspected, never promoted from a receipt into playable.
        self.playing.invalidate();
        let refresh_playing = matches!(&result.outcome, PublishOutcome::Published { .. });
        self.publish_pending = false;
        self.confirmation.clear();
        let notice = match result.outcome {
            PublishOutcome::Published {
                destination,
                transaction,
            } => {
                self.preview = None;
                let replaced = is_replacement(&transaction);
                let mut lines = vec![
                    "Every member of the new copy was re-checked against the catalogue checksums you reviewed.".into(),
                ];
                if replaced {
                    lines.push(format!(
                        "The original archive was not changed or deleted: it is preserved, exactly, at {}.",
                        preserved_original(&transaction)
                    ));
                } else {
                    lines.push("The source archives were not changed.".into());
                }
                lines.push(format!(
                    "Undo is available from transaction {} in the history below{}.",
                    transaction.transaction_id,
                    if replaced {
                        " and puts the original back"
                    } else {
                        ""
                    }
                ));
                lines.push(RECONSTRUCTION_RUNNABILITY_NOTE.into());
                Notice::new(
                    Tone::Success,
                    format!(
                        "{} {}",
                        if replaced { "Replaced" } else { "Published" },
                        destination.display()
                    ),
                    lines,
                )
            }
            PublishOutcome::AlreadyPublished { destination } => {
                self.preview = None;
                Notice::new(
                    Tone::Info,
                    "Nothing to publish",
                    vec![format!(
                        "{} already holds exactly this set; nothing was written.",
                        destination.display()
                    )],
                )
            }
            PublishOutcome::Refused(refusal) => {
                if result.refreshed_plan.is_some() {
                    // The reviewed plan is out of date. Nothing unreviewed is
                    // shown as publishable: the person previews again.
                    self.preview = None;
                }
                Notice::new(
                    Tone::Warning,
                    refusal.headline(),
                    vec![refusal.detail().to_string(), refusal.next_step().into()],
                )
            }
            PublishOutcome::Failed {
                reason,
                transaction,
            } => {
                self.preview = None;
                let mut lines = vec![
                    format!("{reason}."),
                    "It is not reported as published or replaced. EmuWiz never rewrites an archive in place; the transaction record says where each file is.".into(),
                ];
                if let Some(transaction) = transaction {
                    lines.push(format!(
                        "Transaction {} is kept in the MAME rebuild history below for review and recovery.",
                        transaction.transaction_id
                    ));
                } else {
                    lines.push("Check the history below and Recovery before trying again.".into());
                }
                Notice::new(Tone::Danger, "MAME publication did not complete", lines)
            }
        };
        self.notice = Some(notice);
        if refresh_playing {
            self.refresh_playing_library();
        }
    }

    fn refresh_playing_library(&mut self) {
        if let Some(inputs) = self.inputs().filter(|i| i.trusted) {
            self.playing.refresh(inputs, self.environment.clone());
        }
    }

    fn finish_undo(&mut self, outcome: UndoOutcome) {
        self.playing.invalidate();
        self.undo_pending = None;
        self.confirmation.clear();
        self.notice = Some(match outcome {
            UndoOutcome::RolledBack { transaction } => Notice::new(
                Tone::Success,
                "MAME reconstruction was rolled back safely",
                vec![format!(
                    "Transaction {}: {} The source archives were not touched.",
                    transaction.transaction_id,
                    if is_replacement(&transaction) {
                        format!(
                            "the original archive was restored; the rebuilt copy is kept at {}.",
                            preserved_original(&transaction)
                        )
                    } else {
                        "the generated copy was removed.".to_string()
                    }
                )],
            ),
            UndoOutcome::ClosedWithoutPublication { transaction } => Notice::new(
                Tone::Info,
                "Nothing to undo",
                vec![format!(
                    "MAME transaction {} had published nothing, so there was nothing to undo; it is now closed. The source archives were not touched.",
                    transaction.transaction_id
                )],
            ),
            UndoOutcome::Incomplete {
                message,
                transaction,
            } => {
                // Keep the row in step with the receipt the rollback just
                // journalled, even if the history reload could not read it.
                if let Some(row) = self
                    .history
                    .iter_mut()
                    .find(|row| row.transaction_id == transaction.transaction_id)
                {
                    *row = *transaction;
                }
                Notice::new(Tone::Danger, "MAME undo did not complete", vec![message])
            }
            UndoOutcome::Refused(message) => {
                Notice::new(Tone::Warning, "Undo was refused", vec![message])
            }
            UndoOutcome::Failed(message) => {
                Notice::new(Tone::Danger, "MAME undo stopped", vec![message])
            }
        });
    }

    fn finish_finalise(&mut self, outcome: FinaliseOutcome) {
        self.playing.invalidate();
        self.finalise_pending = None;
        self.finalise_ack = false;
        self.confirmation.clear();
        // The history was reloaded from the journal with this result.
        self.notice = Some(match outcome {
            FinaliseOutcome::Finalised {
                transaction_id,
                deleted,
            } => Notice::new(
                Tone::Success,
                "Replacement finalised",
                vec![
                    format!(
                        "Transaction {transaction_id}: the preserved original ({}) was deleted and the journal records the replacement as final.",
                        deleted.display()
                    ),
                    "Undo is no longer available for it. The published file and the source archives were not changed; the record is kept as history.".into(),
                ],
            ),
            FinaliseOutcome::Cancelled { transaction_id } => Notice::new(
                Tone::Info,
                "Finalising cancelled",
                vec![format!(
                    "Transaction {transaction_id}: the original is intact and Undo is available again."
                )],
            ),
            FinaliseOutcome::Refused(message) => {
                Notice::new(Tone::Warning, "Finalise was not completed", vec![message])
            }
        });
    }

    fn request_preview(&mut self) {
        match self.inputs() {
            Some(inputs) if !inputs.set.is_empty() => {
                self.start(MameRequest::Preview { inputs });
            }
            Some(_) => {
                self.notice = Some(Notice::new(
                    Tone::Warning,
                    "Enter a parent or clone set name first.",
                    Vec::new(),
                ))
            }
            None => {
                self.notice = Some(Notice::new(
                    Tone::Warning,
                    "Choose both the MAME folder and its DAT first.",
                    Vec::new(),
                ))
            }
        }
    }

    fn request_refresh(&mut self, only_family: bool) {
        match self.inputs() {
            Some(inputs) if only_family && inputs.set.is_empty() => {
                self.notice = Some(Notice::new(
                    Tone::Warning,
                    "Enter a MAME set name first.",
                    Vec::new(),
                ))
            }
            Some(inputs) => {
                self.start(MameRequest::RefreshEvidence {
                    inputs,
                    only_family,
                });
            }
            None => {
                self.notice = Some(Notice::new(
                    Tone::Warning,
                    "Choose both the MAME folder and its verified DAT first.",
                    Vec::new(),
                ))
            }
        }
    }

    /// Starts the publication of the current preview after the typed
    /// confirmation matched. Refuses everything else.
    pub(super) fn confirm_publish(&mut self) -> bool {
        if !self.can_publish()
            || self.required_phrase().as_deref() != Some(self.confirmation.as_str())
        {
            return false;
        }
        let Some(preview) = self.preview.clone() else {
            return false;
        };
        self.start(MameRequest::Publish {
            preview: Box::new(preview),
        })
    }

    /// Starts an Undo after the typed confirmation matched, only for a
    /// transaction whose recorded evidence supports one.
    pub(super) fn confirm_undo(&mut self, transaction_id: &str) -> bool {
        let Some(transaction) = self
            .history
            .iter()
            .find(|transaction| transaction.transaction_id == transaction_id)
        else {
            return false;
        };
        if self.is_busy()
            || !undo_actionable(transaction)
            || self.confirmation != undo_confirmation_phrase(mame_parent(transaction))
        {
            return false;
        }
        self.start(MameRequest::Undo {
            transaction_id: transaction_id.to_string(),
        })
    }

    /// Starts Finalise Replacement after the typed phrase and the exclusive
    /// access acknowledgement, only for a verified, un-finalised replacement
    /// (or to resume an interrupted one).
    pub(super) fn confirm_finalise(&mut self, transaction_id: &str) -> bool {
        let Some(transaction) = self
            .history
            .iter()
            .find(|transaction| transaction.transaction_id == transaction_id)
        else {
            return false;
        };
        if self.is_busy()
            || !(finalise_actionable(transaction) || finalise_interrupted(transaction))
            || !self.finalise_ack
            || self.confirmation != finalise_confirmation_phrase(mame_parent(transaction))
        {
            return false;
        }
        self.start(MameRequest::Finalise {
            transaction_id: transaction_id.to_string(),
            action: FinaliseAction::Finish,
            exclusive_ack: true,
        })
    }

    /// Cancels an interrupted finalisation (Undo returns). Needs no typed
    /// phrase: it only removes a record and deletes nothing.
    pub(super) fn cancel_interrupted_finalise(&mut self, transaction_id: &str) -> bool {
        let Some(transaction) = self
            .history
            .iter()
            .find(|transaction| transaction.transaction_id == transaction_id)
        else {
            return false;
        };
        if self.is_busy() || !finalise_interrupted(transaction) {
            return false;
        }
        self.start(MameRequest::Finalise {
            transaction_id: transaction_id.to_string(),
            action: FinaliseAction::CancelInterrupted,
            exclusive_ack: false,
        })
    }

    // -----------------------------------------------------------------------
    // Rendering.
    // -----------------------------------------------------------------------

    fn sync_history_root(&mut self) {
        if self.history_root != self.root {
            self.history_root = self.root.clone();
            self.history_requested = false;
            self.unjournaled_stages.clear();
            self.staging_problem = None;
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();
        self.sync_history_root();
        if !self.history_requested && !self.is_busy() {
            self.history_requested = true;
            self.start(MameRequest::LoadHistory {
                root: self.root.clone(),
            });
        }
        if self.is_busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        ui.label("Rebuild one merged parent/clone set from ROM files EmuWiz has verified. Nothing changes until you review the preview and confirm.");
        self.show_job(ui);
        self.show_notice(ui);
        ui.add_space(theme::SPACE_SM);
        self.show_selection(ui);
        // A folder picked during this frame must not inherit the previous
        // folder's orphan status even for one rendered frame.
        self.sync_history_root();
        if let Some(game) = &self.standalone
            && self.inputs().as_ref() == Some(&game.inputs)
        {
            ui.separator();
            show_dependencies(ui, &game.dependencies);
        }
        if let Some(preview) = self.preview.clone() {
            ui.separator();
            show_dependencies(ui, &preview.dependencies);
            ui.separator();
            self.show_preview(ui, &preview);
        }
        self.show_history(ui);
        ui.separator();
        let inputs = self.inputs();
        if let Some(machine) = self
            .playing
            .show(ui, inputs, &self.environment, self.job.is_some())
        {
            self.set = machine;
            self.preview = None;
            self.confirmation.clear();
        }
    }

    fn show_job(&self, ui: &mut egui::Ui) {
        let Some(job) = &self.job else {
            return;
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.strong(job.kind.working_label());
            });
            ui.label(&job.phase);
            if matches!(job.kind, MameJobKind::Publish | MameJobKind::Undo) {
                ui.label("This step cannot be cancelled once started. EmuWiz keeps a recovery record, and you can keep using the rest of the app.");
            } else {
                ui.label("This step only reads files and cannot be cancelled once started.");
            }
        });
    }

    fn show_notice(&self, ui: &mut egui::Ui) {
        let Some(notice) = &self.notice else {
            return;
        };
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.0_f32, notice.tone.colour(ui)))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(&notice.headline)
                        .strong()
                        .color(notice.tone.colour(ui)),
                );
                for line in &notice.lines {
                    ui.label(line);
                }
            });
    }

    fn show_selection(&mut self, ui: &mut egui::Ui) {
        let busy = self.is_busy();
        ui.heading("1. Choose the machine");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Choose MAME folder..."))
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose MAME folder")
                    .pick_folder()
            {
                self.root = Some(path);
            }
        });
        ui.label(format!(
            "MAME folder: {}",
            self.root
                .as_deref()
                .map_or("not selected".into(), |p| p.display().to_string())
        ));
        ui.label(format!(
            "DAT: {}",
            self.dat
                .as_deref()
                .map_or("not selected".into(), |p| p.display().to_string())
        ));
        ui.add_enabled_ui(!busy, |ui| {
            self.catalogue_picker.poll();
            if self.catalogue_picker.loading { ui.ctx().request_repaint_after(Duration::from_millis(100)); }
            if let Some(reference) = self.catalogue_picker.show(ui, DatCatalogueWorkflow::MameReconstruction, &mut self.catalogue) {
                self.dat = self.catalogue_picker.summaries().iter().find(|row| row.reference == reference).and_then(|row| row.technical_path.clone());
                self.catalogue_trusted = false;
                self.confirmation.clear();
            }
            ui.checkbox(&mut self.catalogue_trusted, "I trust this selected DAT as the reference for my collection; I will review its declared MAME version and digest before publishing.");
        });
        ui.label("Register/import and validate a versioned MAME arcade machine DAT in DAT Sources, then select it here. Local registration is not publisher authentication. No catalogue is downloaded, trusted or selected automatically.");
        if self.catalogue.is_none() || !self.catalogue_trusted {
            ui.colored_label(theme::warning(ui), "Publish is unavailable until you explicitly choose and trust a suitable versioned catalogue, then Preview.");
        }
        ui.horizontal(|ui| {
            let label = ui.label("Parent or clone set:");
            ui.add_enabled(!busy, egui::TextEdit::singleline(&mut self.set))
                .labelled_by(label.id);
        });
        let blocker = selection_blocker(
            self.root.is_some(),
            self.dat.is_some() && self.catalogue.is_some(),
        );
        let family_blocker = blocker.or(self.set.trim().is_empty().then_some(NO_SET_SELECTED));
        if let Some(reason) = blocker.or(family_blocker) {
            ui.colored_label(theme::warning(ui), reason);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !busy && family_blocker.is_none(),
                    egui::Button::new("Re-check this family's files"),
                )
                .on_hover_text(
                    "Reads ROM checksums into EmuWiz's evidence. ROM files are not changed.",
                )
                .on_disabled_hover_text(family_blocker.unwrap_or(BUSY))
                .clicked()
            {
                self.request_refresh(true);
            }
            if ui
                .add_enabled(
                    !busy && blocker.is_none(),
                    egui::Button::new("Re-check the whole folder"),
                )
                .on_hover_text(
                    "Reads every set's checksums. This can take a long time on a large folder.",
                )
                .on_disabled_hover_text(blocker.unwrap_or(BUSY))
                .clicked()
            {
                self.request_refresh(false);
            }
            if ui
                .add_enabled(
                    !busy && family_blocker.is_none(),
                    egui::Button::new("Preview reconstruction"),
                )
                .on_hover_text("Read-only: shows dependencies and the files EmuWiz would use.")
                .on_disabled_hover_text(family_blocker.unwrap_or(BUSY))
                .clicked()
            {
                self.request_preview();
            }
        });
        if self.preview.is_some() && !self.preview_is_current() {
            ui.colored_label(theme::warning(ui),
                "The folder, DAT or set changed after this preview. Preview again before publishing.",
            );
        }
    }

    fn show_preview(&mut self, ui: &mut egui::Ui, preview: &MamePreview) {
        if let Some(binding) = &preview.catalogue {
            ui.label(format!(
                "Selected catalogue: {} · source {} · {:?}",
                binding.display_name, binding.source_id, binding.reference
            ));
            ui.label(format!(
                "Provenance: {} · explicitly trusted by you for this reconstruction",
                binding.provenance
            ));
        }
        let plan = &preview.plan;
        ui.heading("3. Review the proposed changes");
        ui.label("Rebuilding makes a new, complete copy of a set from files EmuWiz has verified. It does not repair your existing set in place.");
        ui.label(format!(
            "Target parent set: {} · clone sets: {}",
            plan.parent,
            if plan.clones.is_empty() {
                "none".into()
            } else {
                plan.clones.join(", ")
            }
        ));
        ui.label(format!(
            "New output destination: {}",
            plan.destination.display()
        ));
        ui.label(format!(
            "Catalogue: MAME {} (SHA-256 {})",
            plan.dat_version, plan.dat_sha256
        ));
        ui.label(format!(
            "{} required file(s); {} verified source member(s).",
            plan.required_members.len(),
            plan.sources.len()
        ));
        if !plan.sources.is_empty() {
            ui.strong("Files EmuWiz will use");
            for source in plan.sources.iter().take(8) {
                ui.push_id((&source.archive_path, &source.member_path), |ui| {
                    ui.label(format!("{} -> {}", source.current_name, source.target_name));
                });
            }
            if plan.sources.len() > 8 {
                ui.label(format!(
                    "and {} more source member(s)",
                    plan.sources.len() - 8
                ));
            }
        }
        for (label, values) in [
            ("Missing files", &plan.missing_members),
            ("More than one possible source", &plan.duplicate_candidates),
            ("Wrong checksum evidence", &plan.hash_mismatches),
            (
                "Ownership the catalogue does not prove",
                &plan.unresolved_ownership,
            ),
            ("Output collisions", &plan.collisions),
        ] {
            if !values.is_empty() {
                ui.colored_label(
                    theme::warning(ui),
                    format!("{label}: {}", values.join(", ")),
                );
            }
        }
        match &preview.destination {
            DestinationStatus::Absent => {}
            DestinationStatus::AlreadyPublished => {
                ui.colored_label(theme::info(ui),
                    "The destination already holds exactly this set (checksum-verified). There is nothing to publish.",
                );
            }
            DestinationStatus::Replaceable { changes, .. } => {
                show_replacement_review(ui, &plan.destination, changes);
            }
            DestinationStatus::ReplacementRefused { changes, reasons } => {
                ui.colored_label(
                    theme::warning(ui),
                    format!(
                        "{} already exists and will not be replaced: not every member in it is accounted for.",
                        plan.destination.display()
                    ),
                );
                for reason in reasons {
                    ui.label(format!("• {reason}"));
                }
                show_member_accounting(ui, changes);
                ui.label("Nothing was changed. Fix or move the archive yourself, or rebuild into a folder without it.");
            }
            DestinationStatus::Occupied(reason) => {
                ui.colored_label(
                    theme::warning(ui),
                    format!(
                        "An existing file is at the destination and cannot be replaced: {reason}"
                    ),
                );
            }
        }
        ui.collapsing("Advanced reconstruction evidence", |ui| {
            for reason in &plan.reasons {
                ui.label(format!("• {reason}"));
            }
            for (index, source) in plan.sources.iter().enumerate() {
                let kind = if preview.source_is_extracted.get(index) == Some(&true) {
                    "extracted file"
                } else {
                    "ZIP member"
                };
                ui.label(format!(
                    "{} <- {kind} {}::{}",
                    source.target_name,
                    source.archive_path.display(),
                    source.current_name
                ));
                if let Some(requirement) = plan
                    .required_members
                    .iter()
                    .find(|requirement| requirement.member_name == source.target_name)
                {
                    ui.small(format!(
                        "owner {} · checksum evidence: SHA-1 {} · CRC32 {}",
                        requirement.owner_set,
                        requirement.sha1.as_deref().unwrap_or("not recorded"),
                        requirement.crc32.as_deref().unwrap_or("not recorded")
                    ));
                }
            }
        });

        ui.separator();
        ui.heading("4. Confirm and publish");
        match preview.gate() {
            PublishGate::Blocked(reasons) => {
                ui.colored_label(theme::warning(ui), "Publish is blocked:");
                for reason in reasons {
                    ui.label(format!("• {}", reason.describe()));
                }
                return;
            }
            PublishGate::Ready
                if matches!(
                    preview.destination,
                    DestinationStatus::AlreadyPublished
                        | DestinationStatus::Occupied(_)
                        | DestinationStatus::ReplacementRefused { .. }
                ) =>
            {
                return;
            }
            PublishGate::Ready => {}
        }
        let replacing = preview.is_replacement();
        let destination = preview.plan.destination.display().to_string();
        if replacing {
            ui.label(format!(
                "This replaces {destination} with the rebuilt merged set. The new archive is built and verified first; the original is then swapped out in one step and kept unchanged for Undo."
            ));
        } else {
            ui.label("This creates a separate new copy. Your source archives stay untouched, and you can undo it afterwards.");
        }
        ui.label("After it finishes, EmuWiz re-checks the new copy against what you reviewed.");
        if !self.publish_pending {
            let button = if replacing {
                format!("Review replacement of {destination}")
            } else {
                "Publish reconstructed merged output".to_string()
            };
            if ui
                .add_enabled(self.can_publish(), egui::Button::new(button))
                .clicked()
            {
                self.publish_pending = true;
                self.confirmation.clear();
            }
            return;
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let phrase = self.required_phrase().unwrap_or_default();
            let label = if replacing {
                ui.strong(format!("Confirm: replace {destination}"));
                ui.label(format!(
                    "Type {phrase} exactly to replace this archive. EmuWiz refuses if it changed since this preview."
                ))
            } else {
                ui.strong("Confirm: create the new copy");
                ui.label(format!(
                    "Type {phrase} exactly to create the destination ZIP."
                ))
            };
            ui.add_enabled(
                !self.is_busy(),
                egui::TextEdit::singleline(&mut self.confirmation),
            )
            .labelled_by(label.id);
            ui.horizontal_wrapped(|ui| {
                let confirmed = self.confirmation == phrase && self.can_publish();
                if ui
                    .add_enabled(
                        confirmed,
                        egui::Button::new(if replacing { "Replace now" } else { "Publish now" }),
                    )
                    .clicked()
                {
                    self.confirm_publish();
                }
                if ui
                    .add_enabled(!self.is_busy(), egui::Button::new("Cancel"))
                    .clicked()
                {
                    self.publish_pending = false;
                    self.confirmation.clear();
                }
            });
        });
    }

    fn show_history(&mut self, ui: &mut egui::Ui) {
        if self.history.is_empty()
            && self.history_problems.is_empty()
            && self.unjournaled_stages.is_empty()
            && self.staging_problem.is_none()
        {
            return;
        }
        ui.separator();
        ui.heading("5. Results and Undo");
        if let Some(error) = &self.staging_problem {
            ui.colored_label(theme::warning(ui), format!("Staging inventory could not be read: {error}. Recovery status is unknown; inspect the staging folder before publishing."));
        }
        for item in &self.unjournaled_stages {
            ui.colored_label(
                theme::warning(ui),
                format!("Retained staged file: {}", item.path.display()),
            );
            ui.label(&item.retention_reason);
        }
        for problem in &self.history_problems {
            ui.colored_label(
                theme::warning(ui),
                format!("A recovery journal could not be read: {problem}"),
            );
        }
        let busy = self.is_busy();
        let history = self.history.clone();
        for transaction in history.iter().rev() {
            ui.push_id(&transaction.transaction_id, |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(format!(
                        "{} · {} · {} output",
                        transaction.transaction_id,
                        transaction.state.label(),
                        transaction.entries.len()
                    ));
                    if let Some(entry) = transaction.entries.first() {
                        ui.label(format!("Destination: {}", entry.destination_path.display()));
                    }
                    ui.label(format!("Recorded: {}", transaction.created_at_unix));
                    ui.label(history_status(transaction));
                    if undo_actionable(transaction) {
                        self.show_undo_controls(ui, transaction, busy);
                    }
                    if finalise_actionable(transaction) || finalise_interrupted(transaction) {
                        self.show_finalise_controls(ui, transaction, busy);
                    }
                    ui.collapsing("Advanced transaction details", |ui| {
                        ui.label(format!("State: {}", transaction.state.label()));
                        ui.label(format!("Entries: {}", entry_states(transaction)));
                        ui.label("EmuWiz keeps this record so the publication can be undone or recovered. Your source sets are never changed by an undo.");
                    });
                });
            });
        }
    }

    fn show_finalise_controls(
        &mut self,
        ui: &mut egui::Ui,
        transaction: &RenameTransaction,
        busy: bool,
    ) {
        let id = transaction.transaction_id.as_str();
        let interrupted = finalise_interrupted(transaction);
        if self.finalise_pending.as_deref() != Some(id) {
            let label = if interrupted {
                "Finish finalising this replacement"
            } else {
                "Finalise replacement (delete the original, give up Undo)"
            };
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                    self.finalise_pending = Some(id.to_string());
                    self.finalise_ack = false;
                    self.confirmation.clear();
                }
                if interrupted
                    && ui
                        .add_enabled(!busy, egui::Button::new("Cancel finalising (restore Undo)"))
                        .clicked()
                {
                    self.cancel_interrupted_finalise(id);
                }
            });
            return;
        }
        let phrase = finalise_confirmation_phrase(mame_parent(transaction));
        ui.colored_label(
            theme::warning(ui),
            "This permanently deletes the preserved original archive. Undo will no longer be able to put it back. The published file and your source sets are not changed.",
        );
        ui.add_enabled(
            !busy,
            egui::Checkbox::new(
                &mut self.finalise_ack,
                "Nothing else is using the MAME staging folder or recovery journals",
            ),
        );
        let prompt = ui.label(format!("Type {phrase} exactly to confirm."));
        ui.add_enabled(!busy, egui::TextEdit::singleline(&mut self.confirmation))
            .labelled_by(prompt.id);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !busy && self.finalise_ack && self.confirmation == phrase,
                    egui::Button::new("Finalise now"),
                )
                .clicked()
            {
                self.confirm_finalise(id);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Do not finalise"))
                .clicked()
            {
                self.finalise_pending = None;
                self.finalise_ack = false;
                self.confirmation.clear();
            }
        });
    }

    fn show_undo_controls(
        &mut self,
        ui: &mut egui::Ui,
        transaction: &RenameTransaction,
        busy: bool,
    ) {
        let id = transaction.transaction_id.as_str();
        // "Published" is only said of a verified publication.
        let label = if publication_succeeded(transaction) && is_replacement(transaction) {
            "Undo replacement (put the original archive back)"
        } else if publication_succeeded(transaction) {
            "Undo published MAME output"
        } else if transaction
            .entries
            .iter()
            .any(|entry| entry.state == EntryState::Applied)
        {
            "Undo this transaction's remaining applied outputs"
        } else {
            "Retry undo of the outputs awaiting review"
        };
        if self.undo_pending.as_deref() != Some(id) {
            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                self.undo_pending = Some(id.to_string());
                self.confirmation.clear();
            }
            return;
        }
        let phrase = undo_confirmation_phrase(mame_parent(transaction));
        let prompt = ui.label(format!("Type {phrase} exactly to confirm undo."));
        ui.add_enabled(!busy, egui::TextEdit::singleline(&mut self.confirmation))
            .labelled_by(prompt.id);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !busy && self.confirmation == phrase,
                    egui::Button::new("Undo now"),
                )
                .clicked()
            {
                self.confirm_undo(id);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Cancel undo"))
                .clicked()
            {
                self.undo_pending = None;
                self.confirmation.clear();
            }
        });
    }
}

fn show_replacement_review(
    ui: &mut egui::Ui,
    destination: &Path,
    changes: &ReplacementMemberChanges,
) {
    ui.strong(format!(
        "Replace the existing parent ZIP: {}",
        destination.display()
    ));
    ui.label("EmuWiz checked every member of this archive against the catalogue. Each one is accounted for, but it is not the complete merged set. You can review replacing it, or leave it as it is.");
    show_member_accounting(ui, changes);
    ui.label("The original archive is never edited or deleted. It is kept byte-for-byte in the hidden .emuwiz-mame-staging folder inside your MAME folder, recorded in the recovery journal, and Undo puts the exact original back. Do not move that folder's files by hand.");
}

fn show_member_accounting(ui: &mut egui::Ui, changes: &ReplacementMemberChanges) {
    let provided = changes
        .provided_elsewhere
        .iter()
        .map(|(member, provider)| format!("{member} (verified copy in {provider}.zip)"))
        .collect::<Vec<_>>();
    let unverified = changes
        .support_unverified
        .iter()
        .map(|(member, provider)| format!("{member} (no verified copy in {provider}.zip)"))
        .collect::<Vec<_>>();
    for (label, members) in [
        ("Kept (already correct)", &changes.kept),
        ("Wrong content under a required name", &changes.changed),
        ("Added", &changes.added),
        (
            "Support-set ROMs left out of the merged archive (kept in the preserved original)",
            &provided,
        ),
        (
            "Support-set ROMs without a verified copy elsewhere",
            &unverified,
        ),
        ("Not identified by the catalogue", &changes.unidentified),
    ] {
        if !members.is_empty() {
            ui.label(format!("{label}: {}", members.join(", ")));
        }
    }
}

fn show_dependencies(ui: &mut egui::Ui, dependencies: &ReconstructionDependencies) {
    ui.heading("2. Dependencies");
    ui.label(format!(
        "Parent: {} · clones: {}",
        dependencies.parent,
        if dependencies.clones.is_empty() {
            "none".into()
        } else {
            dependencies.clones.join(", ")
        }
    ));
    ui.label("The merged output holds the parent's and clones' own ROMs only.");
    let separate = dependencies.stored_separately().collect::<Vec<_>>();
    if separate.is_empty() {
        ui.label("No BIOS, device or other support set with ROMs is declared for this family.");
    } else {
        ui.strong(
            "Also needed, stored in their own sets (not part of this rebuild, not checked here)",
        );
        for set in separate {
            let catalogue = match set.catalogue {
                CatalogueReference::Unique => String::new(),
                CatalogueReference::Absent => " · missing from the catalogue".into(),
                CatalogueReference::Duplicate => {
                    " · declared more than once in the catalogue".into()
                }
            };
            ui.label(format!(
                "• {} ({}) · {} ROM(s) · needed by {}{catalogue}",
                set.name,
                set.kind.label(),
                set.declared_roms,
                set.needed_by.join(", ")
            ));
        }
    }
    for reason in dependencies.unresolved_relationships.iter().take(30) {
        ui.colored_label(theme::warning(ui), reason);
    }
    if !dependencies.unresolved_members.is_empty() {
        ui.colored_label(
            theme::warning(ui),
            format!(
                "{} member(s) have ownership the catalogue does not prove:",
                dependencies.unresolved_members.len()
            ),
        );
        for member in &dependencies.unresolved_members {
            ui.label(format!(
                "• {}:{} - {}",
                member.declaring_set, member.member_name, member.reason
            ));
        }
    }
    if !dependencies.device_members.is_empty() {
        // Technical provenance stays collapsed; the plain reasons are above.
        ui.collapsing("Device ROM provenance", |ui| {
            for member in &dependencies.device_members {
                let supply = match &member.supply {
                    DeviceMemberSupply::DeviceOwn => "stored in the device's own set".to_string(),
                    DeviceMemberSupply::BorrowedFrom(owner) => format!("supplied by {owner}"),
                };
                ui.label(format!(
                    "• {}:{} - {supply} · needed by {}",
                    member.device,
                    member.member_name,
                    member.needed_by.join(", ")
                ));
            }
        });
    }
    if !dependencies.disks.is_empty() {
        ui.colored_label(theme::warning(ui),
            format!(
                "{} disk image(s) (CHD) are declared. Disks are not copied, checked or included by this rebuild.",
                dependencies.disks.len()
            ),
        );
    }
    if !dependencies.samples.is_empty() {
        ui.label(format!(
            "Sample sets ({}) are separate and not checked here.",
            dependencies.samples.join(", ")
        ));
    }
    let mut provided = 0;
    let mut no_dump = 0;
    let mut other = 0;
    for member in &dependencies.excluded_members {
        match member.reason {
            ExcludedMemberReason::ProvidedBy(_) => provided += 1,
            ExcludedMemberReason::NoDump => no_dump += 1,
            ExcludedMemberReason::Optional | ExcludedMemberReason::LoadInstruction => other += 1,
        }
    }
    if provided + no_dump + other > 0 {
        ui.label(format!(
            "Not in the output: {provided} member(s) provided by support sets, {no_dump} with no known dump (a preservation gap, not repairable), {other} optional or load instructions."
        ));
    }
    ui.colored_label(theme::warning(ui), RECONSTRUCTION_RUNNABILITY_NOTE);
}

#[cfg(test)]
mod tests;
