//! GUI Batch C: read-only "Plan Preview" - what EmuWiz currently plans to
//! do with the selected file, if organisation were approved.
//!
//! # A preview of the existing planner, not a second one
//!
//! [`crate::dat::rom_organisation`] and
//! [`archivefs_core::platform_evidence_fusion::library_planning`] are the
//! project's real, reviewed, read-only destination planner - see that
//! module's own doc comment for why it exists and why nothing here may
//! duplicate it. This module calls
//! [`archivefs_core::platform_evidence_fusion::library_planning::plan_library`]
//! with exactly one input (the selected file's already-computed identity)
//! and renders whatever it returns. It computes no destination path, no
//! canonical name, and no conflict/collision decision of its own.
//!
//! # Preview only
//!
//! [`archivefs_core::platform_evidence_fusion::library_planning::RenameSuggestion::authorized`]
//! is always `false` for a plan built here (the planner itself never sets
//! it any other way in this milestone) - there is no code path in this
//! module that can move, rename, or write a file. It exists only to answer
//! "what would happen", never to make it happen; see `plan_preview_page`'s
//! own action vocabulary, which offers nothing but `Load`.
//!
//! # Reuses the same master ROM root Canonical Organisation already uses
//!
//! `Config::master_rom_root` (unchanged) is the one setting that decides
//! where anything would go; this page reads it, never invents a
//! destination root of its own, and honestly reports "not configured yet"
//! when it is absent rather than guessing one.
//!
//! # Real, read-only file I/O
//!
//! `plan_library` calls `build_organisation_plan`, which checks for a
//! destination collision with `symlink_metadata`/`read_dir` on the
//! destination's parent directory (see that module's own "Read-only"
//! comment) - real but non-mutating I/O, and enough that this must not run
//! on every frame. Gathering here follows the same explicit-load,
//! off-UI-thread, generation-guarded shape `selected_evidence_page` and
//! `identity_sources_page` already use.
//!
//! # GUI Batch D: Apply readiness, from the same real chain
//!
//! [`ApplyReadinessInfo`] extends the same already-computed
//! [`archivefs_core::platform_evidence_fusion::library_planning::LibraryItemPlan`]
//! one step further down the *existing* frozen-plan/preview boundary:
//! [`archivefs_core::platform_evidence_fusion::library_plan_presentation::present_library_plan`]
//! -> [`archivefs_core::platform_evidence_fusion::library_plan_export::export_item`]
//! -> [`archivefs_core::platform_evidence_fusion::plan_transaction::build_preview`],
//! every one of them documented as pure/read-only in its own module, called
//! here unchanged. This module still creates no
//! [`archivefs_core::dat::rename_apply::model::RenameTransaction`], writes
//! no journal, and calls neither
//! [`archivefs_core::platform_evidence_fusion::plan_transaction::approve_transaction`]
//! nor
//! [`archivefs_core::platform_evidence_fusion::plan_transaction::assess_canary_eligibility`].
//! The former would require a real user acknowledgement this batch does
//! not collect, and the latter requires an
//! [`archivefs_core::platform_evidence_fusion::plan_transaction::ApprovedPlan`]
//! that only the former can produce. Canary eligibility is therefore
//! surfaced as a concept (what it would require, using the real
//! `CANARY_MAX_SIZE_BYTES` ceiling), not as a computed verdict, until a
//! future batch adds a real approval action.

use std::path::Path;

use eframe::egui;

use archivefs_core::dat::rom_organisation::OrganisationMode;
use archivefs_core::platform_evidence_fusion::identity_orchestrator::IdentityResult;
use archivefs_core::platform_evidence_fusion::identity_presentation::{
    IdentityPresentation, IdentityStatus,
};
use archivefs_core::platform_evidence_fusion::library_plan_export::{
    LibraryPlanExport, export_item,
};
use archivefs_core::platform_evidence_fusion::library_plan_presentation::present_library_plan;
use archivefs_core::platform_evidence_fusion::library_planning::{
    LibraryItemPlan, LibraryPlanInput, LibraryPlanningContext, PlanStatus, RenameBasis,
    RommMappingStatus, no_slug_mapping, plan_library,
};
use archivefs_core::platform_evidence_fusion::plan_transaction::{
    CANARY_MAX_SIZE_BYTES, OperationKind, PreconditionStrength, build_preview,
};

use crate::selected_evidence_page::status_tone_for;
use crate::ui::components as widgets;

// ---------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------

/// A read-only preview of the full chain for one file: the resolver result
/// it was already given (unchanged - see [`gather_plan_preview`]'s own doc
/// comment), and the planner's result built from it. Two separate verdicts,
/// shown as two separate steps, never collapsed into one - an unresolved
/// identity and a planner-side conflict are different facts even when both
/// end up blocking the same file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanPreviewInfo {
    /// The resolver's own verdict - [`IdentityPresentation::status`],
    /// unchanged, not re-derived from the planner's result.
    pub(crate) resolver_status: IdentityStatus,
    /// The resolver's own plain-language summary -
    /// [`IdentityPresentation::content_summary`], unchanged, the exact
    /// text the evidence panel above already shows.
    pub(crate) resolver_summary: String,
    /// The planner's own verdict, built from the resolver result above via
    /// [`archivefs_core::platform_evidence_fusion::library_planning::plan_status`]
    /// (inside `plan_library`, unchanged) - never recomputed here.
    pub(crate) status: PlanStatus,
    /// `Some` only when the planner actually produced a destination for
    /// this status (Ready, or a Conflict naming where it would have
    /// landed) - the planner itself leaves this empty for
    /// Blocked/Unsupported/never-computed cases, and this preview never
    /// fabricates one to fill the gap.
    pub(crate) destination_display: Option<String>,
    pub(crate) platform_display_name: Option<String>,
    pub(crate) platform_source: Option<String>,
    /// Why planning is blocked or conflicted, verbatim from the planner -
    /// empty when the status needs no explanation.
    pub(crate) blockers: Vec<String>,
    pub(crate) rename_basis: RenameBasis,
    pub(crate) proposed_name: Option<String>,
    pub(crate) romm_status: RommMappingStatus,
    pub(crate) romm_slug: Option<String>,
    /// GUI Batch D: whether this plan is eligible for a future Apply, and
    /// what would be required first - see [`ApplyReadinessInfo`].
    pub(crate) readiness: ApplyReadinessInfo,
}

/// GUI Batch D: one more real, read-only step past the planner - what the
/// existing frozen-plan/preview boundary
/// ([`archivefs_core::platform_evidence_fusion::plan_transaction`]) says
/// about this single item, built from the exact same [`LibraryItemPlan`]
/// [`gather_plan_preview`] already has. Fail-closed by construction: an
/// operation only exists in `archivefs_core::platform_evidence_fusion::plan_transaction::TransactionPreview::operations`
/// when the planner status is `Ready`, a destination was computed, *and*
/// there are no blockers - see `plan_transaction::build_preview`'s own doc
/// comment. `outcome` is derived from that fact, never a separate guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplyReadinessInfo {
    pub(crate) outcome: ApplyReadinessOutcome,
    /// `Some` only when the preview actually produced an operation for
    /// this item (mirrors `readiness.outcome == ApplyReadinessOutcome::ReadyForReview`).
    pub(crate) operation_kind: Option<OperationKind>,
    pub(crate) precondition_strength: Option<PreconditionStrength>,
    /// Whether *anything* here would need your explicit approval before it
    /// could ever apply - true exactly when the preview produced one or
    /// more operations. There is no path in this codebase (reused,
    /// unchanged) that applies without it; see `plan_transaction::render_preview_text`'s
    /// own always-present "Approval: REQUIRED" line.
    pub(crate) approval_required: bool,
    /// The frozen-plan digest `plan_transaction::compute_plan_digest` would
    /// assign this single-item preview - shown only as an identifier
    /// (Advanced mode), never used here to construct an approval.
    pub(crate) plan_digest: String,
    /// Every blocker the real chain reported, verbatim - the same set
    /// [`PlanPreviewInfo::blockers`] carries, kept alongside the readiness
    /// verdict so this section is legible without cross-referencing the
    /// planner card above it.
    pub(crate) blockers: Vec<String>,
}

/// One simple, fail-closed Apply-readiness outcome - see this module's own
/// doc comment for why `Unknown`/`Ambiguous`/`Conflict`/`Unsupported`/a
/// blocked organisation status can never land on `ReadyForReview`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyReadinessOutcome {
    /// A real operation exists: `Ready`, a destination, zero blockers.
    /// Still not applied - approval (not offered by this batch) is always
    /// required first.
    ReadyForReview,
    /// The resolver itself has not settled - `Conflict` or `Ambiguous`.
    /// Planning-layer blockers are a different problem and never reported
    /// under this outcome.
    ConflictMustResolve,
    /// Anything else: `Unknown`, `Unsupported`, a blocked organisation
    /// status, or (defensively) a `Ready` status the preview still could
    /// not turn into an operation.
    CannotSafelyApply,
}

/// The whole read-only outcome of trying to preview a plan for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanPreviewOutcome {
    /// `Config::master_rom_root` is not set - the one precondition this
    /// preview cannot proceed without, honestly reported rather than
    /// guessed. Distinct from [`PlanStatus::Unsupported`], which is the
    /// planner's own verdict once it does run.
    NoMasterRoot,
    Planned(Box<PlanPreviewInfo>),
}

fn plan_status_label(status: PlanStatus) -> &'static str {
    status.label()
}

fn plan_status_tone(status: PlanStatus) -> widgets::StatusTone {
    match status {
        PlanStatus::Ready => widgets::StatusTone::Success,
        PlanStatus::NeedsReview => widgets::StatusTone::Warning,
        PlanStatus::Ambiguous => widgets::StatusTone::Pending,
        PlanStatus::Conflict => widgets::StatusTone::Blocked,
        PlanStatus::Unknown => widgets::StatusTone::Pending,
        PlanStatus::Unsupported => widgets::StatusTone::Blocked,
    }
}

/// Builds the preview for one file: the resolver result it was already
/// given - `identity`/`identity_presentation`, both produced once by
/// [`crate::selected_evidence_page::gather_selected_evidence`] and passed
/// through unchanged here, never recomputed - fed into the real, unchanged
/// library planner, with exactly one candidate.
///
/// Performs real (read-only) file I/O (see this module's own doc comment)
/// and must not be called on every frame; callers gate it behind an
/// explicit action, the same convention every other gather in this crate
/// uses.
pub(crate) fn gather_plan_preview(
    source_path: &Path,
    identity: &IdentityResult,
    identity_presentation: &IdentityPresentation,
    physical_hash: Option<&str>,
    master_root: Option<&Path>,
) -> PlanPreviewOutcome {
    let Some(master_root) = master_root else {
        return PlanPreviewOutcome::NoMasterRoot;
    };

    let inputs = [LibraryPlanInput {
        source_path: source_path.to_path_buf(),
        identity: identity.clone(),
        // Neither is computed by this preview: `selected_evidence_page`
        // does not currently produce archive-member set identity for a
        // bare selected file, and duplicate-taxonomy hashes are a
        // whole-library concern this single-item preview has no database
        // to check against. Both are optional inputs the planner already
        // treats as "skip this axis" rather than "fabricate a value" -
        // see `LibraryPlanInput`'s own doc comment.
        set_identity: None,
        physical_hash: None,
        normalized_hash: None,
        release_relationship: None,
    }];
    let context = LibraryPlanningContext {
        destination_root: master_root,
        // The same default `rom_organisation_page` itself starts from;
        // this preview does not add a mode picker of its own (section
        // "do not invent destination logic in the GUI").
        mode: OrganisationMode::MoveRealFile,
        slug_for_platform: &no_slug_mapping,
        generation: 1,
    };

    let report = plan_library(&inputs, &context);
    let item = report
        .items
        .into_iter()
        .next()
        .expect("one input always produces exactly one item");

    let destination_display = if item.organisation.destination_path.as_os_str().is_empty() {
        None
    } else {
        Some(item.organisation.destination_path.display().to_string())
    };

    let readiness = gather_apply_readiness(&item, identity, physical_hash);

    PlanPreviewOutcome::Planned(Box::new(PlanPreviewInfo {
        resolver_status: identity_presentation.status,
        resolver_summary: identity_presentation.content_summary.clone(),
        status: item.status,
        destination_display,
        platform_display_name: item
            .organisation
            .platform
            .as_ref()
            .map(|_| item.organisation.platform_display_name.clone()),
        platform_source: item
            .organisation
            .platform
            .as_ref()
            .map(|_| item.organisation.platform_source.clone()),
        blockers: item
            .organisation
            .reason
            .clone()
            .into_iter()
            .chain(item.rename.blockers.clone())
            .collect(),
        rename_basis: item.rename.basis,
        proposed_name: item.rename.proposed_name.clone(),
        romm_status: item.romm.status,
        romm_slug: item.romm.slug.clone(),
        readiness,
    }))
}

/// GUI Batch D: builds [`ApplyReadinessInfo`] from `item` via the real,
/// unchanged frozen-plan/preview boundary - see this module's own doc
/// comment for the exact call chain and why it stops at `build_preview`
/// (never `approve_transaction`/`assess_canary_eligibility`).
fn gather_apply_readiness(
    item: &LibraryItemPlan,
    identity: &IdentityResult,
    physical_hash: Option<&str>,
) -> ApplyReadinessInfo {
    let presentation = present_library_plan(item, identity);
    let export_item_value = export_item(item, &presentation, physical_hash, None);
    let export = LibraryPlanExport {
        items: vec![export_item_value],
    };
    let preview = build_preview(&export);
    let operation = preview.operations.first();

    let outcome = match item.status {
        PlanStatus::Conflict | PlanStatus::Ambiguous => ApplyReadinessOutcome::ConflictMustResolve,
        PlanStatus::Ready if operation.is_some() => ApplyReadinessOutcome::ReadyForReview,
        _ => ApplyReadinessOutcome::CannotSafelyApply,
    };

    ApplyReadinessInfo {
        outcome,
        operation_kind: operation.map(|op| op.kind),
        precondition_strength: operation.map(|op| op.precondition_strength),
        approval_required: preview.total_operation_count > 0,
        plan_digest: preview.digest.as_str().to_string(),
        blockers: presentation.blockers,
    }
}

// ---------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------

/// Mirrors `identity_sources_page::IdentitySourcesState`'s own shape:
/// explicit load, off-UI-thread, generation-guarded.
#[derive(Default)]
pub(crate) enum PlanPreviewState {
    #[default]
    Idle,
    Loading {
        generation: u64,
        receiver: std::sync::mpsc::Receiver<(u64, PlanPreviewOutcome)>,
    },
    Ready {
        #[allow(dead_code)]
        generation: u64,
        outcome: PlanPreviewOutcome,
    },
}

pub(crate) enum PlanPreviewAction {
    Load,
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

fn rename_basis_label(basis: RenameBasis) -> &'static str {
    match basis {
        RenameBasis::AuthoritativeDatRelease => "Confirmed by a verified DAT match",
        RenameBasis::OriginalNamePreserved => "Original name kept (no confirmed release name)",
        RenameBasis::Unavailable => "No safe suggestion",
    }
}

fn romm_status_label(status: RommMappingStatus) -> &'static str {
    match status {
        RommMappingStatus::Mapped => "Mapped",
        RommMappingStatus::Unmapped => "Not mapped",
        RommMappingStatus::Ambiguous => "Ambiguous",
        RommMappingStatus::Unsupported => "Not supported",
    }
}

fn readiness_headline(outcome: ApplyReadinessOutcome) -> &'static str {
    match outcome {
        ApplyReadinessOutcome::ReadyForReview => "Ready for review",
        ApplyReadinessOutcome::ConflictMustResolve => "Conflict must be resolved first",
        ApplyReadinessOutcome::CannotSafelyApply => "Cannot safely apply",
    }
}

fn readiness_tone(outcome: ApplyReadinessOutcome) -> widgets::StatusTone {
    match outcome {
        ApplyReadinessOutcome::ReadyForReview => widgets::StatusTone::Success,
        ApplyReadinessOutcome::ConflictMustResolve => widgets::StatusTone::Blocked,
        ApplyReadinessOutcome::CannotSafelyApply => widgets::StatusTone::Blocked,
    }
}

fn readiness_explanation(info: &ApplyReadinessInfo) -> String {
    match info.outcome {
        ApplyReadinessOutcome::ReadyForReview => {
            "This is ready to apply, but nothing happens until a future version lets you \
             review and approve it. No file has been moved, renamed, or written."
                .to_string()
        }
        ApplyReadinessOutcome::ConflictMustResolve => {
            "Evidence disagrees about what this file is, so it cannot be approved for Apply \
             until that conflict is resolved."
                .to_string()
        }
        ApplyReadinessOutcome::CannotSafelyApply => {
            if info.blockers.is_empty() {
                "This file is not eligible for Apply yet.".to_string()
            } else {
                format!(
                    "This file is not eligible for Apply yet: {}",
                    info.blockers.join("; ")
                )
            }
        }
    }
}

fn operation_kind_label(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Move => "Move",
        OperationKind::Rename => "Rename",
        OperationKind::Unsupported => "Unsupported",
    }
}

fn precondition_strength_label(strength: PreconditionStrength) -> &'static str {
    match strength {
        PreconditionStrength::HashVerified => "Hash-verified (re-checked before Apply)",
        PreconditionStrength::IdentityOnly => "Identity only (no frozen hash to re-verify)",
    }
}

/// Draws the "Plan Preview" section. Returns an action the caller should
/// perform (loading/refreshing the preview) - drawing itself never
/// mutates anything and offers no Apply of any kind.
pub(crate) fn show_plan_preview_panel(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    selected_path: Option<&Path>,
    state: &PlanPreviewState,
) -> Option<PlanPreviewAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Plan Preview",
        Some(
            "What EmuWiz would do with this file if organisation were approved. Preview only - nothing is moved, renamed, or written.",
        ),
    );

    if selected_path.is_none() {
        ui.label("No archive is selected in the Library.");
        return None;
    }

    match state {
        PlanPreviewState::Idle => {
            widgets::card(ui, |ui| {
                ui.label("The destination plan has not been previewed yet.");
                if widgets::action_button(ui, "Preview plan", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    action = Some(PlanPreviewAction::Load);
                }
            });
        }
        PlanPreviewState::Loading { .. } => {
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the proposed destination…");
                });
            });
        }
        PlanPreviewState::Ready { outcome, .. } => {
            show_outcome(ui, advanced_mode, outcome);
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(PlanPreviewAction::Load);
            }
        }
    }

    action
}

fn show_outcome(ui: &mut egui::Ui, advanced_mode: bool, outcome: &PlanPreviewOutcome) {
    let info = match outcome {
        PlanPreviewOutcome::NoMasterRoot => {
            widgets::banner(
                ui,
                "No master ROM root configured",
                "A destination cannot be previewed until a master ROM root is set in Canonical Organisation.",
                widgets::StatusTone::Pending,
            );
            return;
        }
        PlanPreviewOutcome::Planned(info) => info,
    };

    if advanced_mode {
        show_advanced(ui, info);
    } else {
        show_gamer(ui, info);
    }
}

/// Gamer mode: one plain-language consequence card - headline, a plain
/// explanation of *why* (resolver-first: an unresolved identity is always
/// the reason given, never masked by a secondary planner label), and the
/// consequence/destination when the planner actually reached one.
fn show_gamer(ui: &mut egui::Ui, info: &PlanPreviewInfo) {
    let (headline, explanation) = match info.status {
        PlanStatus::Ready => ("Ready to organise", info.resolver_summary.clone()),
        PlanStatus::NeedsReview => (
            "Needs a closer look",
            "Something about this file needs review before it can be organised automatically."
                .to_string(),
        ),
        PlanStatus::Ambiguous => (
            "Not sure yet",
            "More than one identity is still plausible, so no destination can be proposed yet."
                .to_string(),
        ),
        PlanStatus::Conflict => (
            "Conflict",
            "Evidence disagrees about what this file is, so it will not be organised until that \
             is resolved."
                .to_string(),
        ),
        PlanStatus::Unknown => (
            "Unknown",
            "Nothing has identified this file yet, so no destination can be proposed.".to_string(),
        ),
        PlanStatus::Unsupported => (
            "Not supported yet",
            "This file's platform or situation is not one EmuWiz can organise automatically yet."
                .to_string(),
        ),
    };
    let consequence = info
        .destination_display
        .as_deref()
        .map(|destination| format!("This file would move to {destination}."));

    widgets::card(ui, |ui| {
        widgets::status_badge(ui, headline, plan_status_tone(info.status));
        ui.label(explanation);
        if let Some(consequence) = consequence {
            ui.label(consequence);
        }
    });

    widgets::card(ui, |ui| {
        widgets::status_badge(
            ui,
            readiness_headline(info.readiness.outcome),
            readiness_tone(info.readiness.outcome),
        );
        ui.label(readiness_explanation(&info.readiness));
    });
}

/// Advanced mode: the full chain as two distinct steps - the resolver
/// result exactly as computed, then the planner result built from it,
/// then the destination/reason details a planner-only view would miss.
fn show_advanced(ui: &mut egui::Ui, info: &PlanPreviewInfo) {
    widgets::card(ui, |ui| {
        ui.strong("Resolver");
        widgets::status_badge(
            ui,
            info.resolver_status.label(),
            status_tone_for(info.resolver_status),
        );
        ui.label(&info.resolver_summary);
    });
    widgets::card(ui, |ui| {
        ui.strong("Planner");
        widgets::status_badge(
            ui,
            plan_status_label(info.status),
            plan_status_tone(info.status),
        );
        if let Some(destination) = &info.destination_display {
            ui.label(format!("Destination: {destination}"));
        }
        if let Some(platform) = &info.platform_display_name {
            ui.label(format!("Platform: {platform}"));
        }
        if let Some(source) = &info.platform_source {
            ui.label(format!("Platform source: {source}"));
        }
        ui.label(format!(
            "Rename basis: {}",
            rename_basis_label(info.rename_basis)
        ));
        if let Some(proposed) = &info.proposed_name {
            ui.label(format!("Proposed name: {proposed}"));
        }
        ui.label(format!(
            "RomM mapping: {}{}",
            romm_status_label(info.romm_status),
            info.romm_slug
                .as_deref()
                .map(|slug| format!(" ({slug})"))
                .unwrap_or_default()
        ));
        if !info.blockers.is_empty() {
            widgets::technical_details(ui, "plan-preview-blockers", |ui| {
                for blocker in &info.blockers {
                    ui.label(blocker);
                }
            });
        }
    });
    show_advanced_readiness(ui, &info.readiness);
}

/// GUI Batch D: the Apply-readiness card - eligibility, preconditions, the
/// approval requirement, and the frozen-plan digest, all read straight off
/// [`ApplyReadinessInfo`] (built via the real, unchanged
/// `plan_transaction::build_preview`).
fn show_advanced_readiness(ui: &mut egui::Ui, readiness: &ApplyReadinessInfo) {
    widgets::card(ui, |ui| {
        ui.strong("Apply readiness");
        widgets::status_badge(
            ui,
            readiness_headline(readiness.outcome),
            readiness_tone(readiness.outcome),
        );
        ui.label(format!(
            "Operation: {}",
            readiness
                .operation_kind
                .map(operation_kind_label)
                .unwrap_or("None (not eligible)")
        ));
        if let Some(strength) = readiness.precondition_strength {
            ui.label(format!(
                "Precondition: {}",
                precondition_strength_label(strength)
            ));
        }
        ui.label(format!(
            "Approval: {}",
            if readiness.approval_required {
                "Required (not offered by this preview)"
            } else {
                "Not applicable - no eligible operation"
            }
        ));
        ui.label(format!(
            "Canary eligibility: not assessed here. It requires an approved plan, which this \
             preview never creates; when a future batch adds approval, EmuWiz would additionally \
             require the file to sit under a disposable canary root, off the production ROM \
             root, on the same filesystem as its destination, and no larger than {} MB.",
            CANARY_MAX_SIZE_BYTES / (1024 * 1024)
        ));
        if !readiness.blockers.is_empty() {
            widgets::technical_details(ui, "plan-preview-readiness-blockers", |ui| {
                for blocker in &readiness.blockers {
                    ui.label(blocker);
                }
            });
        }
        widgets::technical_details(ui, "plan-preview-digest", |ui| {
            ui.label(format!("Plan digest: {}", readiness.plan_digest));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::content_evidence::{ContentEvidence, ContentEvidenceConfidence};
    use archivefs_core::platform_evidence_fusion::identity_orchestrator::{
        IdentityInspectionInput, inspect_identity,
    };
    use archivefs_core::platform_evidence_fusion::identity_presentation::present_identity;
    use std::io::Write;

    fn gb_rom_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x150];
        let logo: [u8; 48] = [
            0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C,
            0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6,
            0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC,
            0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
        ];
        bytes[0x104..0x134].copy_from_slice(&logo);
        bytes[0x134..0x143].copy_from_slice(b"TESTGAME\0\0\0\0\0\0\0");
        let checksum = archivefs_core::gb_header_evidence::compute_header_checksum(&bytes)
            .expect("checksum computable");
        bytes[0x14D] = checksum;
        bytes
    }

    struct FixtureDir(std::path::PathBuf);

    impl FixtureDir {
        fn new(label: &str) -> Self {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let dir =
                std::env::temp_dir().join(format!("archivefs-gui-plan-preview-{label}-{now}"));
            std::fs::create_dir_all(&dir).expect("create fixture dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_rom(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).expect("create rom fixture");
        file.write_all(bytes).expect("write rom fixture");
        path
    }

    fn unknown_identity() -> IdentityResult {
        inspect_identity(IdentityInspectionInput::default())
    }

    fn gb_identified_identity() -> IdentityResult {
        // The real GB header detector's own output, not a hand-typed
        // fusion-rule literal - see `selected_evidence_page`'s own tests
        // for the same convention.
        let fact = archivefs_core::gb_header_evidence::parse_gb_header(&gb_rom_bytes())
            .map(|header| archivefs_core::gb_header_evidence::observe_gb_evidence(&header))
            .expect("synthetic GB header parses");
        inspect_identity(IdentityInspectionInput {
            content_evidence: fact,
            ..Default::default()
        })
    }

    #[test]
    fn no_master_root_is_reported_honestly_not_a_fabricated_destination() {
        let dir = FixtureDir::new("no-root");
        let rom_path = write_rom(dir.path(), "game.gb", &gb_rom_bytes());
        let identity = unknown_identity();

        let presentation = present_identity(&identity);
        let outcome = gather_plan_preview(&rom_path, &identity, &presentation, None, None);
        assert_eq!(outcome, PlanPreviewOutcome::NoMasterRoot);
    }

    #[test]
    fn unknown_identity_yields_an_unknown_or_unsupported_status_never_a_destination() {
        let dir = FixtureDir::new("unknown");
        let rom_path = write_rom(dir.path(), "game.gb", &gb_rom_bytes());
        let root = FixtureDir::new("unknown-root");
        let identity = unknown_identity();

        let presentation = present_identity(&identity);
        let outcome =
            gather_plan_preview(&rom_path, &identity, &presentation, None, Some(root.path()));
        match outcome {
            PlanPreviewOutcome::Planned(info) => {
                assert!(matches!(
                    info.status,
                    PlanStatus::Unknown | PlanStatus::Unsupported
                ));
                assert!(info.destination_display.is_none());
                assert_eq!(
                    info.readiness.outcome,
                    ApplyReadinessOutcome::CannotSafelyApply
                );
                assert!(!info.readiness.approval_required);
                assert!(info.readiness.operation_kind.is_none());
            }
            other => panic!("expected Planned, got {other:?}"),
        }
    }

    fn conflicting_identity() -> IdentityResult {
        let fact = ContentEvidence::new(
            archivefs_core::content_evidence::ContentEvidenceKind::BootStructure,
            "SEGA SEGASATURN",
            ContentEvidenceConfidence::Strong,
            "validated header",
        );
        let dat = archivefs_core::dat::identity::resolve_dat_platform_identity([
            archivefs_core::dat::identity::DatPlatformEvidence {
                platform: "PSX".to_string(),
                machine_key: None,
                kind: archivefs_core::dat::identity::DatPlatformEvidenceKind::HeaderName,
                confidence: archivefs_core::dat::identity::DatPlatformConfidence::Strong,
                detail: "synthetic DAT".to_string(),
            },
        ]);
        inspect_identity(IdentityInspectionInput {
            content_evidence: vec![fact],
            dat: Some(dat),
            ..Default::default()
        })
    }

    #[test]
    fn conflicting_identity_never_reaches_ready_for_review() {
        let dir = FixtureDir::new("conflict");
        let rom_path = write_rom(dir.path(), "game.gb", &gb_rom_bytes());
        let root = FixtureDir::new("conflict-root");
        let identity = conflicting_identity();

        let presentation = present_identity(&identity);
        let outcome =
            gather_plan_preview(&rom_path, &identity, &presentation, None, Some(root.path()));
        match outcome {
            PlanPreviewOutcome::Planned(info) => {
                assert_eq!(info.status, PlanStatus::Conflict);
                assert_eq!(
                    info.readiness.outcome,
                    ApplyReadinessOutcome::ConflictMustResolve
                );
                assert!(!info.readiness.approval_required);
                assert!(info.readiness.operation_kind.is_none());
            }
            other => panic!("expected Planned, got {other:?}"),
        }
    }

    #[test]
    fn a_ready_identified_plan_is_ready_for_review_with_a_real_operation() {
        let dir = FixtureDir::new("ready");
        let rom_path = write_rom(dir.path(), "Alleyway (Test).gb", &gb_rom_bytes());
        let root = FixtureDir::new("ready-root");
        let identity = gb_identified_identity();

        let presentation = present_identity(&identity);
        let outcome =
            gather_plan_preview(&rom_path, &identity, &presentation, None, Some(root.path()));
        match outcome {
            PlanPreviewOutcome::Planned(info) => {
                assert_eq!(info.status, PlanStatus::Ready);
                assert_eq!(
                    info.readiness.outcome,
                    ApplyReadinessOutcome::ReadyForReview
                );
                assert!(info.readiness.approval_required);
                assert_eq!(info.readiness.operation_kind, Some(OperationKind::Move));
                assert_eq!(
                    info.readiness.precondition_strength,
                    Some(PreconditionStrength::IdentityOnly),
                    "no physical_hash was supplied, so the precondition must be honestly reported \
                     as identity-only, never upgraded to hash-verified"
                );
                assert!(!info.readiness.plan_digest.is_empty());
            }
            other => panic!("expected Planned, got {other:?}"),
        }
    }

    #[test]
    fn a_supplied_physical_hash_upgrades_the_precondition_strength() {
        let dir = FixtureDir::new("hash-verified");
        let rom_path = write_rom(dir.path(), "Alleyway (Test).gb", &gb_rom_bytes());
        let root = FixtureDir::new("hash-verified-root");
        let identity = gb_identified_identity();

        let presentation = present_identity(&identity);
        let outcome = gather_plan_preview(
            &rom_path,
            &identity,
            &presentation,
            Some("deadbeef"),
            Some(root.path()),
        );
        match outcome {
            PlanPreviewOutcome::Planned(info) => {
                assert_eq!(
                    info.readiness.precondition_strength,
                    Some(PreconditionStrength::HashVerified)
                );
            }
            other => panic!("expected Planned, got {other:?}"),
        }
    }

    #[test]
    fn readiness_never_authorizes_or_creates_a_transaction() {
        // `ApplyReadinessInfo` carries no field an executor could consume -
        // no `RenameTransaction`, no journal path, no `ApprovedPlan`. This
        // test documents that boundary by construction: it only reads the
        // fields the struct actually has.
        let dir = FixtureDir::new("no-transaction");
        let rom_path = write_rom(dir.path(), "Alleyway (Test).gb", &gb_rom_bytes());
        let root = FixtureDir::new("no-transaction-root");
        let identity = gb_identified_identity();

        let presentation = present_identity(&identity);
        let outcome =
            gather_plan_preview(&rom_path, &identity, &presentation, None, Some(root.path()));
        let PlanPreviewOutcome::Planned(info) = outcome else {
            panic!("expected Planned");
        };
        let ApplyReadinessInfo {
            outcome: _,
            operation_kind: _,
            precondition_strength: _,
            approval_required: _,
            plan_digest: _,
            blockers: _,
        } = info.readiness;
    }

    #[test]
    fn plan_preview_never_authorizes_a_rename() {
        // The planner's own `RenameSuggestion::authorized` is always
        // false in this milestone - this test documents that this
        // preview neither reads nor could act on a `true` value, since
        // `PlanPreviewInfo` does not even carry the field.
        let dir = FixtureDir::new("no-apply");
        let rom_path = write_rom(dir.path(), "game.gb", &gb_rom_bytes());
        let root = FixtureDir::new("no-apply-root");
        let identity = gb_identified_identity();

        let presentation = present_identity(&identity);
        let outcome =
            gather_plan_preview(&rom_path, &identity, &presentation, None, Some(root.path()));
        assert!(matches!(outcome, PlanPreviewOutcome::Planned(_)));
        // `PlanPreviewInfo` has no `authorized`/`apply`/`confirm` field at
        // all - see its own struct definition above.
    }

    #[test]
    fn action_vocabulary_is_load_only_never_a_mutation() {
        fn assert_read_only(action: PlanPreviewAction) {
            match action {
                PlanPreviewAction::Load => {}
            }
        }
        assert_read_only(PlanPreviewAction::Load);
    }

    // -- render smoke tests -------------------------------------------

    #[test]
    fn idle_panel_renders_without_panicking() {
        let ctx = egui::Context::default();
        let state = PlanPreviewState::Idle;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_plan_preview_panel(ui, false, Some(Path::new("game.gb")), &state);
            });
        });
    }

    #[test]
    fn panel_with_no_selection_renders_without_panicking() {
        let ctx = egui::Context::default();
        let state = PlanPreviewState::Idle;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_plan_preview_panel(ui, false, None, &state);
            });
        });
    }

    #[test]
    fn no_master_root_outcome_renders_in_both_modes_without_panicking() {
        for advanced in [false, true] {
            let ctx = egui::Context::default();
            let state = PlanPreviewState::Ready {
                generation: 1,
                outcome: PlanPreviewOutcome::NoMasterRoot,
            };
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ =
                        show_plan_preview_panel(ui, advanced, Some(Path::new("game.gb")), &state);
                });
            });
        }
    }

    #[test]
    fn planned_outcome_renders_in_both_modes_without_panicking() {
        let dir = FixtureDir::new("render");
        let rom_path = write_rom(dir.path(), "game.gb", &gb_rom_bytes());
        let root = FixtureDir::new("render-root");
        let identity = gb_identified_identity();
        let presentation = present_identity(&identity);
        let outcome =
            gather_plan_preview(&rom_path, &identity, &presentation, None, Some(root.path()));

        for advanced in [false, true] {
            let ctx = egui::Context::default();
            let state = PlanPreviewState::Ready {
                generation: 1,
                outcome: outcome.clone(),
            };
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show_plan_preview_panel(ui, advanced, Some(rom_path.as_path()), &state);
                });
            });
        }
    }

    #[test]
    fn loading_panel_renders_without_panicking() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let ctx = egui::Context::default();
        let state = PlanPreviewState::Loading {
            generation: 1,
            receiver,
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_plan_preview_panel(ui, false, Some(Path::new("game.gb")), &state);
            });
        });
    }
}
