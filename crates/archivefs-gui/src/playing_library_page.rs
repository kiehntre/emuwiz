//! The "Build Playing Library" flow: a thin GUI over
//! `archivefs_core::playing_library`'s read-only 1G1R planner.
//!
//! Reached from the Library Organisation page as a mode, not a new sidebar
//! destination (see `rom_organisation_page::show_rom_organisation_page`).
//! This page never re-implements grouping, evidence parsing, or election -
//! it only collects a source root, a destination root, a DAT catalogue path,
//! and a few plain-language preferences, hands them to
//! `archivefs_core::playing_library::build_playing_library_plan`, and shows
//! the result. Selecting an elected family shows its own
//! `ElectionExplanation` verbatim - there is no second explanation model
//! here. Applying builds a `RenameTransaction` via
//! `archivefs_core::playing_library::build_playing_library_transaction` and
//! runs it through the exact shared `rename_apply` executor every other
//! apply path in this app already uses; there is no second filesystem
//! engine anywhere in this file.

pub(crate) mod preferences;
mod presentation;
mod retrodeck;

use presentation::PreviewFilter;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use archivefs_core::dat::catalogue_selection::{CatalogueRef, resolve_catalogue};
use archivefs_core::dat::identity::DatPlatformIdentity;
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::rename_apply::executor::{
    ApplyExecution, HardConflictMode, apply_transaction,
};
use archivefs_core::dat::rename_apply::identity::{capture_identity, identity_matches};
use archivefs_core::dat::rename_apply::journal::{default_rename_transaction_dir, write_journal};
use archivefs_core::dat::rename_apply::model::{ObjectIdentity, ObjectKind};
use archivefs_core::dat::rename_apply::model::{RenameTransaction, TransactionState};
use archivefs_core::dat::rename_apply::preflight::DirectoryPolicy;
use archivefs_core::dat::rename_apply::rollback::rollback_transaction;
use archivefs_core::emulator_environment::es_de::{EsDeEnvironmentReport, EsDeProfile};
use archivefs_core::launch::es_de_export::{ES_DE_SYSTEM_MAP, es_de_system_for_platform};
use archivefs_core::launch::es_de_publish::{
    EsDeGamelistPublication, EsDePublicationError, apply_es_de_gamelist_publication,
    has_unresolved_es_de_gamelist_recovery, plan_es_de_gamelist_publication,
    recover_es_de_gamelist_publication,
};
use archivefs_core::playing_library::apply_adapter::prepare_destination_root;
use archivefs_core::playing_library::{
    CandidateEvidenceSummary, DatArchiveMatch, LibraryOutputProfile, LibraryOutputProjection,
    PlayingLibraryPlan, PlayingLibraryPolicy, ReleaseClass, RetroDeckProjectionPlan,
    RetroDeckVisibility, RommLibraryProjectionPlan, RommVisibility, build_playing_library_plan,
    build_playing_library_transaction, build_retrodeck_projection,
    build_retrodeck_projection_transaction, build_romm_projection_transaction,
    build_romm_projection_with_visibility, inspect_library_output_ownership,
    match_loose_files_against_dat, override_verified_candidate, project_generic_playing_library,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;

use crate::dat_catalogue_picker::{DatCataloguePickerState, DatCatalogueWorkflow};
use crate::rom_organisation_page::collect_source_files_reported;
use crate::ui::{components as widgets, theme};

/// The confirmation phrase a user must type before a Create Playing Library
/// apply runs - truthful
/// wording, matching `crate::rom_organisation_page::apply_confirmation_phrase`
/// for `OrganisationMode::BuildLinkedLibrary`: a link is created, nothing is
/// renamed or moved.
pub(crate) fn playing_library_confirmation_phrase(count: usize) -> String {
    format!("CREATE {count} LINKS")
}

/// Beginner-facing destination choice. All variants use the existing
/// Playing Library election and apply machinery; this selects the reviewed
/// destination projection shown by the workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlayingLibraryDestination {
    #[default]
    Generic,
    Romm,
    EsDe,
    /// RetroDECK uses its own ES-DE-compatible filesystem root and sandbox
    /// visibility contract. It is deliberately separate from ES-DE
    /// gamelist publication, which remains an explicit follow-up action.
    RetroDeck,
}

/// The page's authoritative state.
#[derive(Clone)]
pub(crate) struct PlayingLibraryPageState {
    pub(crate) destination: PlayingLibraryDestination,
    pub(crate) selected_catalogue: Option<CatalogueRef>,
    pub(crate) catalogue_picker: DatCataloguePickerState,
    #[cfg(test)]
    pub(crate) dat_path_draft: String,
    pub(crate) source_root_draft: String,
    pub(crate) destination_root_draft: String,
    /// Comma-separated, most-preferred first, e.g. `"Europe, USA, Japan"`.
    pub(crate) preferred_regions_draft: String,
    /// Comma-separated, most-preferred first, e.g. `"English"` maps to the
    /// recognized `en` code.
    pub(crate) preferred_languages_draft: String,
    pub(crate) prefer_newest_revision: bool,
    pub(crate) prefer_parent: bool,
    pub(crate) exclude_beta: bool,
    pub(crate) exclude_proto: bool,
    pub(crate) exclude_demo: bool,
    pub(crate) exclude_sample: bool,
    plan: Option<PlayingLibraryPlan>,
    /// Immutable, single-attempt source/content evidence captured before the
    /// preview is shown. Confirmation must never recapture a new baseline.
    preview_transaction: Option<(PlayingLibraryPlan, Result<RenameTransaction, String>)>,
    verified_dat: Option<ParsedDat>,
    verified_matches: Vec<DatArchiveMatch>,
    candidate_overrides: BTreeMap<String, PathBuf>,
    /// Content and object bindings captured during the reviewed preview.
    preview_sources: BTreeMap<PathBuf, ObjectIdentity>,
    output_projection: Option<LibraryOutputProjection>,
    dat_platform_identity: Option<DatPlatformIdentity>,
    plan_generation: u64,
    error: Option<String>,
    /// The elected family currently shown in "Why this one?", identified by
    /// its own `dat_entry_name` (unique per election within one plan).
    selected_family: Option<String>,
    /// Presentation only: observations of the existing read-only matcher.
    review_paths: Vec<PathBuf>,
    source_files_examined: usize,
    preview_filter: PreviewFilter,
    preview_page: usize,
    pending_apply: Option<usize>,
    pub(crate) confirm_text: String,
    applied: Option<RenameTransaction>,
    romm_projection: Option<RommLibraryProjectionPlan>,
    romm_preview_transaction:
        Option<(RommLibraryProjectionPlan, Result<RenameTransaction, String>)>,
    romm_pending_apply: bool,
    romm_confirm_text: String,
    romm_applied: Option<RenameTransaction>,
    romm_error: Option<String>,
    romm_visibility_verified: bool,
    romm_visible_source_root_draft: String,
    retrodeck_projection: Option<RetroDeckProjectionPlan>,
    retrodeck_preview_transaction:
        Option<(RetroDeckProjectionPlan, Result<RenameTransaction, String>)>,
    retrodeck_error: Option<String>,
    retrodeck_visibility_verified: bool,
    retrodeck_visible_source_root_draft: String,
    retrodeck_destination_root_draft: String,
    retrodeck_pending_apply: bool,
    retrodeck_confirm_text: String,
    retrodeck_applied: Option<RenameTransaction>,
    retrodeck_phase: Option<archivefs_core::launch::es_de_publish::operation::Phase>,
    retrodeck_selected_id: Option<String>,
    retrodeck_operations: Vec<archivefs_core::launch::es_de_publish::operation::OperationOutcome>,
    retrodeck_discovery_errors: Vec<String>,
    /// The plan that produced `applied`, kept (instead of dropped like
    /// `plan` is) purely so "Publish to ES-DE" - offered only once a
    /// library has actually been created - can still name which elections
    /// are eligible. Never mutated once set; `plan_es_de_gamelist_publication`
    /// reads it exactly as `build_playing_library_transaction` already did.
    applied_plan: Option<PlayingLibraryPlan>,
    apply_error: Option<String>,
    /// `apply_error` came from an Undo attempt, not from Apply.
    apply_error_is_undo: bool,
    journal_dir: PathBuf,
    async_busy: bool,
    apply_stage: Arc<AtomicU8>,

    // --- "Publish to ES-DE" (see `archivefs_core::launch::es_de_publish`) ---
    //
    // This page never parses, writes, or recovers a gamelist itself - every
    // field below only remembers which choice the user made and which
    // result the core API already returned.
    esde_platform_id: Option<&'static str>,
    esde_profile: Option<EsDeProfile>,
    esde_discovery_error: Option<String>,
    /// Set when a preview finds an unresolved recovery record for the
    /// resolved gamelist path - the only state while this is `Some` is
    /// "explain it and offer to restore", per
    /// `archivefs_core::launch::es_de_publish`'s own recovery policy.
    esde_recovery_gamelist_path: Option<PathBuf>,
    esde_recovery_pending: bool,
    /// Beginner-facing text, plus - only when the failure actually came
    /// from `archivefs_core::launch::es_de_publish` - its own raw `Display`
    /// text, shown only behind an expandable "Technical details" section
    /// (see [`esde_friendly_error`]).
    esde_recovery_error: Option<(String, Option<String>)>,
    esde_recovery_done: bool,
    esde_publication: Option<EsDeGamelistPublication>,
    esde_preview_error: Option<(String, Option<String>)>,
    esde_pending_publish: bool,
    esde_publish_error: Option<(String, Option<String>)>,
    esde_published: bool,
    /// Test-only ES-DE discovery seam, exactly like `journal_dir`/
    /// `with_journal_dir` above: production always calls
    /// `discover_es_de_environment_default`, so a test never depends on the
    /// developer's real `$HOME` or a real ES-DE install.
    #[cfg(test)]
    esde_home_override: Option<PathBuf>,
}

impl Default for PlayingLibraryPageState {
    fn default() -> Self {
        Self {
            destination: PlayingLibraryDestination::Generic,
            selected_catalogue: None,
            catalogue_picker: DatCataloguePickerState::default(),
            #[cfg(test)]
            dat_path_draft: String::new(),
            source_root_draft: String::new(),
            destination_root_draft: String::new(),
            preferred_regions_draft: "Europe, USA, Japan, Other".to_string(),
            preferred_languages_draft: String::new(),
            prefer_newest_revision: true,
            prefer_parent: true,
            exclude_beta: true,
            exclude_proto: true,
            exclude_demo: true,
            exclude_sample: true,
            plan: None,
            preview_transaction: None,
            verified_dat: None,
            verified_matches: Vec::new(),
            candidate_overrides: BTreeMap::new(),
            preview_sources: BTreeMap::new(),
            output_projection: None,
            dat_platform_identity: None,
            plan_generation: 0,
            error: None,
            selected_family: None,
            review_paths: Vec::new(),
            source_files_examined: 0,
            preview_filter: PreviewFilter::All,
            preview_page: 0,
            pending_apply: None,
            confirm_text: String::new(),
            applied: None,
            romm_projection: None,
            romm_preview_transaction: None,
            romm_pending_apply: false,
            romm_confirm_text: String::new(),
            romm_applied: None,
            romm_error: None,
            romm_visibility_verified: false,
            romm_visible_source_root_draft: String::new(),
            retrodeck_projection: None,
            retrodeck_preview_transaction: None,
            retrodeck_error: None,
            retrodeck_visibility_verified: false,
            retrodeck_visible_source_root_draft: String::new(),
            retrodeck_destination_root_draft: String::new(),
            retrodeck_pending_apply: false,
            retrodeck_confirm_text: String::new(),
            retrodeck_applied: None,
            retrodeck_phase: None,
            retrodeck_selected_id: None,
            retrodeck_operations: Vec::new(),
            retrodeck_discovery_errors: Vec::new(),
            applied_plan: None,
            apply_error: None,
            apply_error_is_undo: false,
            journal_dir: default_rename_transaction_dir().unwrap_or_else(|_| {
                PathBuf::from(archivefs_core::dat::rename_apply::RENAME_TRANSACTIONS_DIRECTORY)
            }),
            async_busy: false,
            apply_stage: Arc::new(AtomicU8::new(0)),
            esde_platform_id: None,
            esde_profile: None,
            esde_discovery_error: None,
            esde_recovery_gamelist_path: None,
            esde_recovery_pending: false,
            esde_recovery_error: None,
            esde_recovery_done: false,
            esde_publication: None,
            esde_preview_error: None,
            esde_pending_publish: false,
            esde_publish_error: None,
            esde_published: false,
            #[cfg(test)]
            esde_home_override: None,
        }
    }
}

/// English's recognized evidence code, used to translate the plain-language
/// "English" checkbox into the same token `evidence::dat_release_evidence`
/// recognizes. Extending this to more languages is future GUI work; the
/// core planner already accepts any recognized code via
/// `preferred_languages_draft`, typed directly.
const ENGLISH_LANGUAGE_CODE: &str = "en";

fn split_preference_list(draft: &str) -> Vec<String> {
    draft
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.eq_ignore_ascii_case("english") {
                ENGLISH_LANGUAGE_CODE.to_string()
            } else {
                value.to_string()
            }
        })
        .collect()
}

/// Outcome wording for a failed Apply or Undo, from what the receipt records
/// rather than from the error text: refused (nothing changed), partially
/// completed (recorded links remain, Undo offered) or Undo not finished.
fn apply_error_heading(
    applied: Option<&RenameTransaction>,
    is_undo: bool,
) -> (&'static str, &'static str) {
    if is_undo {
        // Only the receipt can say whether links remain, and a refused Undo
        // (for example a lost journal seal) is not a promise of a retry.
        let remaining = applied.map_or(0, |tx| {
            tx.entries
                .iter()
                .filter(|e| e.blocks_full_rollback())
                .count()
        });
        return if remaining > 0 {
            (
                "Undo did not finish",
                "The receipt still records links that were not confirmed as removed. Your original games are untouched. Review Technical details before trying Undo again; it may be refused if the record cannot be verified.",
            )
        } else {
            (
                "Undo did not finish",
                "The receipt records no links still awaiting removal. Your original games are untouched. Review Technical details.",
            )
        };
    }
    match applied {
        Some(tx) if tx.applied_count() > 0 => (
            "Partly created",
            "Some links were created and are recorded in the receipt below; the rest were not. Your original games are untouched. Use Undo to remove the recorded links.",
        ),
        _ => (
            "Not created",
            "No links were created. Your original games are untouched.",
        ),
    }
}

// The core deliberately returns Ok for a completed rollback *attempt*. Only
// FullyRolledBack means success; background completion uses destination_error.
fn incomplete_undo_error(
    outcome: Result<archivefs_core::dat::rename_apply::RollbackOutcome, String>,
) -> Option<String> {
    use archivefs_core::dat::rename_apply::model::RollbackResult;
    match outcome {
        Err(error) => Some(error),
        Ok(outcome) => match outcome.result {
            RollbackResult::FullyRolledBack => None,
            result => Some(format!(
                "Undo did not finish. Review recovery before retrying: {}",
                result
                    .failed()
                    .iter()
                    .map(|(path, reason)| format!("{}: {reason}", path.display()))
                    .collect::<Vec<_>>()
                    .join("; "),
            )),
        },
    }
}

impl PlayingLibraryPageState {
    pub(crate) fn destination_transaction(&self) -> Option<&RenameTransaction> {
        match self.destination {
            PlayingLibraryDestination::Romm => self.romm_applied.as_ref(),
            PlayingLibraryDestination::RetroDeck => self.retrodeck_applied.as_ref(),
            PlayingLibraryDestination::Generic | PlayingLibraryDestination::EsDe => {
                self.applied.as_ref()
            }
        }
    }

    pub(crate) fn destination_error(&self) -> Option<&str> {
        match self.destination {
            PlayingLibraryDestination::Romm => self.romm_error.as_deref(),
            PlayingLibraryDestination::RetroDeck => self.retrodeck_error.as_deref(),
            PlayingLibraryDestination::Generic | PlayingLibraryDestination::EsDe => {
                self.apply_error.as_deref().or(self.error.as_deref())
            }
        }
    }

    pub(crate) fn esde_operation_error(&self) -> Option<&str> {
        self.esde_recovery_error
            .as_ref()
            .or(self.esde_publish_error.as_ref())
            .or(self.esde_preview_error.as_ref())
            .map(|error| error.0.as_str())
            .or(self.esde_discovery_error.as_deref())
    }

    /// Projects current workflow-owned errors without repeating a preview or
    /// checking a destination. A refreshed workflow replaces these facts.
    pub(crate) fn attention_snapshot(&self) -> archivefs_core::attention::AttentionSnapshot {
        use archivefs_core::attention::*;
        let mut snapshot = AttentionSnapshot::default();
        if let Some(transaction) = &self.romm_applied {
            if let Some(item) = operation_attention(
                &archivefs_core::operation::romm_publication_operation(transaction, None),
            ) {
                snapshot.insert(item);
            }
        } else if let Some(error) = &self.romm_error {
            let mut item = AttentionItem::new(
                format!("romm-preview:{}", self.destination_root_draft),
                AttentionCategory::Publication,
                AttentionSeverity::ActionNeeded,
                "RomM publication needs review".into(),
                AttentionDestination::Romm,
            );
            item.summary = error.clone();
            item.source_workflow = "RomM publication".into();
            item.provenance =
                "Current publication workflow error, not an inferred destination check".into();
            snapshot.insert(item);
        }
        let esde_error = self
            .esde_recovery_error
            .as_ref()
            .or(self.esde_publish_error.as_ref())
            .or(self.esde_preview_error.as_ref());
        let recovery_pending =
            self.esde_recovery_gamelist_path.is_some() && !self.esde_recovery_done;
        if esde_error.is_some() || recovery_pending {
            let mut item = AttentionItem::new(
                format!("esde-publication:{}", self.destination_root_draft),
                AttentionCategory::Publication,
                AttentionSeverity::ActionNeeded,
                "ES-DE publication needs review".into(),
                AttentionDestination::EsDe,
            );
            item.summary = esde_error.map(|e| e.0.clone()).unwrap_or_else(|| {
                "The existing ES-DE preview found an unresolved publication recovery record.".into()
            });
            item.platform = self.esde_platform_id.map(str::to_owned);
            item.affected = self
                .esde_recovery_gamelist_path
                .as_ref()
                .or(self.esde_publication.as_ref().map(|p| &p.gamelist_path))
                .map(|p| p.display().to_string());
            item.source_workflow = "ES-DE publication".into();
            item.provenance = "Current ES-DE publication/recovery workflow result".into();
            if let Some((_, Some(detail))) = esde_error {
                item.source_records.push(detail.clone());
            }
            snapshot.insert(item);
        }
        snapshot
    }

    fn has_catalogue_selection(&self) -> bool {
        let selected = self.selected_catalogue.as_ref().is_some_and(|reference| {
            self.catalogue_picker
                .is_usable(DatCatalogueWorkflow::PlayingLibrary, reference)
        });
        #[cfg(test)]
        {
            selected || !self.dat_path_draft.trim().is_empty()
        }
        #[cfg(not(test))]
        selected
    }

    pub(crate) fn load() -> Self {
        let mut state = Self::default();
        state.rediscover_retrodeck_operations();
        state
    }

    pub(crate) fn set_destination(&mut self, destination: PlayingLibraryDestination) {
        self.destination = destination;
        // Reuse the preview revision so even same-destination re-entry
        // invalidates an older worker carrying a superseded approval.
        self.plan_generation = self.plan_generation.wrapping_add(1);
        self.plan = None;
        self.preview_transaction = None;
        self.output_projection = None;
        self.romm_projection = None;
        self.romm_preview_transaction = None;
        self.retrodeck_projection = None;
        self.retrodeck_preview_transaction = None;
        // Approval belongs to the displayed destination and preview, never
        // to a later plan with the same election count. Keep completed
        // receipts and publication/recovery state available to History/Undo.
        self.dat_platform_identity = None;
        self.selected_family = None;
        self.pending_apply = None;
        self.confirm_text.clear();
        self.romm_pending_apply = false;
        self.romm_confirm_text.clear();
        self.retrodeck_pending_apply = false;
        self.retrodeck_confirm_text.clear();
        self.error = None;
        if self.applied.is_none() {
            self.apply_error = None;
        }
        if self.romm_applied.is_none() {
            self.romm_error = None;
        }
        if self.retrodeck_applied.is_none() && self.retrodeck_selected_id.is_none() {
            self.retrodeck_error = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn with_journal_dir(journal_dir: PathBuf) -> Self {
        Self {
            journal_dir,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn with_esde_home_override(mut self, home: PathBuf) -> Self {
        self.esde_home_override = Some(home);
        self
    }

    pub(crate) fn plan(&self) -> Option<&PlayingLibraryPlan> {
        self.plan.as_ref()
    }

    pub(crate) fn input_fingerprint(&self) -> String {
        format!(
            "{:?}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{}",
            self.destination,
            self.selected_catalogue,
            self.source_root_draft,
            self.destination_root_draft,
            self.preferred_regions_draft,
            self.preferred_languages_draft,
            self.prefer_newest_revision,
            self.prefer_parent,
            self.exclude_beta,
            self.exclude_proto,
            self.exclude_demo,
            self.exclude_sample,
            self.candidate_overrides,
            self.plan_generation,
        )
    }

    pub(crate) fn set_candidate_override(&mut self, family: String, source: PathBuf) {
        self.candidate_overrides.insert(family, source);
    }

    pub(crate) fn clear_candidate_overrides(&mut self) {
        self.candidate_overrides.clear();
    }

    pub(crate) fn verified_paths_named(&self, name: &str) -> Vec<PathBuf> {
        let Some(dat) = &self.verified_dat else {
            return Vec::new();
        };
        self.verified_matches
            .iter()
            .filter(|candidate| {
                dat.games
                    .get(candidate.dat_entry_index)
                    .is_some_and(|entry| entry.name == name)
            })
            .map(|candidate| candidate.archive_path.clone())
            .collect()
    }

    pub(crate) fn merge_async_apply_result(&mut self, result: Self) {
        self.preview_transaction = result.preview_transaction;
        self.pending_apply = result.pending_apply;
        self.confirm_text = result.confirm_text;
        self.applied = result.applied;
        self.applied_plan = result.applied_plan;
        self.apply_error = result.apply_error;
        self.apply_error_is_undo = result.apply_error_is_undo;
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn apply_error(&self) -> Option<&str> {
        self.apply_error.as_deref()
    }

    pub(crate) fn applied(&self) -> Option<&RenameTransaction> {
        self.applied.as_ref()
    }

    pub(crate) fn selected_family(&self) -> Option<&str> {
        self.selected_family.as_deref()
    }

    pub(crate) fn select_family(&mut self, name: Option<String>) {
        self.selected_family = name;
    }

    /// Builds the policy the current draft fields describe. Pure and cheap,
    /// so tests can assert on it directly without running a preview.
    pub(crate) fn build_policy(&self) -> PlayingLibraryPolicy {
        let mut excluded_release_classes = Vec::new();
        if self.exclude_beta {
            excluded_release_classes.push(ReleaseClass::Beta);
        }
        if self.exclude_proto {
            excluded_release_classes.push(ReleaseClass::Proto);
        }
        if self.exclude_demo {
            excluded_release_classes.push(ReleaseClass::Demo);
        }
        if self.exclude_sample {
            excluded_release_classes.push(ReleaseClass::Sample);
        }
        PlayingLibraryPolicy {
            mode: archivefs_core::playing_library::PlayingLibraryPolicyMode::Console1g1r,
            arcade_evidence: std::collections::BTreeMap::new(),
            preferred_regions: split_preference_list(&self.preferred_regions_draft),
            preferred_languages: split_preference_list(&self.preferred_languages_draft),
            prefer_newest_revision: self.prefer_newest_revision,
            prefer_parent: self.prefer_parent,
            excluded_release_classes,
        }
    }

    /// Parses the configured DAT, hash-matches the source folder's loose
    /// files against it, and runs the real core planner. Never touches the
    /// filesystem beyond reading the DAT and hashing candidate files -
    /// nothing is written, moved, or created.
    pub(crate) fn preview(&mut self) {
        self.plan = None;
        self.preview_transaction = None;
        self.verified_dat = None;
        self.verified_matches.clear();
        self.preview_sources.clear();
        self.output_projection = None;
        self.dat_platform_identity = None;
        self.romm_projection = None;
        self.romm_preview_transaction = None;
        // Keep the existing projection receipt available to Undo, but never
        // let a new base preview authorize its old Apply binding.
        self.retrodeck_preview_transaction = None;
        self.romm_error = None;
        self.romm_visibility_verified = false;
        self.error = None;
        self.selected_family = None;
        self.review_paths.clear();
        self.preview_page = 0;
        self.source_files_examined = 0;
        self.plan_generation += 1;

        let source_root = PathBuf::from(self.source_root_draft.trim());
        let destination_root = PathBuf::from(self.destination_root_draft.trim());
        if self.source_root_draft.trim().is_empty() {
            self.error = Some("choose a source library folder first".to_string());
            return;
        }
        if !destination_root.is_absolute() {
            self.error = Some("the destination folder must be an absolute path".to_string());
            return;
        }

        let Some(reference) = self.selected_catalogue.clone() else {
            #[cfg(test)]
            if !self.dat_path_draft.trim().is_empty() {
                return self.preview_test_dat_path(&source_root, &destination_root);
            }
            self.error = Some("choose a DAT catalogue file first".to_string());
            return;
        };

        let Some(snapshot) = self.catalogue_picker.snapshot() else {
            self.error = Some("the catalogue list is still loading; try again shortly".into());
            return;
        };
        let resolved = match resolve_catalogue(&reference, snapshot.inputs()) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.error = Some(format!("could not resolve the selected catalogue: {error}"));
                return;
            }
        };
        self.dat_platform_identity = Some(resolved.dat_platform_identity());

        let scan = collect_source_files_reported(std::slice::from_ref(&source_root), 20_000, 16);
        if !scan.incomplete.is_empty() {
            self.error = Some(format!(
                "source scan is incomplete; no selection was made: {}",
                scan.incomplete[0]
            ));
            return;
        }
        let candidates = scan.files;
        let trusted = TrustedRoots::from_paths([&source_root]);
        let outcome_matches = match_loose_files_against_dat(
            &resolved.parsed,
            &candidates,
            &trusted,
            &AtomicBool::new(false),
        );

        self.source_files_examined = candidates.len();
        self.review_paths = presentation::unrepresented_paths(&candidates, &outcome_matches);

        let request = resolved.playing_library_request(
            outcome_matches.matches.clone(),
            destination_root.clone(),
            self.build_policy(),
        );
        match build_playing_library_plan(&request) {
            Ok(mut plan) => {
                self.verified_dat = Some(resolved.parsed.clone());
                self.verified_matches = outcome_matches.matches;
                plan.rejected_launchers = outcome_matches.rejected_launchers;
                for (family, source) in &self.candidate_overrides {
                    plan = match override_verified_candidate(
                        &plan,
                        &resolved.parsed,
                        &self.verified_matches,
                        family,
                        source,
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            self.error = Some(format!("override needs review: {error}"));
                            return;
                        }
                    };
                }
                if self.destination == PlayingLibraryDestination::Generic {
                    plan = match project_generic_playing_library(
                        &plan,
                        self.dat_platform_identity
                            .as_ref()
                            .expect("identity set above"),
                        destination_root.to_path_buf(),
                    ) {
                        Ok(projected) => projected,
                        Err(error) => {
                            self.error = Some(error);
                            return;
                        }
                    };
                    self.output_projection =
                        Some(LibraryOutputProjection::from_plan_with_ownership(
                            LibraryOutputProfile::Generic,
                            destination_root.to_path_buf(),
                            &plan,
                            generic_mapping_explanation(self.dat_platform_identity.as_ref()),
                            &inspect_library_output_ownership(&plan, &self.journal_dir),
                        ));
                } else if self.destination == PlayingLibraryDestination::EsDe {
                    self.output_projection =
                        Some(LibraryOutputProjection::from_plan_with_ownership(
                            LibraryOutputProfile::EsDe,
                            destination_root,
                            &plan,
                            Vec::new(),
                            &inspect_library_output_ownership(&plan, &self.journal_dir),
                        ));
                }
                self.retain_reviewed_plan(plan);
            }
            Err(error) => self.error = Some(error),
        }
    }

    #[cfg(test)]
    fn preview_test_dat_path(&mut self, source_root: &Path, destination_root: &Path) {
        let outcome = match archivefs_core::dat::parsers::parse_dat_file(
            Path::new(self.dat_path_draft.trim()),
            archivefs_core::dat::limits::DatLimits::default(),
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.error = Some(format!("could not read the DAT catalogue: {error}"));
                return;
            }
        };
        self.dat_platform_identity = Some(archivefs_core::dat::identity::identify_dat_source(
            &outcome.dat,
        ));
        let source_roots = [source_root.to_path_buf()];
        let scan = collect_source_files_reported(&source_roots, 20_000, 16);
        if !scan.incomplete.is_empty() {
            self.error = Some(format!(
                "source scan is incomplete; no selection was made: {}",
                scan.incomplete[0]
            ));
            return;
        }
        let candidates = scan.files;
        let trusted = TrustedRoots::from_paths([source_root]);
        let outcome_matches = match_loose_files_against_dat(
            &outcome.dat,
            &candidates,
            &trusted,
            &AtomicBool::new(false),
        );
        self.source_files_examined = candidates.len();
        self.review_paths = presentation::unrepresented_paths(&candidates, &outcome_matches);

        let request = archivefs_core::playing_library::PlayingLibraryRequest {
            dat: &outcome.dat,
            matches: outcome_matches.matches.clone(),
            destination_root: destination_root.to_path_buf(),
            policy: self.build_policy(),
        };
        match build_playing_library_plan(&request) {
            Ok(mut plan) => {
                self.verified_dat = Some(outcome.dat.clone());
                self.verified_matches = outcome_matches.matches;
                plan.rejected_launchers = outcome_matches.rejected_launchers;
                for (family, source) in &self.candidate_overrides {
                    plan = match override_verified_candidate(
                        &plan,
                        &outcome.dat,
                        &self.verified_matches,
                        family,
                        source,
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            self.error = Some(format!("override needs review: {error}"));
                            return;
                        }
                    };
                }
                if self.destination == PlayingLibraryDestination::Generic {
                    plan = match project_generic_playing_library(
                        &plan,
                        self.dat_platform_identity
                            .as_ref()
                            .expect("identity set above"),
                        destination_root.to_path_buf(),
                    ) {
                        Ok(projected) => projected,
                        Err(error) => {
                            self.error = Some(error);
                            return;
                        }
                    };
                    self.output_projection =
                        Some(LibraryOutputProjection::from_plan_with_ownership(
                            LibraryOutputProfile::Generic,
                            destination_root.to_path_buf(),
                            &plan,
                            generic_mapping_explanation(self.dat_platform_identity.as_ref()),
                            &inspect_library_output_ownership(&plan, &self.journal_dir),
                        ));
                }
                self.retain_reviewed_plan(plan);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn retain_reviewed_plan(&mut self, plan: PlayingLibraryPlan) {
        if let Err(error) = self.bind_preview_sources(&plan) {
            self.error = Some(error);
            return;
        }
        let transaction = build_playing_library_transaction(&plan, self.plan_generation).and_then(
            |transaction| {
                if transaction.entries.len() != plan.operations.len() {
                    Err("one or more preview sources cannot be bound; run a fresh preview".into())
                } else {
                    Ok(transaction)
                }
            },
        );
        self.preview_transaction = Some((plan.clone(), transaction));
        self.plan = Some(plan);
    }

    pub(crate) fn request_apply(&mut self) {
        let Some(plan) = &self.plan else {
            return;
        };
        self.apply_error = None;
        self.apply_stage.store(0, Ordering::Relaxed);
        self.pending_apply = Some(
            self.output_projection
                .as_ref()
                .map_or(plan.operations.len(), |projection| {
                    projection.operation_count
                }),
        );
        self.confirm_text.clear();
    }

    pub(crate) fn cancel_apply(&mut self) {
        self.pending_apply = None;
        self.confirm_text.clear();
    }

    /// Reuse the original Playing Library preview's source proof. A profile
    /// preview may change output paths, never approve new source objects or
    /// bytes merely because the projection builder recaptured them.
    fn bind_projection_transaction(
        &self,
        transaction: Result<RenameTransaction, String>,
    ) -> Result<RenameTransaction, String> {
        let transaction = transaction?;
        let Some((reviewed_plan, Ok(reviewed))) = &self.preview_transaction else {
            return Err(
                "source evidence is unavailable; run a fresh Playing Library Preview".into(),
            );
        };
        if self.plan.as_ref() != Some(reviewed_plan)
            || reviewed.plan_generation != self.plan_generation
        {
            return Err("source plan is stale; run a fresh Playing Library Preview".into());
        }
        let sources: std::collections::BTreeMap<_, _> = reviewed
            .entries
            .iter()
            .map(|entry| (entry.source_path.as_path(), &entry.identity))
            .collect();
        for entry in &transaction.entries {
            let Some(expected) = sources.get(entry.source_path.as_path()) else {
                return Err("source was not reviewed; run a fresh Playing Library Preview".into());
            };
            if !archivefs_core::dat::rename_apply::identity::identity_matches(
                expected,
                &entry.identity,
            ) {
                return Err("source changed; run a fresh Playing Library Preview".into());
            }
        }
        Ok(transaction)
    }

    fn retain_romm_projection(&mut self, projection: RommLibraryProjectionPlan) {
        let transaction = self.bind_projection_transaction(build_romm_projection_transaction(
            &projection,
            self.plan_generation,
        ));
        self.romm_preview_transaction = Some((projection.clone(), transaction));
        self.romm_projection = Some(projection);
    }

    fn retain_retrodeck_projection(&mut self, projection: RetroDeckProjectionPlan) {
        let transaction = self.bind_projection_transaction(build_retrodeck_projection_transaction(
            &projection,
            self.plan_generation,
        ));
        self.retrodeck_preview_transaction = Some((projection.clone(), transaction));
        self.retrodeck_projection = Some(projection);
    }

    pub(crate) fn preview_romm(&mut self) {
        self.romm_projection = None;
        self.romm_preview_transaction = None;
        self.romm_error = None;
        let (Some(plan), Some(identity)) = (&self.plan, &self.dat_platform_identity) else {
            self.romm_error = Some("build a Playing Library preview first".to_string());
            return;
        };
        let source_root = self.source_root_draft_path();
        let visible_root = PathBuf::from(self.romm_visible_source_root_draft.trim());
        let visibility = if self.romm_visibility_verified
            && !visible_root.as_os_str().is_empty()
            && visible_root == source_root
        {
            RommVisibility::verified_same_path_bind(source_root)
                .unwrap_or_else(|_| RommVisibility::unverified(None, None))
        } else {
            RommVisibility::unverified(
                Some(source_root),
                (!visible_root.as_os_str().is_empty()).then_some(visible_root),
            )
        };
        match build_romm_projection_with_visibility(
            plan,
            identity,
            PathBuf::from(self.destination_root_draft.trim()),
            visibility,
        ) {
            Ok(projection) => {
                self.output_projection = Some(LibraryOutputProjection::from_plan_with_ownership(
                    LibraryOutputProfile::Romm,
                    projection.destination_root.clone(),
                    &projection.playing_library_plan,
                    Vec::new(),
                    &inspect_library_output_ownership(
                        &projection.playing_library_plan,
                        &self.journal_dir,
                    ),
                ));
                self.retain_romm_projection(projection);
            }
            Err(error) => self.romm_error = Some(error),
        }
    }

    pub(crate) fn preview_retrodeck(&mut self) {
        self.retrodeck_projection = None;
        self.retrodeck_preview_transaction = None;
        self.retrodeck_error = None;
        let (Some(plan), Some(identity)) = (&self.plan, &self.dat_platform_identity) else {
            self.retrodeck_error = Some("build a Playing Library preview first".into());
            return;
        };
        let destination = PathBuf::from(self.retrodeck_destination_root_draft.trim());
        let source = self.source_root_draft_path();
        let visible = PathBuf::from(self.retrodeck_visible_source_root_draft.trim());
        let visibility = if self.retrodeck_visibility_verified && visible == source {
            RetroDeckVisibility::verified_same_path_bind(source, destination.clone())
                .unwrap_or_else(|_| RetroDeckVisibility::unverified(None, None, None, None))
        } else {
            RetroDeckVisibility::unverified(
                Some(source),
                (!visible.as_os_str().is_empty()).then_some(visible),
                Some(destination.clone()),
                None,
            )
        };
        let report = match self.discover_esde_report() {
            Ok(report) => report,
            Err(error) => {
                self.retrodeck_error = Some(error);
                return;
            }
        };
        let Some(platform_id) = identity.platform() else {
            self.retrodeck_error = Some("platform evidence is not resolved".into());
            return;
        };
        let Some(profile) = report.profiles.into_iter().find(|profile| {
            es_de_system_for_platform(platform_id).is_some()
                && profile.system_data.iter().any(|system| {
                    system.system_name
                        == es_de_system_for_platform(platform_id).unwrap().es_de_system
                })
        }) else {
            self.retrodeck_error =
                Some("no configured ES-DE system was found for this verified platform".into());
            return;
        };
        match build_retrodeck_projection(plan, identity, destination, visibility, &profile) {
            Ok(projection) => {
                self.output_projection = Some(LibraryOutputProjection::from_plan_with_ownership(
                    LibraryOutputProfile::RetroDeck,
                    projection.destination_root.clone(),
                    &projection.playing_library_plan,
                    Vec::new(),
                    &inspect_library_output_ownership(
                        &projection.playing_library_plan,
                        &self.journal_dir,
                    ),
                ));
                self.retain_retrodeck_projection(projection);
            }
            Err(error) => self.retrodeck_error = Some(error),
        }
    }

    pub(crate) fn request_retrodeck_apply(&mut self) {
        if self.retrodeck_projection.is_some() {
            self.retrodeck_pending_apply = true;
            self.retrodeck_confirm_text.clear();
        }
    }
    pub(crate) fn cancel_retrodeck_apply(&mut self) {
        self.retrodeck_pending_apply = false;
        self.retrodeck_confirm_text.clear();
    }
    pub(crate) fn confirm_retrodeck_apply(&mut self) {
        if !self.retrodeck_pending_apply {
            return;
        }
        let Some(projection) = &self.retrodeck_projection else {
            return;
        };
        if self.retrodeck_confirm_text.trim()
            != playing_library_confirmation_phrase(projection.total_files)
        {
            self.retrodeck_error = Some("the typed confirmation did not match".into());
            return;
        }
        if self
            .output_projection
            .as_ref()
            .map(|value| value.operation_count)
            != Some(projection.total_files)
        {
            self.retrodeck_error = Some(
                "partial RetroDECK output needs review before more links can be created".into(),
            );
            self.retrodeck_pending_apply = false;
            return;
        }
        let reviewed = match self.retrodeck_preview_transaction.take() {
            Some((preview, transaction)) if preview == *projection => transaction,
            _ => Err("RetroDECK preview is stale or already used; run a fresh Preview".into()),
        };
        let transaction = match reviewed {
            Ok(value) => value,
            Err(error) => {
                self.retrodeck_error = Some(error);
                self.retrodeck_pending_apply = false;
                return;
            }
        };
        self.apply_retrodeck_reviewed(transaction);
    }

    pub(crate) fn request_romm_apply(&mut self) {
        if self.romm_projection.is_some() {
            self.romm_pending_apply = true;
            self.romm_confirm_text.clear();
            self.romm_error = None;
        }
    }

    pub(crate) fn cancel_romm_apply(&mut self) {
        self.romm_pending_apply = false;
        self.romm_confirm_text.clear();
    }

    pub(crate) fn confirm_romm_apply(&mut self) {
        let Some(projection) = &self.romm_projection else {
            return;
        };
        let count = projection.total_files;
        if self.romm_confirm_text.trim() != playing_library_confirmation_phrase(count) {
            self.romm_error = Some("the typed confirmation did not match".to_string());
            return;
        }
        if self
            .output_projection
            .as_ref()
            .map(|value| value.operation_count)
            != Some(count)
        {
            self.romm_error =
                Some("partial RomM output needs review before more links can be created".into());
            self.romm_pending_apply = false;
            return;
        }
        let reviewed = match self.romm_preview_transaction.take() {
            Some((preview, transaction)) if preview == *projection => transaction,
            _ => Err("RomM preview is stale or already used; run a fresh Preview".into()),
        };
        let mut transaction = match reviewed {
            Ok(transaction) => transaction,
            Err(error) => {
                self.romm_error = Some(error);
                self.romm_pending_apply = false;
                return;
            }
        };
        if let Err(error) = std::fs::create_dir_all(&projection.romm_root) {
            self.romm_error = Some(format!("could not create the RomM destination: {error}"));
            return;
        }
        if let Err(error) = write_journal(&self.journal_dir, &transaction) {
            self.romm_error = Some(format!("could not journal the RomM transaction: {error}"));
            return;
        }
        let approved_paths = transaction
            .entries
            .iter()
            .map(|entry| entry.source_path.to_string_lossy().into_owned())
            .collect();
        let trusted =
            TrustedRoots::from_paths([projection.romm_root.clone(), self.source_root_draft_path()]);
        let result = apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths,
            current_generation: self.plan_generation,
            trusted,
            journal_dir: self.journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        });
        self.romm_pending_apply = false;
        self.romm_confirm_text.clear();
        match result {
            Ok(outcome) => {
                if outcome.transaction.state != TransactionState::Applied {
                    self.romm_error = Some(
                        "RomM link creation stopped partway through; review the receipt".into(),
                    );
                }
                self.romm_applied = Some(outcome.transaction);
            }
            Err(error) => self.romm_error = Some(error.to_string()),
        }
    }

    pub(crate) fn rollback_romm_last(&mut self) {
        let Some(transaction) = &mut self.romm_applied else {
            return;
        };
        self.romm_error = incomplete_undo_error(rollback_transaction(
            transaction,
            &self.journal_dir,
            &AtomicBool::new(false),
        ));
    }

    /// Consumes the preview's bound transaction and runs it through the
    /// exact shared `rename_apply` executor - the same journal/apply/
    /// rollback machinery every other apply path in this app uses. No
    /// destination is ever overwritten (the shared preflight's no-clobber
    /// check enforces this the same way it does everywhere else); no
    /// original archive is moved, renamed, deleted, or modified - only a new
    /// symlink object is created at the destination.
    pub(crate) fn confirm_apply(&mut self) {
        let Some(count) = self.pending_apply else {
            return;
        };
        if self.confirm_text.trim() != playing_library_confirmation_phrase(count) {
            self.apply_error = Some("the typed confirmation did not match".to_string());
            return;
        }
        // A confirmed attempt consumes the original source binding even if
        // the fresh collection check refuses it. Retrying requires Preview.
        let reviewed_binding = self.preview_transaction.take();
        self.apply_error = None;
        self.apply_error_is_undo = false;
        self.apply_stage.store(1, Ordering::Relaxed);
        // Re-resolve the catalogue and re-hash the collection on the worker.
        // A changed DAT, source set, or election invalidates the reviewed plan.
        let mut fresh = self.clone();
        fresh.preview();
        if fresh.error.is_some()
            || fresh.plan != self.plan
            || fresh.output_projection != self.output_projection
        {
            self.apply_error =
                Some("the collection or DAT changed; preview is stale; preview again before creating links".into());
            self.pending_apply = None;
            return;
        }
        if self.preview_sources.len() != fresh.preview_sources.len()
            || self.preview_sources.iter().any(|(path, old)| {
                fresh
                    .preview_sources
                    .get(path)
                    .is_none_or(|new| !identity_matches(old, new))
            })
        {
            self.apply_error =
                Some("a source changed since preview; preview again before creating links".into());
            self.pending_apply = None;
            return;
        }
        self.apply_stage.store(2, Ordering::Relaxed);
        let Some(plan) = &self.plan else {
            self.pending_apply = None;
            return;
        };
        // Keep the reviewed identity all the way into the executor's fresh
        // preflight. Rebuilding here would silently approve a replacement.
        // Consume even a refused attempt: a retry needs a new preview, never
        // an overwrite of a prior attempt's authoritative journal.
        let reviewed = match reviewed_binding {
            Some((reviewed_plan, transaction)) if reviewed_plan == *plan => transaction,
            _ => Err("the preview binding is stale or already used; run a fresh preview".into()),
        };
        let mut transaction = match reviewed {
            Ok(transaction) => transaction,
            Err(error) => {
                self.apply_error = Some(error);
                self.pending_apply = None;
                return;
            }
        };
        if let Some(projection) = &self.output_projection {
            transaction.entries.retain(|entry| {
                projection.planned_links.iter().any(|link| {
                    link.source_path == entry.source_path
                        && link.destination_path == entry.destination_path
                })
            });
        }
        if self.destination == PlayingLibraryDestination::Generic {
            transaction.unknown.insert(
                "playing_library_profile".into(),
                serde_json::Value::String("generic".into()),
            );
        }
        if let Err(error) = prepare_destination_root(&plan.destination_root) {
            self.apply_error = Some(format!("could not create the destination folder: {error}"));
            self.pending_apply = None;
            return;
        }
        if let Err(error) = write_journal(&self.journal_dir, &transaction) {
            self.apply_error = Some(format!("could not journal the transaction: {error}"));
            self.pending_apply = None;
            return;
        }
        let approved_paths = transaction
            .entries
            .iter()
            .map(|entry| entry.source_path.to_string_lossy().into_owned())
            .collect();
        let trusted = TrustedRoots::from_paths(
            [plan.destination_root.clone(), self.source_root_draft_path()].iter(),
        );
        let result = apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths,
            current_generation: self.plan_generation,
            trusted,
            journal_dir: self.journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        });
        self.apply_stage.store(4, Ordering::Relaxed);
        self.pending_apply = None;
        self.confirm_text.clear();
        match result {
            Ok(outcome) => {
                // Kept (not dropped) purely so "Publish to ES-DE" - offered
                // only once a library has actually been created - still has
                // the elections that produced it.
                self.applied_plan = self.plan.take();
                self.applied = Some(outcome.transaction);
                if self
                    .applied
                    .as_ref()
                    .is_some_and(|tx| tx.state != TransactionState::Applied)
                {
                    self.apply_error = Some("link creation stopped partway through; review the receipt and use Undo for recorded links".into());
                }
            }
            Err(error) => {
                self.apply_error = Some(error.to_string());
                self.applied = Some(transaction);
            }
        }
    }

    fn bind_preview_sources(&mut self, plan: &PlayingLibraryPlan) -> Result<(), String> {
        let mut bindings = BTreeMap::new();
        for operation in &plan.operations {
            let identity = capture_identity(&operation.source_path)
                .map_err(|error| format!("source changed during preview: {error}"))?;
            if identity.kind != ObjectKind::RegularFile {
                return Err("source is no longer a regular file; preview again".into());
            }
            bindings.insert(operation.source_path.clone(), identity);
        }
        self.preview_sources = bindings;
        Ok(())
    }

    fn source_root_draft_path(&self) -> PathBuf {
        PathBuf::from(self.source_root_draft.trim())
    }

    /// Rolls back the last applied transaction through the exact shared
    /// rollback engine - the same journal-backed path every other rollback
    /// in this app uses.
    pub(crate) fn rollback_last(&mut self) {
        let Some(transaction) = &mut self.applied else {
            return;
        };
        self.apply_error = incomplete_undo_error(rollback_transaction(
            transaction,
            &self.journal_dir,
            &AtomicBool::new(false),
        ));
        self.apply_error_is_undo = self.apply_error.is_some();
    }

    pub(crate) fn load_generic_history_receipt(&mut self, transaction: RenameTransaction) {
        self.destination = PlayingLibraryDestination::Generic;
        self.applied = Some(transaction);
        self.apply_error = None;
        self.apply_error_is_undo = false;
    }

    // --- "Publish to ES-DE" ------------------------------------------------
    //
    // Every filesystem operation below - planning, writing, and recovering
    // a gamelist - goes through `archivefs_core::launch::es_de_publish`'s
    // public API exactly as published: this page never parses or writes
    // ES-DE's XML, and never re-implements its recovery journal.

    pub(crate) fn applied_plan(&self) -> Option<&PlayingLibraryPlan> {
        self.applied_plan.as_ref()
    }

    pub(crate) fn esde_platform_id(&self) -> Option<&'static str> {
        self.esde_platform_id
    }

    pub(crate) fn esde_profile(&self) -> Option<&EsDeProfile> {
        self.esde_profile.as_ref()
    }

    pub(crate) fn esde_discovery_error(&self) -> Option<&str> {
        self.esde_discovery_error.as_deref()
    }

    pub(crate) fn esde_recovery_gamelist_path(&self) -> Option<&Path> {
        self.esde_recovery_gamelist_path.as_deref()
    }

    pub(crate) fn esde_recovery_pending(&self) -> bool {
        self.esde_recovery_pending
    }

    pub(crate) fn esde_recovery_error(&self) -> Option<(&str, Option<&str>)> {
        self.esde_recovery_error
            .as_ref()
            .map(|(friendly, detail)| (friendly.as_str(), detail.as_deref()))
    }

    pub(crate) fn esde_recovery_done(&self) -> bool {
        self.esde_recovery_done
    }

    pub(crate) fn esde_publication(&self) -> Option<&EsDeGamelistPublication> {
        self.esde_publication.as_ref()
    }

    pub(crate) fn esde_preview_error(&self) -> Option<(&str, Option<&str>)> {
        self.esde_preview_error
            .as_ref()
            .map(|(friendly, detail)| (friendly.as_str(), detail.as_deref()))
    }

    pub(crate) fn esde_pending_publish(&self) -> bool {
        self.esde_pending_publish
    }

    pub(crate) fn esde_publish_error(&self) -> Option<(&str, Option<&str>)> {
        self.esde_publish_error
            .as_ref()
            .map(|(friendly, detail)| (friendly.as_str(), detail.as_deref()))
    }

    pub(crate) fn esde_published(&self) -> bool {
        self.esde_published
    }

    pub(crate) fn select_esde_platform(&mut self, platform_id: Option<&'static str>) {
        self.esde_platform_id = platform_id;
        self.esde_publication = None;
        self.esde_preview_error = None;
        self.esde_published = false;
    }

    fn discover_esde_report(&self) -> Result<EsDeEnvironmentReport, String> {
        #[cfg(test)]
        if let Some(home) = &self.esde_home_override {
            use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
            use archivefs_core::emulator_environment::es_de::{
                DiscoveryEnvironment, ExplicitRoot, discover_es_de_environment,
            };
            let environment = DiscoveryEnvironment {
                home: Some(std::ffi::OsString::from(
                    "/nonexistent-home-not-used-by-explicit-profile",
                )),
                path: Some(std::ffi::OsString::from("")),
                explicit_bundled_systems_files: Vec::new(),
                appimage_search_roots: Vec::new(),
                explicit_root: Some(ExplicitRoot {
                    home_directory: home.clone(),
                    executable_path: None,
                }),
                explicit_appimages: Vec::new(),
                explicit_portables: Vec::new(),
            };
            return discover_es_de_environment(&HostReadOnlyFilesystem, &environment)
                .map_err(|error| error.to_string());
        }
        archivefs_core::emulator_environment::es_de::discover_es_de_environment_default()
            .map_err(|error| error.to_string())
    }

    /// Resolves the exact gamelist path for `platform_id` in `profile` - the
    /// same lookup `plan_es_de_gamelist_publication` performs internally.
    /// Needed here only to display the path and to check for an unresolved
    /// recovery record *before* running a real preview; the actual
    /// publication planning, XML handling, and recovery logic still live
    /// exclusively in `archivefs_core::launch::es_de_publish`.
    fn resolve_esde_gamelist_path(profile: &EsDeProfile, platform_id: &str) -> Option<PathBuf> {
        let mapping = es_de_system_for_platform(platform_id)?;
        let locations = profile
            .system_data
            .iter()
            .find(|entry| entry.system_name == mapping.es_de_system)?;
        if locations.gamelist_file.path.lossy {
            return None;
        }
        Some(PathBuf::from(&locations.gamelist_file.path.display))
    }

    /// Detects the user's real ES-DE installation, then previews exactly
    /// what publishing the already-created playing library would change.
    /// Read-only: discovery only probes paths, and
    /// `plan_es_de_gamelist_publication` performs at most one bounded read
    /// of the existing gamelist - nothing is written.
    pub(crate) fn preview_esde_publication(&mut self) {
        self.esde_discovery_error = None;
        self.esde_recovery_gamelist_path = None;
        self.esde_recovery_error = None;
        self.esde_recovery_done = false;
        self.esde_publication = None;
        self.esde_preview_error = None;
        self.esde_published = false;

        let Some(platform_id) = self.esde_platform_id else {
            self.esde_preview_error = Some(("choose a platform first".to_string(), None));
            return;
        };
        let Some(plan) = &self.applied_plan else {
            self.esde_preview_error = Some(("create the playing library first".to_string(), None));
            return;
        };

        let report = match self.discover_esde_report() {
            Ok(report) => report,
            Err(error) => {
                self.esde_discovery_error =
                    format!("EmuWiz could not look for an ES-DE installation: {error}").into();
                return;
            }
        };
        // A profile's own `eligible` flag answers "would ES-DE itself
        // launch from here" (a valid executable + config root) - not what
        // matters for editing a gamelist, which only needs a real,
        // already-configured system directory. So this picks the first
        // profile that actually knows about `platform_id`'s ES-DE system,
        // via the exact same lookup `plan_es_de_gamelist_publication`
        // performs internally.
        let Some(profile) = report
            .profiles
            .into_iter()
            .find(|profile| Self::resolve_esde_gamelist_path(profile, platform_id).is_some())
        else {
            self.esde_discovery_error = Some(
                "No ES-DE installation with this platform already configured was found on this \
                 computer."
                    .to_string(),
            );
            return;
        };

        if let Some(gamelist_path) = Self::resolve_esde_gamelist_path(&profile, platform_id)
            && has_unresolved_es_de_gamelist_recovery(&gamelist_path)
        {
            self.esde_recovery_gamelist_path = Some(gamelist_path);
            self.esde_profile = Some(profile);
            return;
        }

        match plan_es_de_gamelist_publication(plan, platform_id, &profile) {
            Ok(publication) => {
                self.esde_publication = Some(publication);
                self.esde_profile = Some(profile);
            }
            Err(error) => {
                self.esde_preview_error =
                    Some((esde_friendly_error(&error), Some(error.to_string())));
            }
        }
    }

    pub(crate) fn request_esde_publish(&mut self) {
        if self.esde_publication.is_some() {
            self.esde_pending_publish = true;
        }
    }

    pub(crate) fn cancel_esde_publish(&mut self) {
        self.esde_pending_publish = false;
    }

    /// Applies exactly the previewed publication - never a re-plan, never a
    /// fresh discovery - through
    /// `archivefs_core::launch::es_de_publish::apply_es_de_gamelist_publication`,
    /// the same durable, recovery-journaled write every other caller of
    /// that function uses.
    pub(crate) fn confirm_esde_publish(&mut self) {
        self.esde_pending_publish = false;
        let Some(publication) = &self.esde_publication else {
            return;
        };
        match apply_es_de_gamelist_publication(publication) {
            Ok(()) => {
                self.esde_published = true;
                self.esde_publish_error = None;
            }
            Err(error) => {
                self.esde_publish_error =
                    Some((esde_friendly_error(&error), Some(error.to_string())));
            }
        }
    }

    pub(crate) fn request_esde_recovery(&mut self) {
        if self.esde_recovery_gamelist_path.is_some() {
            self.esde_recovery_pending = true;
        }
    }

    pub(crate) fn cancel_esde_recovery(&mut self) {
        self.esde_recovery_pending = false;
    }

    /// Restores ES-DE's previous gamelist exactly through
    /// `recover_es_de_gamelist_publication`, or reports its refusal when the
    /// current file no longer matches the recorded state - never touches master ROMs or
    /// playing-library links, which live entirely outside that function's
    /// reach.
    pub(crate) fn confirm_esde_recovery(&mut self) {
        self.esde_recovery_pending = false;
        let Some(gamelist_path) = self.esde_recovery_gamelist_path.take() else {
            return;
        };
        match recover_es_de_gamelist_publication(&gamelist_path) {
            Ok(()) => self.esde_recovery_done = true,
            Err(error) => {
                self.esde_recovery_error =
                    Some((esde_friendly_error(&error), Some(error.to_string())));
                // A refused recovery keeps its evidence; keep offering it
                // rather than hiding a record that is still unresolved.
                if has_unresolved_es_de_gamelist_recovery(&gamelist_path) {
                    self.esde_recovery_gamelist_path = Some(gamelist_path);
                }
            }
        }
    }
}

/// Beginner-facing, non-technical wording for every
/// `EsDePublicationError` variant - the raw `Display` text (which names
/// paths, byte counts, and internal field names) is never shown here; it
/// remains available only behind an expandable technical-details section.
fn esde_friendly_error(error: &EsDePublicationError) -> String {
    match error {
        EsDePublicationError::PlatformUnmapped { .. } => {
            "EmuWiz does not yet know how to publish this platform to ES-DE.".to_string()
        }
        EsDePublicationError::SystemNotConfigured { .. } => {
            "This platform is not set up in your ES-DE installation yet.".to_string()
        }
        EsDePublicationError::UnsupportedPathEncoding { .. } => {
            "ES-DE's game list uses a file path EmuWiz cannot safely handle.".to_string()
        }
        EsDePublicationError::GamelistUnreadable { .. } => {
            "EmuWiz could not read ES-DE's existing game list.".to_string()
        }
        EsDePublicationError::GamelistTooLarge { .. } => {
            "ES-DE's existing game list is too large for EmuWiz to safely update.".to_string()
        }
        EsDePublicationError::MalformedGamelist { .. } => "ES-DE's existing game list does not \
             look like a game list EmuWiz recognises, so nothing was changed."
            .to_string(),
        EsDePublicationError::NothingToPublish => "ES-DE is already up to date.".to_string(),
        EsDePublicationError::UnresolvedRecovery { .. } => "A previous ES-DE update did not \
             finish. Restore it before publishing again."
            .to_string(),
        EsDePublicationError::NoRecoveryRecord { .. } => "There is nothing to restore.".to_string(),
        EsDePublicationError::RecoveryCorrupt { .. } => "EmuWiz could not safely read the \
             recovery information for ES-DE, so nothing was changed."
            .to_string(),
        EsDePublicationError::RecoveryTooLarge { .. } => {
            "The recovery information for ES-DE is too large for EmuWiz to safely read.".to_string()
        }
        EsDePublicationError::RecoveryPathMismatch { .. } => "EmuWiz found unexpected recovery \
             information and refused to use it, to protect your files."
            .to_string(),
        EsDePublicationError::Io { .. } => {
            "EmuWiz could not update ES-DE because of a filesystem error.".to_string()
        }
    }
}

/// The platforms `archivefs_core::launch::es_de_export::es_de_system_for_platform`
/// already knows how to map - the only ones ES-DE publication ever offers,
/// since anything else would fail closed anyway.
fn esde_platform_options() -> Vec<&'static str> {
    ES_DE_SYSTEM_MAP
        .iter()
        .map(|mapping| mapping.platform_id)
        .collect()
}

pub(crate) enum PlayingLibraryPageAction {
    Preview,
    SelectFamily(Option<String>),
    OverrideCandidate { family: String, source: PathBuf },
    ClearOverrides,
    RequestApply,
    CancelApply,
    ConfirmApply,
    RollbackLast,
    SelectEsdePlatform(&'static str),
    PreviewEsde,
    RequestEsdePublish,
    CancelEsdePublish,
    ConfirmEsdePublish,
    RequestEsdeRecovery,
    CancelEsdeRecovery,
    ConfirmEsdeRecovery,
    PreviewRomm,
    RollbackRomm,
    PreviewRetroDeck,
    RollbackRetroDeck,
}

/// Stable, absolute widget ids for the text fields a test needs to drive
/// through real keyboard focus + `egui::Event::Text` (the same
/// `ctx.memory_mut(|memory| memory.request_focus(id))` pattern this crate's
/// own `set_text_edit_caret`/`apply_select_all` already use) rather than by
/// mutating page state directly.
#[cfg(test)]
pub(crate) const DAT_PATH_FIELD_ID: &str = "playing_library_dat_path_field";
pub(crate) const SOURCE_ROOT_FIELD_ID: &str = "playing_library_source_root_field";
pub(crate) const DESTINATION_ROOT_FIELD_ID: &str = "playing_library_destination_root_field";
pub(crate) const PREFERRED_REGIONS_FIELD_ID: &str = "playing_library_preferred_regions_field";
pub(crate) const PREFERRED_LANGUAGES_FIELD_ID: &str = "playing_library_preferred_languages_field";

/// Renders the Build Playing Library flow. Returns the action the caller
/// (the Library Organisation page) should apply to this state on the next
/// frame for the higher-level operations (Preview, apply, rollback, ...) -
/// the same "render describes, caller mutates" split every other page in
/// this app follows for those. Text fields and checkboxes are simple enough
/// state that this function mutates them in place immediately, exactly like
/// `rom_organisation_page::show_rom_organisation_page` already does for its
/// own `master_root_draft`/`library_root_draft`/`confirm_text`.
pub(crate) fn show_playing_library_page(
    ui: &mut egui::Ui,
    state: &mut PlayingLibraryPageState,
) -> Option<PlayingLibraryPageAction> {
    show_playing_library_page_with_busy(ui, state, false)
}

pub(crate) fn show_playing_library_page_with_busy(
    ui: &mut egui::Ui,
    state: &mut PlayingLibraryPageState,
    async_busy: bool,
) -> Option<PlayingLibraryPageAction> {
    state.async_busy = async_busy;
    let mut action = None;
    let simple = crate::simple_mode::active(ui.ctx());

    let title = match state.destination {
        PlayingLibraryDestination::Generic => {
            if simple {
                "Your playing library"
            } else {
                "Playing Library"
            }
        }
        PlayingLibraryDestination::Romm => "RomM Library",
        PlayingLibraryDestination::EsDe => "ES-DE Library",
        PlayingLibraryDestination::RetroDeck => "RetroDECK Library",
    };
    widgets::section_header(ui, title, None);
    ui.label(
        egui::RichText::new(
            "Pick one representative release per game and create a linked library of it. \
             Your original files are never moved, renamed, or changed.",
        )
        .color(theme::muted(ui)),
    );
    let destination_help = match state.destination {
        PlayingLibraryDestination::Generic => {
            "A clean linked library without frontend-specific folders."
        }
        PlayingLibraryDestination::Romm => {
            "The RomM preview supplies the reviewed system folder automatically."
        }
        PlayingLibraryDestination::EsDe => {
            "The existing ES-DE mapping supplies the reviewed system folder automatically."
        }
        PlayingLibraryDestination::RetroDeck => {
            "The existing ES-DE system mapping supplies RetroDECK's reviewed ROM folder."
        }
    };
    ui.label(egui::RichText::new(destination_help).color(theme::muted(ui)));
    ui.add_space(8.0);

    // Keep the next action in the first viewport on compact displays. The
    // detailed output card below remains the authoritative review surface.
    let inputs_ready = state.has_catalogue_selection()
        && !state.source_root_draft.trim().is_empty()
        && !state.destination_root_draft.trim().is_empty();
    let compact_profile_action =
        state
            .output_projection
            .as_ref()
            .and_then(|projection| match state.destination {
                PlayingLibraryDestination::Romm => Some((
                    projection.operation_count,
                    projection.conflicts.is_empty()
                        && state.romm_projection.as_ref().is_some_and(|value| {
                            value.visibility.is_verified()
                                && value.total_files == projection.operation_count
                        }),
                    state.romm_pending_apply,
                    state.romm_applied.is_some(),
                )),
                PlayingLibraryDestination::RetroDeck => Some((
                    projection.operation_count,
                    projection.conflicts.is_empty()
                        && state.retrodeck_projection.as_ref().is_some_and(|value| {
                            value.visibility.is_verified()
                                && value.total_files == projection.operation_count
                        }),
                    state.retrodeck_pending_apply,
                    state.retrodeck_applied.is_some(),
                )),
                _ => None,
            });
    ui.horizontal_wrapped(|ui| {
        if widgets::action_button(
            ui,
            "Preview playing library",
            widgets::ActionStyle::Primary,
            inputs_ready && !async_busy,
        )
        .clicked()
        {
            action = Some(PlayingLibraryPageAction::Preview);
        }
        if matches!(
            state.destination,
            PlayingLibraryDestination::Generic | PlayingLibraryDestination::EsDe
        ) && state.output_projection.as_ref().is_some_and(|projection| {
            projection.operation_count > 0 && projection.conflicts.is_empty()
        }) && !async_busy
        {
            if state.pending_apply.is_none() {
                if widgets::action_button(
                    ui,
                    "Create reviewed links",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
                {
                    action = Some(PlayingLibraryPageAction::RequestApply);
                }
            } else {
                let count = state.pending_apply.unwrap_or(0);
                ui.label(format!(
                    "Type {}",
                    playing_library_confirmation_phrase(count)
                ));
                ui.add(egui::TextEdit::singleline(&mut state.confirm_text).desired_width(210.0));
                if widgets::action_button(
                    ui,
                    "Confirm links",
                    widgets::ActionStyle::Destructive,
                    true,
                )
                .clicked()
                {
                    action = Some(PlayingLibraryPageAction::ConfirmApply);
                }
            }
        }
        if let Some((count, ready, pending, applied)) = compact_profile_action
            && count > 0
            && !applied
        {
            if !pending {
                if widgets::action_button(
                    ui,
                    "Apply output profile",
                    widgets::ActionStyle::Primary,
                    ready && !async_busy,
                )
                .clicked()
                {
                    action = Some(PlayingLibraryPageAction::RequestApply);
                }
            } else {
                let required = if state.destination == PlayingLibraryDestination::Romm {
                    state
                        .romm_projection
                        .as_ref()
                        .map_or(count, |value| value.total_files)
                } else {
                    state
                        .retrodeck_projection
                        .as_ref()
                        .map_or(count, |value| value.total_files)
                };
                ui.label(format!(
                    "Type {}",
                    playing_library_confirmation_phrase(required)
                ));
                let confirmation = if state.destination == PlayingLibraryDestination::Romm {
                    &mut state.romm_confirm_text
                } else {
                    &mut state.retrodeck_confirm_text
                };
                ui.add(egui::TextEdit::singleline(confirmation).desired_width(210.0));
                if widgets::action_button(
                    ui,
                    "Confirm",
                    widgets::ActionStyle::Destructive,
                    !async_busy,
                )
                .clicked()
                {
                    action = Some(PlayingLibraryPageAction::ConfirmApply);
                }
            }
        }
    });
    if !state.candidate_overrides.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} individual choice(s) active",
                state.candidate_overrides.len()
            ));
            if ui.button("Clear individual choices").clicked() {
                action = Some(PlayingLibraryPageAction::ClearOverrides);
            }
        });
    }

    widgets::section_header(
        ui,
        "Inputs",
        Some(if crate::simple_mode::active(ui.ctx()) {
            "Choose a platform and the folders to use. Next you will preview the result before approving any changes."
        } else {
            "Choose the catalogue and the folders used for this preview."
        }),
    );
    widgets::card(ui, |ui| {
        if state.catalogue_picker.poll() || state.catalogue_picker.loading {
            ui.ctx().request_repaint();
        }
        let _ = state.catalogue_picker.show(
            ui,
            DatCatalogueWorkflow::PlayingLibrary,
            &mut state.selected_catalogue,
        );
        // Keep the historical render harness able to exercise its stable
        // widget ID while production uses the typed picker exclusively.
        #[cfg(test)]
        ui.add(
            egui::TextEdit::singleline(&mut state.dat_path_draft)
                .id(egui::Id::new(DAT_PATH_FIELD_ID))
                .desired_width(280.0),
        );
        ui.add_space(6.0);

        ui.label(
            egui::RichText::new(if simple {
                "Your games folder"
            } else {
                "Source library"
            })
            .strong(),
        );
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.source_root_draft)
                    .id(egui::Id::new(SOURCE_ROOT_FIELD_ID))
                    .desired_width(ui.available_width().max(240.0) - 90.0),
            );
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose Source Library Folder")
                    .pick_folder()
            {
                state.source_root_draft = path.display().to_string();
            }
        });
        if path_looks_missing(&state.source_root_draft, true) {
            ui.label(egui::RichText::new(if simple { "This games folder is unavailable. Connect its drive or use Browse to choose another before previewing. Your games have not changed." } else { "This folder was not found." }).color(theme::warning(ui)));
        }
        ui.add_space(6.0);

        let destination_label = match state.destination {
            PlayingLibraryDestination::Generic => "Playing Library destination",
            PlayingLibraryDestination::Romm => "RomM library destination root",
            PlayingLibraryDestination::EsDe => "ES-DE library destination root",
            PlayingLibraryDestination::RetroDeck => "RetroDECK library destination root",
        };
        ui.label(egui::RichText::new(destination_label).strong());
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.destination_root_draft)
                    .id(egui::Id::new(DESTINATION_ROOT_FIELD_ID))
                    .desired_width(ui.available_width().max(240.0) - 90.0),
            );
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title(match state.destination {
                        PlayingLibraryDestination::Generic => {
                            "Choose Playing Library Destination Folder"
                        }
                        PlayingLibraryDestination::Romm => "Choose RomM Library Folder",
                        PlayingLibraryDestination::EsDe => "Choose ES-DE Library Folder",
                        PlayingLibraryDestination::RetroDeck => "Choose RetroDECK Library Folder",
                    })
                    .pick_folder()
            {
                state.destination_root_draft = path.display().to_string();
            }
        });
        ui.label(
            egui::RichText::new("Created automatically if it does not exist yet.")
                .color(theme::muted(ui))
                .small(),
        );
    });

    ui.add_space(8.0);
    let mut show_preferences = |ui: &mut egui::Ui| {
        widgets::section_header(
            ui,
            "Preferences",
            Some("Set the existing 1G1R election preferences."),
        );
        ui.label(
        egui::RichText::new(
            "1G1R means 1 Game, 1 ROM: EmuWiz picks one preferred verified release for each game. These are the 1G1R selection rules.",
        )
        .color(theme::muted(ui))
        .small(),
    );
        widgets::card(ui, |ui| {
            ui.label("Region order (most preferred first):");
            ui.add(
                egui::TextEdit::singleline(&mut state.preferred_regions_draft)
                    .id(egui::Id::new(PREFERRED_REGIONS_FIELD_ID))
                    .hint_text("Europe, USA, Japan, Other"),
            );
            ui.add_space(6.0);

            ui.label("Preferred languages:");
            ui.add(
                egui::TextEdit::singleline(&mut state.preferred_languages_draft)
                    .id(egui::Id::new(PREFERRED_LANGUAGES_FIELD_ID))
                    .hint_text("English"),
            );
            ui.add_space(6.0);

            ui.checkbox(
                &mut state.prefer_newest_revision,
                "Prefer newest verified revision",
            );
            ui.checkbox(&mut state.prefer_parent, "Prefer declared parent");
            ui.add_space(6.0);

            ui.label("Exclude:");
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut state.exclude_beta, "Beta");
                ui.checkbox(&mut state.exclude_proto, "Proto");
                ui.checkbox(&mut state.exclude_demo, "Demo");
                ui.checkbox(&mut state.exclude_sample, "Sample");
            });
        });
    };
    if simple {
        ui.collapsing(
            "Advanced details — which game versions to prefer",
            show_preferences,
        );
    } else {
        show_preferences(ui);
    }

    ui.add_space(8.0);
    let ready = state.has_catalogue_selection()
        && !state.source_root_draft.trim().is_empty()
        && !state.destination_root_draft.trim().is_empty();
    widgets::section_header(ui, "Plan and preview", None);
    if widgets::action_button(
        ui,
        if simple {
            "Preview playing library"
        } else {
            "Preview playing library"
        },
        widgets::ActionStyle::Primary,
        ready && !async_busy,
    )
    .clicked()
    {
        action = Some(PlayingLibraryPageAction::Preview);
    }
    if async_busy {
        let progress = match state.apply_stage.load(Ordering::Relaxed) {
            1 => "Checking DAT and source identities…",
            2 => "Preparing and journaling links…",
            3 => "Creating links with a durable journal…",
            4 => "Finalising the link receipt…",
            _ => "Working in the background… You can keep browsing.",
        };
        ui.label(progress);
    }
    if !ready {
        ui.label(
            egui::RichText::new(if crate::simple_mode::active(ui.ctx()) { "Choose verification data, your games folder, and a new output folder above to prepare the preview." } else { "Choose a DAT catalogue, a source, and a destination first." })
                .color(theme::muted(ui))
                .small(),
        );
    }

    if let Some(error) = state.error() {
        ui.add_space(6.0);
        widgets::banner(
            ui,
            "Could not build a preview",
            if simple {
                "The preview could not be completed. No links were created by this preview. Check the selected verification data and folders above, then try Preview playing library again."
            } else {
                error
            },
            widgets::StatusTone::Blocked,
        );
        if simple {
            widgets::technical_details(ui, "simple-library-preview-error", |ui| {
                ui.label(error);
            });
        }
    }

    if state.plan().is_some() {
        ui.add_space(10.0);
        widgets::section_header(
            ui,
            "Review Playing Library",
            Some("Preview reads only. Original ROMs will remain unchanged."),
        );
        ui.horizontal_wrapped(|ui| {
            for filter in PreviewFilter::ALL {
                if ui
                    .selectable_value(&mut state.preview_filter, filter, filter.label())
                    .changed()
                {
                    state.preview_page = 0;
                }
            }
        });
        let total = state
            .plan()
            .map(|plan| {
                presentation::ReviewCounts::from_state(plan, state).visible(state.preview_filter)
            })
            .unwrap_or(0);
        let pages = total.div_ceil(presentation::ROW_LIMIT).max(1);
        let page = state.preview_page.min(pages - 1);
        if pages > 1 {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(page > 0, egui::Button::new("Previous review page"))
                    .clicked()
                {
                    state.preview_page = page - 1;
                }
                ui.label(format!("Review page {} of {pages}", page + 1));
                if ui
                    .add_enabled(page + 1 < pages, egui::Button::new("Next review page"))
                    .clicked()
                {
                    state.preview_page = page + 1;
                }
            });
        }
        if let Some(plan) = state.plan() {
            show_preview_summary(ui, plan, state, &mut action);
        }
        widgets::section_header(
            ui,
            "Create reviewed output",
            Some(
                "Creates separate links · Originals preserved · Review and confirm before Apply · Undo is offered only for links that are recorded",
            ),
        );
        if let Some(projection) = state.output_projection.clone() {
            show_output_projection_summary(ui, &projection, state, &mut action);
        } else {
            ui.label("This plan is preview-only until an output profile preview and its required checks are complete. Apply is not available yet.");
        }
        ui.add_space(10.0);
        if matches!(state.destination, PlayingLibraryDestination::Romm) {
            show_romm_projection_summary(ui, state, &mut action);
        }
        // Keep the historical ES-DE choice wired to the same safe
        // RetroDECK filesystem projection for compatibility with existing
        // saved workflows; RetroDECK now also has an explicit profile card.
        if matches!(
            state.destination,
            PlayingLibraryDestination::EsDe | PlayingLibraryDestination::RetroDeck
        ) {
            show_retrodeck_projection_summary(ui, state, &mut action);
        }
    }

    if matches!(
        state.destination,
        PlayingLibraryDestination::RetroDeck | PlayingLibraryDestination::EsDe
    ) {
        retrodeck::show_operations(ui, state, &mut action);
    }

    if let Some(transaction) = state.applied() {
        ui.add_space(10.0);
        widgets::card(ui, |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Receipt {} · {} of {} link(s) recorded · {:?}",
                    transaction.transaction_id,
                    transaction.applied_count(),
                    transaction.entries.len(),
                    transaction.state
                ))
                .strong(),
            );
            ui.label("Original ROM data copied: 0 bytes. Link metadata uses filesystem space.");
            if transaction.state == TransactionState::RolledBack {
                let retained = transaction
                    .entries
                    .iter()
                    .filter(|entry| entry.unknown.contains_key("emuwiz_link_undo_tombstone_v1"))
                    .count();
                if retained > 0 {
                    ui.label(format!("Undo moved {retained} link(s) out of the Playing Library. Hidden link tombstones remain for safety; originals are unchanged."));
                }
            }
            ui.label("this receipt is not current collection evidence");
            widgets::technical_details(ui, "playing-library-receipt-entries", |ui| {
                for entry in &transaction.entries {
                    ui.label(format!(
                        "{} · {}{}",
                        entry.state.label(),
                        entry.destination_path.display(),
                        entry
                            .failure_reason
                            .as_deref()
                            .map(|reason| format!(" · {reason}"))
                            .unwrap_or_default()
                    ));
                }
            });
            if transaction.applied_count() > 0
                && transaction.state != TransactionState::RolledBack
                && widgets::action_button(ui, "Undo", widgets::ActionStyle::Quiet, true).clicked()
            {
                action = Some(PlayingLibraryPageAction::RollbackLast);
            }
        });
    }

    if let Some(error) = state.apply_error() {
        ui.add_space(6.0);
        let (title, simple_body) = apply_error_heading(state.applied(), state.apply_error_is_undo);
        widgets::banner(
            ui,
            title,
            if simple { simple_body } else { error },
            widgets::StatusTone::Blocked,
        );
        if simple {
            widgets::technical_details(ui, "simple-library-apply-error", |ui| {
                ui.label(error);
            });
        }
    }

    // Only offered once the links this preview describes actually exist -
    // never while nothing has been applied yet, and never again once a
    // rollback has removed them.
    if matches!(
        state.applied(),
        Some(transaction) if transaction.state == TransactionState::Applied
    ) {
        ui.add_space(10.0);
        show_esde_publish_section(ui, state, &mut action);
    }

    action
}

/// Presents every output profile through one read-only summary model. The
/// profile-specific cards below remain only for visibility acknowledgements,
/// ES-DE publication, and rollback details that have no common equivalent.
fn show_output_projection_summary(
    ui: &mut egui::Ui,
    projection: &LibraryOutputProjection,
    state: &mut PlayingLibraryPageState,
    action: &mut Option<PlayingLibraryPageAction>,
) {
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Output profile preview", None);
        ui.label(format!("Profile: {}", projection.profile.label()));
        ui.label(format!(
            "Destination: {}",
            projection.destination_root.display()
        ));
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                format!("{} planned link(s)", projection.operation_count),
                widgets::StatusTone::Info,
            );
            widgets::status_badge(
                ui,
                format!("{} directory(ies)", projection.planned_directories.len()),
                widgets::StatusTone::Info,
            );
            widgets::status_badge(
                ui,
                format!("{} already correct", projection.already_correct),
                widgets::StatusTone::Success,
            );
            widgets::status_badge(
                ui,
                format!("{} conflict(s)", projection.conflicts.len()),
                if projection.conflicts.is_empty() {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Blocked
                },
            );
        });
        ui.label("ROM data to copy: 0 bytes. Links use a small amount of filesystem metadata.");
        if !projection.unresolved_mappings.is_empty() {
            ui.label(
                egui::RichText::new("Some platform mappings need review.")
                    .color(theme::warning(ui)),
            );
            for mapping in &projection.unresolved_mappings {
                ui.label(format!("- {mapping}"));
            }
        }
        if projection.stale_owned_entries.is_empty() {
            ui.label("Stale owned entries: none reported by this preview.");
        } else {
            ui.label(format!(
                "Stale owned entries: {} (explicit cleanup required)",
                projection.stale_owned_entries.len()
            ));
            widgets::technical_details(ui, "playing_library_stale_owned_entries", |ui| {
                for path in &projection.stale_owned_entries {
                    ui.label(path.display().to_string());
                }
            });
        }
        match projection.profile {
            LibraryOutputProfile::Generic => {
                ui.label("Layout: <destination>/<platform>/<game file>");
            }
            LibraryOutputProfile::Romm => {
                if let Some(romm) = &state.romm_projection {
                    ui.label(format!("RomM platform slug: {}", romm.romm_platform_slug));
                }
            }
            LibraryOutputProfile::EsDe => {
                ui.label("ES-DE system metadata remains an explicit publication step.");
            }
            LibraryOutputProfile::RetroDeck => {
                if let Some(retrodeck) = &state.retrodeck_projection {
                    ui.label(format!(
                        "RetroDECK ROM directory: {}",
                        retrodeck.es_de_system
                    ));
                }
            }
        }
        if projection.sample_paths.is_empty() {
            ui.label("Sample output paths: none");
        } else {
            widgets::technical_details(ui, "playing_library_output_sample_paths", |ui| {
                for path in &projection.sample_paths {
                    ui.label(path.display().to_string());
                }
            });
        }
        if !projection.conflicts.is_empty() {
            ui.colored_label(theme::warning(ui), "Apply unavailable: the destination has conflicts. Review them; no force overwrite is offered.");
            widgets::technical_details(ui, "playing_library_destination_conflicts", |ui| {
                for conflict in projection.conflicts.iter().take(presentation::ROW_LIMIT) {
                    ui.label(format!("{conflict:?}"));
                }
            });
        } else if projection.operation_count == 0 {
            ui.label(if projection.already_correct > 0 {
                "Playing Library already up to date for the reviewed entries. No links need creating."
            } else {
                "Apply unavailable: no eligible output operations in this preview."
            });
        }
        let pending = match projection.profile {
            LibraryOutputProfile::Romm => state.romm_pending_apply,
            LibraryOutputProfile::RetroDeck => state.retrodeck_pending_apply,
            LibraryOutputProfile::Generic | LibraryOutputProfile::EsDe => {
                state.pending_apply.is_some()
            }
        };
        let applied = match projection.profile {
            LibraryOutputProfile::Romm => state.romm_applied.is_some(),
            LibraryOutputProfile::RetroDeck => state.retrodeck_applied.is_some(),
            LibraryOutputProfile::Generic | LibraryOutputProfile::EsDe => false,
        };
        if !state.async_busy
            && !pending
            && !applied
            && projection.operation_count > 0
            && projection.conflicts.is_empty()
        {
            let visibility_ok = match projection.profile {
                LibraryOutputProfile::Romm => state.romm_projection.as_ref().is_some_and(|value| {
                    value.visibility.is_verified()
                        && value.total_files == projection.operation_count
                }),
                LibraryOutputProfile::RetroDeck => {
                    state.retrodeck_projection.as_ref().is_some_and(|value| {
                        value.visibility.is_verified()
                            && value.total_files == projection.operation_count
                    })
                }
                LibraryOutputProfile::Generic | LibraryOutputProfile::EsDe => true,
            };
            if widgets::action_button(
                ui,
                "Apply output profile",
                widgets::ActionStyle::Primary,
                visibility_ok,
            )
            .clicked()
            {
                *action = Some(PlayingLibraryPageAction::RequestApply);
            }
            if !visibility_ok {
                ui.label(
                    egui::RichText::new(
                        if matches!(projection.profile, LibraryOutputProfile::Romm | LibraryOutputProfile::RetroDeck)
                            && projection.already_correct > 0 {
                            "Apply is blocked: partial existing profile output needs review. Undo its receipt or use a fresh destination."
                        } else {
                            "Apply is blocked until the profile's source visibility is explicitly verified."
                        },
                    )
                    .color(theme::warning(ui)),
                );
            }
        }
        if pending {
            if state.async_busy {
                ui.label("Building your playing library in the background…");
            }
            let count = match projection.profile {
                LibraryOutputProfile::Romm => state
                    .romm_projection
                    .as_ref()
                    .map_or(projection.operation_count, |value| value.total_files),
                LibraryOutputProfile::RetroDeck => state
                    .retrodeck_projection
                    .as_ref()
                    .map_or(projection.operation_count, |value| value.total_files),
                _ => projection.operation_count,
            };
            {
                let confirmation = match projection.profile {
                    LibraryOutputProfile::Romm => &mut state.romm_confirm_text,
                    LibraryOutputProfile::RetroDeck => &mut state.retrodeck_confirm_text,
                    LibraryOutputProfile::Generic | LibraryOutputProfile::EsDe => {
                        &mut state.confirm_text
                    }
                };
                ui.label(format!(
                    "Type \"{}\" to confirm:",
                    playing_library_confirmation_phrase(count)
                ));
                ui.add(
                    egui::TextEdit::singleline(confirmation)
                        .desired_width(260.0)
                        .hint_text(playing_library_confirmation_phrase(count)),
                );
            }
            ui.horizontal_wrapped(|ui| {
                if widgets::action_button(
                    ui,
                    "Confirm",
                    widgets::ActionStyle::Destructive,
                    !state.async_busy,
                )
                .clicked()
                {
                    *action = Some(PlayingLibraryPageAction::ConfirmApply);
                }
                if widgets::action_button(
                    ui,
                    "Cancel",
                    widgets::ActionStyle::Quiet,
                    !state.async_busy,
                )
                .clicked()
                {
                    *action = Some(PlayingLibraryPageAction::CancelApply);
                }
            });
        }
    });
}

fn show_romm_projection_summary(
    ui: &mut egui::Ui,
    state: &mut PlayingLibraryPageState,
    action: &mut Option<PlayingLibraryPageAction>,
) {
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Build RomM Library", None);
        ui.label(
            egui::RichText::new(
                "Create a separate RomM-readable symlink library. Source files are never moved, renamed, or copied.",
            )
            .color(theme::muted(ui)),
        );
        if state.romm_projection.is_none()
            && widgets::action_button(
                ui,
                "Preview RomM Library",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
        {
            *action = Some(PlayingLibraryPageAction::PreviewRomm);
        }
        if let Some(error) = &state.romm_error {
            ui.label(egui::RichText::new(error).color(theme::warning(ui)));
        }
        let visibility_changed = ui
            .checkbox(
                &mut state.romm_visibility_verified,
                "I have verified RomM sees the source root at the same absolute path",
            )
            .changed();
        if visibility_changed {
            state.romm_projection = None;
        }
        if !state.romm_visibility_verified {
            ui.label(
                egui::RichText::new(
                    "Needs setup — RomM cannot currently see this source path safely. Apply is blocked.",
                )
                .color(theme::warning(ui)),
            );
        } else {
            ui.label(
                egui::RichText::new("Ready — RomM path visibility has been verified.")
                    .color(theme::success(ui)),
            );
        }
        ui.horizontal(|ui| {
            ui.label("RomM-visible source root:");
            ui.add(
                egui::TextEdit::singleline(&mut state.romm_visible_source_root_draft)
                    .desired_width(300.0)
                    .hint_text("e.g. /mnt/usbdrive/games"),
            );
        });
        if let Some(projection) = &state.romm_projection {
            ui.label(format!("Destination: {}", projection.romm_root.display()));
            ui.label(format!(
                "{} game(s), {} file(s), reviewed RomM platform `{}`",
                projection.games.len(),
                projection.total_files,
                projection.romm_platform_slug
            ));
            ui.label(format!(
                "{} election exclusion(s), {} unresolved, {} refused multi-file release(s)",
                projection.excluded_elections,
                projection.unresolved_elections,
                projection.rejected_launchers
            ));
            let companion_count: usize = projection
                .games
                .iter()
                .map(|game| game.companions.len())
                .sum();
            ui.label(format!(
                "{companion_count} companion file(s) kept with their launchers"
            ));
            ui.label(if projection.visibility.is_verified() {
                "Visibility: Ready"
            } else {
                "Visibility: Needs setup"
            });
            match &projection.visibility {
                RommVisibility::VerifiedVisible {
                    host_root,
                    romm_root,
                }
                | RommVisibility::Unverified {
                    host_root: Some(host_root),
                    romm_root: Some(romm_root),
                } => {
                    ui.label(format!(
                        "Link targets: host {} -> RomM {}",
                        host_root.display(),
                        romm_root.display()
                    ));
                }
                _ => {
                    ui.label("Link targets: no verified host/container mapping");
                }
            }
            if let Some(transaction) = &state.romm_applied {
                if transaction.is_fully_rolled_back() {
                    ui.label(
                        egui::RichText::new("RomM library rolled back; no generated links remain.")
                            .color(theme::success(ui)),
                    );
                } else {
                    if transaction.state == TransactionState::Applied {
                        ui.label(
                            egui::RichText::new(format!(
                                "RomM library created: {} link(s)",
                                transaction.applied_count()
                            ))
                            .color(theme::success(ui)),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(format!(
                                "Recovery needs review: {}",
                                transaction.state.label()
                            ))
                            .color(theme::warning(ui)),
                        );
                    }
                    if widgets::action_button(
                        ui,
                        "Roll back RomM Library",
                        widgets::ActionStyle::Quiet,
                        true,
                    )
                    .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::RollbackRomm);
                    }
                }
            }
            widgets::technical_details(ui, "playing_library_romm_files", |ui| {
                for game in &projection.games {
                    ui.label(format!(
                        "Launcher: {} -> {}",
                        game.launcher.destination_path.display(),
                        game.launcher.source_path.display()
                    ));
                    for companion in &game.companions {
                        ui.label(format!(
                            "  Companion: {} -> {}",
                            companion.destination_path.display(),
                            companion.source_path.display()
                        ));
                    }
                }
            });
        }
    });
}

fn show_retrodeck_projection_summary(
    ui: &mut egui::Ui,
    state: &mut PlayingLibraryPageState,
    action: &mut Option<PlayingLibraryPageAction>,
) {
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Build RetroDECK Library", None);
        ui.label("Create a separate RetroDECK/ES-DE library of verified releases. Source files are never changed.");
        let old_settings = (
            state.retrodeck_destination_root_draft.clone(),
            state.retrodeck_visible_source_root_draft.clone(),
            state.retrodeck_visibility_verified,
        );
        ui.horizontal(|ui| {
            ui.label("RetroDECK destination root:");
            ui.add(
                egui::TextEdit::singleline(&mut state.retrodeck_destination_root_draft)
                    .desired_width(320.0)
                    .hint_text("e.g. /mnt/roms/RetroDECK"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Sandbox-visible source root:");
            ui.add(
                egui::TextEdit::singleline(&mut state.retrodeck_visible_source_root_draft)
                    .desired_width(320.0)
                    .hint_text("same absolute path for a reviewed bind"),
            );
        });
        if ui
            .checkbox(
                &mut state.retrodeck_visibility_verified,
                "I have verified RetroDECK can see these paths at the same absolute locations",
            )
            .changed()
        {
            state.retrodeck_projection = None;
        }
        if old_settings
            != (
                state.retrodeck_destination_root_draft.clone(),
                state.retrodeck_visible_source_root_draft.clone(),
                state.retrodeck_visibility_verified,
            )
        {
            state.retrodeck_projection = None;
            state.retrodeck_preview_transaction = None;
            state.output_projection = None;
            state.retrodeck_pending_apply = false;
        }
        if widgets::action_button(
            ui,
            "Preview RetroDECK Library",
            widgets::ActionStyle::Secondary,
            state.plan.is_some(),
        )
        .clicked()
        {
            *action = Some(PlayingLibraryPageAction::PreviewRetroDeck);
        }
        if let Some(error) = &state.retrodeck_error {
            ui.label(egui::RichText::new(error).color(theme::warning(ui)));
        }
        if state.retrodeck_applied.is_some() && state.retrodeck_phase.is_none() {
            ui.colored_label(theme::warning(ui), "Recovery needs review: this legacy link receipt has no validated combined RetroDECK metadata receipt. It is preserved; independent Undo is blocked.");
        }
        if let Some(projection) = &state.retrodeck_projection {
            ui.label(format!(
                "Game list: {} · {} new entry(s), {} already present",
                projection.es_de_publication.gamelist_path.display(),
                projection.es_de_publication.added.len(),
                projection.es_de_publication.already_present.len()
            ));
            ui.label(format!(
                "Destination: {}",
                projection.retrodeck_rom_root.display()
            ));
            ui.label(format!(
                "{} game(s), {} file(s), ES-DE system `{}`",
                projection.games.len(),
                projection.total_files,
                projection.es_de_system
            ));
            let companions: usize = projection
                .games
                .iter()
                .map(|game| game.companions.len())
                .sum();
            ui.label(format!(
                "{companions} companion file(s) kept with their launchers"
            ));
            let refused = &projection.es_de_publication.refused;
            if !refused.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "{} game(s) will not be added to RetroDECK's game list: an existing \
                         entry with the same file name cannot be confirmed to be a different game",
                        refused.len()
                    ))
                    .color(theme::WARNING),
                );
                for entry in refused {
                    ui.label(format!(
                        "  {}: existing entry {} ({})",
                        entry.entry.dat_entry_name, entry.existing_path, entry.reason
                    ));
                }
            }
            ui.label(if projection.visibility.is_verified() {
                "Sandbox visibility: Ready"
            } else {
                "Sandbox visibility: Needs setup"
            });
            if !projection.visibility.is_verified() {
                ui.label(egui::RichText::new("Apply is blocked until source and destination visibility is explicitly verified.").color(theme::warning(ui)));
            }
            widgets::technical_details(ui, "playing_library_retrodeck_files", |ui| {
                for game in &projection.games {
                    ui.label(format!(
                        "Launcher: {}",
                        game.launcher.destination_path.display()
                    ));
                    for companion in &game.companions {
                        ui.label(format!(
                            "Companion: {}",
                            companion.destination_path.display()
                        ));
                    }
                }
            });
        }
    });
}

/// Whether `draft` names something that plainly is not there yet: non-empty
/// but the path does not exist, or (when `must_be_dir`) exists but is not a
/// directory. An empty draft is never "missing" - that is the separate
/// "choose one first" state the Preview button's own disabled hint already
/// covers, not a bad-path error.
/// One plain-English line summarising a candidate's structured evidence -
/// region/language/revision/parent-clone status - reusing exactly the
/// values `crate::playing_library`'s own election produced
/// ([`archivefs_core::playing_library::CandidateEvidenceSummary`]), never
/// re-derived from a name or a filename here. Absent evidence reads as
/// "unknown", never as a blank or an inferred guess.
fn evidence_summary_line(evidence: &CandidateEvidenceSummary) -> String {
    let region = if evidence.regions.is_empty() {
        "unknown".to_string()
    } else {
        evidence.regions.join(", ")
    };
    let language = if evidence.languages.is_empty() {
        "unknown".to_string()
    } else {
        evidence.languages.join(", ")
    };
    let revision = evidence.revision.as_deref().unwrap_or("unknown");
    let relationship = if evidence.is_declared_parent {
        "declared parent"
    } else if evidence.is_declared_clone {
        "declared clone"
    } else {
        "no declared relationship"
    };
    format!(
        "dump: {} - region: {region} - language: {language} - revision: {revision} - {relationship}",
        evidence.dump_quality
    )
}

fn generic_mapping_explanation(identity: Option<&DatPlatformIdentity>) -> Vec<String> {
    match identity {
        Some(DatPlatformIdentity::Resolved {
            confidence: archivefs_core::dat::identity::DatPlatformConfidence::Strong,
            ..
        }) => Vec::new(),
        Some(DatPlatformIdentity::Resolved { .. }) => {
            vec!["platform evidence is not strong; using the explicit `unknown` folder".into()]
        }
        Some(DatPlatformIdentity::Unknown) => {
            vec!["no platform identity was established; using the explicit `unknown` folder".into()]
        }
        Some(DatPlatformIdentity::Ambiguous { .. }) => {
            vec!["platform identity is ambiguous; using the explicit `unknown` folder".into()]
        }
        None => {
            vec!["platform identity was not available; using the explicit `unknown` folder".into()]
        }
    }
}

fn path_looks_missing(draft: &str, must_be_dir: bool) -> bool {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return false;
    }
    let path = std::path::Path::new(trimmed);
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => must_be_dir && !metadata.is_dir(),
        Err(_) => true,
    }
}

fn show_preview_summary(
    ui: &mut egui::Ui,
    plan: &PlayingLibraryPlan,
    state: &PlayingLibraryPageState,
    action: &mut Option<PlayingLibraryPageAction>,
) {
    presentation::show_preview(ui, plan, state, action);
}

/// The "Publish to ES-DE" section: choose a platform, preview, confirm,
/// publish - or, while a previous update is unresolved, explain that and
/// offer to restore instead. Every number and path shown here comes
/// straight from an `archivefs_core::launch::es_de_publish` value; this
/// function only lays them out.
fn show_esde_publish_section(
    ui: &mut egui::Ui,
    state: &PlayingLibraryPageState,
    action: &mut Option<PlayingLibraryPageAction>,
) {
    widgets::section_header(ui, "Add to ES-DE / RetroDECK", None);
    ui.label(
        egui::RichText::new(
            "Add the games you just created to ES-DE's menu. Your original files and links are \
             never touched by this step.",
        )
        .color(theme::muted(ui)),
    );
    ui.add_space(6.0);

    widgets::card(ui, |ui| {
        // A recovery record takes over the whole section: nothing else
        // here is safe to offer until it is resolved one way or the other.
        if let Some(gamelist_path) = state.esde_recovery_gamelist_path() {
            widgets::banner(
                ui,
                "A previous ES-DE update did not finish",
                "EmuWiz cannot tell whether that update fully applied before EmuWiz or your \
                 computer stopped. Restore ES-DE's previous menu to a known-good state before \
                 publishing again.",
                widgets::StatusTone::Warning,
            );
            ui.label(format!("Game list: {}", gamelist_path.display()));

            if state.esde_recovery_pending() {
                ui.label("Restore ES-DE's previous menu now?");
                ui.horizontal(|ui| {
                    if widgets::action_button(
                        ui,
                        "Confirm",
                        widgets::ActionStyle::Destructive,
                        true,
                    )
                    .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::ConfirmEsdeRecovery);
                    }
                    if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::CancelEsdeRecovery);
                    }
                });
            } else if widgets::action_button(
                ui,
                "Restore previous ES-DE menu",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                *action = Some(PlayingLibraryPageAction::RequestEsdeRecovery);
            }

            if let Some((friendly, detail)) = state.esde_recovery_error() {
                ui.add_space(6.0);
                widgets::banner(
                    ui,
                    "Could not restore ES-DE's previous menu",
                    friendly,
                    widgets::StatusTone::Blocked,
                );
                if let Some(detail) = detail {
                    widgets::technical_details(ui, "playing_library_esde_recovery_error", |ui| {
                        ui.label(detail);
                    });
                }
            }
            return;
        }

        if state.esde_recovery_done() {
            ui.label(
                egui::RichText::new("ES-DE's previous menu was restored.")
                    .color(theme::success(ui)),
            );
            ui.add_space(6.0);
        }

        ui.label("Platform:");
        let platforms = esde_platform_options();
        if let Some(clicked) = widgets::platform_picker(
            ui,
            "playing_library_esde_platform",
            &platforms,
            state.esde_platform_id(),
            true,
        ) {
            *action = Some(PlayingLibraryPageAction::SelectEsdePlatform(clicked));
        }
        ui.add_space(6.0);

        if widgets::action_button(
            ui,
            "Preview ES-DE changes",
            widgets::ActionStyle::Primary,
            state.esde_platform_id().is_some(),
        )
        .clicked()
        {
            *action = Some(PlayingLibraryPageAction::PreviewEsde);
        }
        if state.esde_platform_id().is_none() {
            ui.label(
                egui::RichText::new("Choose a platform first.")
                    .color(theme::muted(ui))
                    .small(),
            );
        }

        if let Some(error) = state.esde_discovery_error() {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "ES-DE was not found",
                error,
                widgets::StatusTone::Blocked,
            );
        }

        if let Some((friendly, detail)) = state.esde_preview_error() {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Could not preview ES-DE changes",
                friendly,
                widgets::StatusTone::Blocked,
            );
            if let Some(detail) = detail {
                widgets::technical_details(ui, "playing_library_esde_preview_error", |ui| {
                    ui.label(detail);
                });
            }
        }

        if let Some(publication) = state.esde_publication() {
            ui.add_space(8.0);
            if let Some(profile) = state.esde_profile() {
                ui.label(format!(
                    "ES-DE profile: {}",
                    profile.home_directory.path.display
                ));
            }
            ui.label(format!("System: {}", publication.es_de_system));
            ui.label(format!(
                "Game list: {}",
                publication.gamelist_path.display()
            ));
            ui.label(if publication.previous_content.is_some() {
                "Existing gamelist: Found"
            } else {
                "Existing gamelist: Not found — a new one will be created"
            });
            ui.label(format!("{} new game(s)", publication.added.len()));
            ui.label(format!(
                "{} already in ES-DE",
                publication.already_present.len()
            ));
            if !publication.refused.is_empty() {
                ui.label(format!(
                    "{} game(s) not added: ES-DE already has an entry with the same file name \
                     that EmuWiz cannot confirm is a different game",
                    publication.refused.len()
                ));
                for refused in &publication.refused {
                    ui.label(format!(
                        "  {}: existing entry {} ({})",
                        refused.entry.dat_entry_name, refused.existing_path, refused.reason
                    ));
                }
            }
            ui.label("Existing unrelated entries preserved: Yes");
            ui.label("Recovery backup: Ready before publication");
            if let Some(plan) = state.applied_plan() {
                let not_included = plan.unresolved_groups.len() + plan.exclusions.len();
                if not_included > 0 {
                    ui.label(format!(
                        "{not_included} release(s) from your playing library plan were not \
                         part of it and so are not published (see the summary above for why)"
                    ));
                }
            }

            if state.esde_published() {
                ui.add_space(6.0);
                if publication.is_unchanged() {
                    ui.label(
                        egui::RichText::new("ES-DE was already up to date - nothing changed.")
                            .color(theme::success(ui)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!(
                            "Added {} game(s) to ES-DE.",
                            publication.added.len()
                        ))
                        .color(theme::success(ui))
                        .strong(),
                    );
                }
            } else if publication.is_unchanged() && !publication.refused.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Nothing was added: {} game(s) need review (listed above).",
                        publication.refused.len()
                    ))
                    .color(theme::WARNING),
                );
            } else if publication.is_unchanged() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("ES-DE is already up to date.").color(theme::muted(ui)),
                );
            } else if state.esde_pending_publish() {
                ui.add_space(6.0);
                ui.label(format!("Add {} game(s) to ES-DE?", publication.added.len()));
                ui.horizontal(|ui| {
                    if widgets::action_button(ui, "Confirm", widgets::ActionStyle::Primary, true)
                        .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::ConfirmEsdePublish);
                    }
                    if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::CancelEsdePublish);
                    }
                });
            } else {
                ui.add_space(6.0);
                if widgets::action_button(
                    ui,
                    "Add to ES-DE / RetroDECK",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
                {
                    *action = Some(PlayingLibraryPageAction::RequestEsdePublish);
                }
            }
        }

        if let Some((friendly, detail)) = state.esde_publish_error() {
            ui.add_space(6.0);
            if matches!(
                state.applied(),
                Some(transaction) if transaction.state == TransactionState::Applied
            ) {
                ui.label(
                    egui::RichText::new(
                        "Library created successfully. ES-DE metadata was not published.",
                    )
                    .color(theme::warning(ui))
                    .strong(),
                );
            }
            widgets::banner(
                ui,
                "Could not add games to ES-DE",
                friendly,
                widgets::StatusTone::Blocked,
            );
            if let Some(detail) = detail {
                widgets::technical_details(ui, "playing_library_esde_publish_error", |ui| {
                    ui.label(detail);
                });
            }
        }
    });
}

#[cfg(test)]
mod tests;
