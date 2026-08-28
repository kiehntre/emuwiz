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

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use archivefs_core::dat::identity::{DatPlatformIdentity, identify_dat_source};
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::parsers::parse_dat_file;
use archivefs_core::dat::rename_apply::executor::{
    ApplyExecution, HardConflictMode, apply_transaction,
};
use archivefs_core::dat::rename_apply::journal::{default_rename_transaction_dir, write_journal};
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
use archivefs_core::playing_library::{
    CandidateEvidenceSummary, PlayingLibraryPlan, PlayingLibraryPolicy, PlayingLibraryRequest,
    ReleaseClass, RetroDeckProjectionPlan, RetroDeckVisibility, RommLibraryProjectionPlan,
    RommVisibility, build_playing_library_plan, build_playing_library_transaction,
    build_retrodeck_projection, build_retrodeck_projection_transaction,
    build_romm_projection_transaction, build_romm_projection_with_visibility,
    match_loose_files_against_dat,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;

use crate::rom_organisation_page::collect_source_files;
use crate::ui::{components as widgets, theme};

/// The confirmation phrase a user must type before a Create Playing Library
/// apply larger than [`TYPED_CONFIRMATION_THRESHOLD`] runs - truthful
/// wording, matching `crate::rom_organisation_page::apply_confirmation_phrase`
/// for `OrganisationMode::BuildLinkedLibrary`: a link is created, nothing is
/// renamed or moved.
pub(crate) fn playing_library_confirmation_phrase(count: usize) -> String {
    format!("CREATE {count} LINKS")
}

pub(crate) const TYPED_CONFIRMATION_THRESHOLD: usize = 8;

/// The page's authoritative state.
pub(crate) struct PlayingLibraryPageState {
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
    dat_platform_identity: Option<DatPlatformIdentity>,
    plan_generation: u64,
    error: Option<String>,
    /// The elected family currently shown in "Why this one?", identified by
    /// its own `dat_entry_name` (unique per election within one plan).
    selected_family: Option<String>,
    pending_apply: Option<usize>,
    confirm_text: String,
    applied: Option<RenameTransaction>,
    romm_projection: Option<RommLibraryProjectionPlan>,
    romm_pending_apply: bool,
    romm_confirm_text: String,
    romm_applied: Option<RenameTransaction>,
    romm_error: Option<String>,
    romm_visibility_verified: bool,
    romm_visible_source_root_draft: String,
    retrodeck_projection: Option<RetroDeckProjectionPlan>,
    retrodeck_error: Option<String>,
    retrodeck_visibility_verified: bool,
    retrodeck_visible_source_root_draft: String,
    retrodeck_destination_root_draft: String,
    retrodeck_pending_apply: bool,
    retrodeck_confirm_text: String,
    retrodeck_applied: Option<RenameTransaction>,
    /// The plan that produced `applied`, kept (instead of dropped like
    /// `plan` is) purely so "Publish to ES-DE" - offered only once a
    /// library has actually been created - can still name which elections
    /// are eligible. Never mutated once set; `plan_es_de_gamelist_publication`
    /// reads it exactly as `build_playing_library_transaction` already did.
    applied_plan: Option<PlayingLibraryPlan>,
    apply_error: Option<String>,
    journal_dir: PathBuf,

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
            dat_path_draft: String::new(),
            source_root_draft: String::new(),
            destination_root_draft: String::new(),
            preferred_regions_draft: "Europe, USA, Japan".to_string(),
            preferred_languages_draft: String::new(),
            prefer_newest_revision: true,
            prefer_parent: true,
            exclude_beta: true,
            exclude_proto: true,
            exclude_demo: true,
            exclude_sample: true,
            plan: None,
            dat_platform_identity: None,
            plan_generation: 0,
            error: None,
            selected_family: None,
            pending_apply: None,
            confirm_text: String::new(),
            applied: None,
            romm_projection: None,
            romm_pending_apply: false,
            romm_confirm_text: String::new(),
            romm_applied: None,
            romm_error: None,
            romm_visibility_verified: false,
            romm_visible_source_root_draft: String::new(),
            retrodeck_projection: None,
            retrodeck_error: None,
            retrodeck_visibility_verified: false,
            retrodeck_visible_source_root_draft: String::new(),
            retrodeck_destination_root_draft: String::new(),
            retrodeck_pending_apply: false,
            retrodeck_confirm_text: String::new(),
            retrodeck_applied: None,
            applied_plan: None,
            apply_error: None,
            journal_dir: default_rename_transaction_dir()
                .unwrap_or_else(|_| PathBuf::from("rename-transactions")),
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

impl PlayingLibraryPageState {
    pub(crate) fn load() -> Self {
        Self::default()
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
        self.dat_platform_identity = None;
        self.romm_projection = None;
        self.romm_error = None;
        self.romm_visibility_verified = false;
        self.error = None;
        self.selected_family = None;
        self.plan_generation += 1;

        let dat_path = PathBuf::from(self.dat_path_draft.trim());
        let source_root = PathBuf::from(self.source_root_draft.trim());
        let destination_root = PathBuf::from(self.destination_root_draft.trim());
        if self.dat_path_draft.trim().is_empty() {
            self.error = Some("choose a DAT catalogue file first".to_string());
            return;
        }
        if self.source_root_draft.trim().is_empty() {
            self.error = Some("choose a source library folder first".to_string());
            return;
        }
        if !destination_root.is_absolute() {
            self.error = Some("the destination folder must be an absolute path".to_string());
            return;
        }

        let outcome = match parse_dat_file(&dat_path, DatLimits::default()) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.error = Some(format!("could not read the DAT catalogue: {error}"));
                return;
            }
        };
        self.dat_platform_identity = Some(identify_dat_source(&outcome.dat));

        let candidates = collect_source_files(std::slice::from_ref(&source_root));
        let trusted = TrustedRoots::from_paths([&source_root]);
        let outcome_matches = match_loose_files_against_dat(
            &outcome.dat,
            &candidates,
            &trusted,
            &AtomicBool::new(false),
        );

        let request = PlayingLibraryRequest {
            dat: &outcome.dat,
            matches: outcome_matches.matches,
            destination_root,
            policy: self.build_policy(),
        };
        match build_playing_library_plan(&request) {
            Ok(mut plan) => {
                plan.rejected_launchers = outcome_matches.rejected_launchers;
                self.plan = Some(plan);
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub(crate) fn request_apply(&mut self) {
        let Some(plan) = &self.plan else {
            return;
        };
        self.apply_error = None;
        self.pending_apply = Some(plan.elected_games.len());
        self.confirm_text.clear();
    }

    pub(crate) fn cancel_apply(&mut self) {
        self.pending_apply = None;
        self.confirm_text.clear();
    }

    pub(crate) fn preview_romm(&mut self) {
        self.romm_projection = None;
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
            Ok(projection) => self.romm_projection = Some(projection),
            Err(error) => self.romm_error = Some(error),
        }
    }

    pub(crate) fn preview_retrodeck(&mut self) {
        self.retrodeck_projection = None;
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
            Ok(projection) => self.retrodeck_projection = Some(projection),
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
        let Some(projection) = &self.retrodeck_projection else {
            return;
        };
        if projection.total_files > TYPED_CONFIRMATION_THRESHOLD
            && self.retrodeck_confirm_text.trim()
                != playing_library_confirmation_phrase(projection.total_files)
        {
            self.retrodeck_error = Some("the typed confirmation did not match".into());
            return;
        }
        let mut transaction =
            match build_retrodeck_projection_transaction(projection, self.plan_generation) {
                Ok(value) => value,
                Err(error) => {
                    self.retrodeck_error = Some(error);
                    return;
                }
            };
        if let Err(error) = std::fs::create_dir_all(&projection.retrodeck_rom_root) {
            self.retrodeck_error = Some(error.to_string());
            return;
        }
        if let Err(error) = write_journal(&self.journal_dir, &transaction) {
            self.retrodeck_error = Some(error.to_string());
            return;
        }
        let approved_paths = transaction
            .entries
            .iter()
            .map(|entry| entry.source_path.to_string_lossy().into_owned())
            .collect();
        let trusted = TrustedRoots::from_paths([
            projection.retrodeck_rom_root.clone(),
            self.source_root_draft_path(),
        ]);
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
        self.retrodeck_pending_apply = false;
        match result {
            Ok(mut outcome) => {
                if let Err(error) =
                    archivefs_core::launch::es_de_publish::apply_es_de_gamelist_publication(
                        &projection.es_de_publication,
                    )
                {
                    let rollback_error = rollback_transaction(
                        &mut outcome.transaction,
                        &self.journal_dir,
                        &AtomicBool::new(false),
                    )
                    .err();
                    self.retrodeck_error = Some(match rollback_error {
                        Some(rollback) => format!(
                            "ES-DE publication failed: {error}; link rollback also failed: {rollback}"
                        ),
                        None => error.to_string(),
                    });
                } else {
                    self.retrodeck_applied = Some(outcome.transaction);
                }
            }
            Err(error) => self.retrodeck_error = Some(error.to_string()),
        }
    }

    pub(crate) fn rollback_retrodeck(&mut self) {
        if let Some(projection) = &self.retrodeck_projection
            && let Err(error) =
                archivefs_core::launch::es_de_publish::rollback_es_de_gamelist_publication(
                    &projection.es_de_publication,
                )
        {
            self.retrodeck_error = Some(error.to_string());
            return;
        }
        if let Some(transaction) = &mut self.retrodeck_applied
            && let Err(error) =
                rollback_transaction(transaction, &self.journal_dir, &AtomicBool::new(false))
        {
            self.retrodeck_error = Some(error);
        }
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
        if count > TYPED_CONFIRMATION_THRESHOLD
            && self.romm_confirm_text.trim() != playing_library_confirmation_phrase(count)
        {
            self.romm_error = Some("the typed confirmation did not match".to_string());
            return;
        }
        let mut transaction =
            match build_romm_projection_transaction(projection, self.plan_generation) {
                Ok(transaction) => transaction,
                Err(error) => {
                    self.romm_error = Some(error);
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
            Ok(outcome) => self.romm_applied = Some(outcome.transaction),
            Err(error) => self.romm_error = Some(error.to_string()),
        }
    }

    pub(crate) fn rollback_romm_last(&mut self) {
        let Some(transaction) = &mut self.romm_applied else {
            return;
        };
        if let Err(error) =
            rollback_transaction(transaction, &self.journal_dir, &AtomicBool::new(false))
        {
            self.romm_error = Some(error);
        }
    }

    /// Builds the transaction from the current plan and runs it through the
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
        if count > TYPED_CONFIRMATION_THRESHOLD
            && self.confirm_text.trim() != playing_library_confirmation_phrase(count)
        {
            self.apply_error = Some("the typed confirmation did not match".to_string());
            return;
        }
        let Some(plan) = &self.plan else {
            self.pending_apply = None;
            return;
        };
        let mut transaction = match build_playing_library_transaction(plan, self.plan_generation) {
            Ok(transaction) => transaction,
            Err(error) => {
                self.apply_error = Some(error);
                self.pending_apply = None;
                return;
            }
        };
        if let Err(error) = std::fs::create_dir_all(&plan.destination_root) {
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
        self.pending_apply = None;
        self.confirm_text.clear();
        match result {
            Ok(outcome) => {
                // Kept (not dropped) purely so "Publish to ES-DE" - offered
                // only once a library has actually been created - still has
                // the elections that produced it.
                self.applied_plan = self.plan.take();
                self.applied = Some(outcome.transaction);
            }
            Err(error) => self.apply_error = Some(error.to_string()),
        }
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
        match rollback_transaction(transaction, &self.journal_dir, &AtomicBool::new(false)) {
            Ok(_) => {}
            Err(error) => self.apply_error = Some(error),
        }
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

    /// Restores ES-DE's previous gamelist exactly, through
    /// `recover_es_de_gamelist_publication` - never touches master ROMs or
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
        EsDePublicationError::NothingToPublish => {
            "There is nothing new to publish to ES-DE.".to_string()
        }
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
    RequestRommApply,
    CancelRommApply,
    ConfirmRommApply,
    RollbackRomm,
    PreviewRetroDeck,
    RequestRetroDeckApply,
    CancelRetroDeckApply,
    ConfirmRetroDeckApply,
    RollbackRetroDeck,
}

/// Stable, absolute widget ids for the text fields a test needs to drive
/// through real keyboard focus + `egui::Event::Text` (the same
/// `ctx.memory_mut(|memory| memory.request_focus(id))` pattern this crate's
/// own `set_text_edit_caret`/`apply_select_all` already use) rather than by
/// mutating page state directly.
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
    let mut action = None;

    widgets::section_header(ui, "Build Playing Library", None);
    ui.label(
        egui::RichText::new(
            "Pick one representative release per game and create a linked library of it. \
             Your original files are never moved, renamed, or changed.",
        )
        .color(theme::muted(ui)),
    );
    ui.add_space(8.0);

    widgets::card(ui, |ui| {
        ui.label("Catalogue (DAT) file:");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.dat_path_draft)
                    .id(egui::Id::new(DAT_PATH_FIELD_ID))
                    .desired_width(ui.available_width() - 90.0),
            );
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose Playing Library DAT Catalogue")
                    .add_filter("DAT catalogues", &["dat", "xml"])
                    .pick_file()
            {
                state.dat_path_draft = path.display().to_string();
            }
        });
        if path_looks_missing(&state.dat_path_draft, false) {
            ui.label(egui::RichText::new("This file was not found.").color(theme::WARNING));
        }
        ui.add_space(6.0);

        ui.label("Source:");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.source_root_draft)
                    .id(egui::Id::new(SOURCE_ROOT_FIELD_ID))
                    .desired_width(ui.available_width() - 90.0),
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
            ui.label(egui::RichText::new("This folder was not found.").color(theme::WARNING));
        }
        ui.add_space(6.0);

        ui.label("Destination:");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.destination_root_draft)
                    .id(egui::Id::new(DESTINATION_ROOT_FIELD_ID))
                    .desired_width(ui.available_width() - 90.0),
            );
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose Playing Library Destination Folder")
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
    widgets::section_header(ui, "Preferences", None);
    widgets::card(ui, |ui| {
        ui.label("Region order (most preferred first):");
        ui.add(
            egui::TextEdit::singleline(&mut state.preferred_regions_draft)
                .id(egui::Id::new(PREFERRED_REGIONS_FIELD_ID))
                .hint_text("Europe, USA, Japan"),
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
        ui.horizontal(|ui| {
            ui.checkbox(&mut state.exclude_beta, "Beta");
            ui.checkbox(&mut state.exclude_proto, "Proto");
            ui.checkbox(&mut state.exclude_demo, "Demo");
            ui.checkbox(&mut state.exclude_sample, "Sample");
        });
    });

    ui.add_space(8.0);
    let ready = !state.dat_path_draft.trim().is_empty()
        && !state.source_root_draft.trim().is_empty()
        && !state.destination_root_draft.trim().is_empty();
    if widgets::action_button(
        ui,
        "Preview Playing Library",
        widgets::ActionStyle::Primary,
        ready,
    )
    .clicked()
    {
        action = Some(PlayingLibraryPageAction::Preview);
    }
    if !ready {
        ui.label(
            egui::RichText::new("Choose a DAT catalogue, a source, and a destination first.")
                .color(theme::muted(ui))
                .small(),
        );
    }

    if let Some(error) = state.error() {
        ui.add_space(6.0);
        widgets::banner(
            ui,
            "Could not build a preview",
            error,
            widgets::StatusTone::Blocked,
        );
    }

    if let Some(plan) = state.plan() {
        ui.add_space(10.0);
        show_preview_summary(ui, plan, state, &mut action);
        ui.add_space(10.0);
        show_romm_projection_summary(ui, state, &mut action);
        show_retrodeck_projection_summary(ui, state, &mut action);
    }

    if let Some(transaction) = state.applied() {
        ui.add_space(10.0);
        widgets::card(ui, |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Playing library created: {} link(s)",
                    transaction.applied_count()
                ))
                .strong(),
            );
            if transaction.state == TransactionState::Applied
                && widgets::action_button(ui, "Undo", widgets::ActionStyle::Quiet, true).clicked()
            {
                action = Some(PlayingLibraryPageAction::RollbackLast);
            }
        });
    }

    if let Some(error) = state.apply_error() {
        ui.add_space(6.0);
        widgets::banner(
            ui,
            "Could not create the playing library",
            error,
            widgets::StatusTone::Blocked,
        );
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
        if state.romm_projection.is_none() {
            if widgets::action_button(
                ui,
                "Preview RomM Library",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                *action = Some(PlayingLibraryPageAction::PreviewRomm);
            }
        }
        if let Some(error) = &state.romm_error {
            ui.label(egui::RichText::new(error).color(theme::WARNING));
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
                    "Apply is blocked until Docker/bind-mount visibility is explicitly verified.",
                )
                .color(theme::WARNING),
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
            ui.label(format!(
                "Visibility: {}",
                projection.visibility.description()
            ));
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
            if !state.romm_pending_apply
                && state.romm_applied.is_none()
                && widgets::action_button(
                    ui,
                    "Create RomM Library",
                    widgets::ActionStyle::Primary,
                    projection.visibility.is_verified(),
                )
                .clicked()
            {
                *action = Some(PlayingLibraryPageAction::RequestRommApply);
            }
            if state.romm_pending_apply {
                if projection.total_files > TYPED_CONFIRMATION_THRESHOLD {
                    ui.label(format!(
                        "Type \"{}\" to confirm:",
                        playing_library_confirmation_phrase(projection.total_files)
                    ));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.romm_confirm_text)
                            .desired_width(260.0)
                            .hint_text(playing_library_confirmation_phrase(projection.total_files)),
                    );
                }
                ui.horizontal(|ui| {
                    if widgets::action_button(
                        ui,
                        "Confirm",
                        widgets::ActionStyle::Destructive,
                        true,
                    )
                    .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::ConfirmRommApply);
                    }
                    if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::CancelRommApply);
                    }
                });
            }
            if let Some(transaction) = &state.romm_applied {
                if transaction.state == TransactionState::RolledBack {
                    ui.label(
                        egui::RichText::new("RomM library rolled back; no generated links remain.")
                            .color(theme::SUCCESS),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!(
                            "RomM library created: {} link(s)",
                            transaction.applied_count()
                        ))
                        .color(theme::SUCCESS),
                    );
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
            ui.label(egui::RichText::new(error).color(theme::WARNING));
        }
        if let Some(projection) = &state.retrodeck_projection {
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
            ui.label(format!(
                "Visibility: {}",
                projection.visibility.description()
            ));
            if !projection.visibility.is_verified() {
                ui.label(egui::RichText::new("Apply is blocked until source and destination visibility is explicitly verified.").color(theme::WARNING));
            }
            if !state.retrodeck_pending_apply
                && state.retrodeck_applied.is_none()
                && widgets::action_button(
                    ui,
                    "Create RetroDECK Library",
                    widgets::ActionStyle::Primary,
                    projection.visibility.is_verified(),
                )
                .clicked()
            {
                *action = Some(PlayingLibraryPageAction::RequestRetroDeckApply);
            }
            if state.retrodeck_pending_apply {
                if projection.total_files > TYPED_CONFIRMATION_THRESHOLD {
                    ui.label(format!(
                        "Type \"{}\" to confirm:",
                        playing_library_confirmation_phrase(projection.total_files)
                    ));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.retrodeck_confirm_text)
                            .desired_width(260.0),
                    );
                }
                if widgets::action_button(ui, "Confirm", widgets::ActionStyle::Destructive, true)
                    .clicked()
                {
                    *action = Some(PlayingLibraryPageAction::ConfirmRetroDeckApply);
                }
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked()
                {
                    *action = Some(PlayingLibraryPageAction::CancelRetroDeckApply);
                }
            }
            if let Some(transaction) = &state.retrodeck_applied {
                if transaction.state == TransactionState::RolledBack {
                    ui.label(
                        egui::RichText::new(
                            "RetroDECK library rolled back; generated links were removed.",
                        )
                        .color(theme::SUCCESS),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!(
                            "RetroDECK library created: {} link(s)",
                            transaction.applied_count()
                        ))
                        .color(theme::SUCCESS),
                    );
                    if widgets::action_button(
                        ui,
                        "Roll back RetroDECK Library",
                        widgets::ActionStyle::Quiet,
                        true,
                    )
                    .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::RollbackRetroDeck);
                    }
                }
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
    format!("region: {region} - language: {language} - revision: {revision} - {relationship}")
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
    widgets::card(ui, |ui| {
        ui.label(format!("{} verified releases", plan.archives_examined));
        ui.label(format!("{} game families", plan.families_examined));
        ui.label(format!(
            "{} selected for playing library",
            plan.elected_games.len()
        ));
        ui.label(format!("{} unresolved", plan.unresolved_groups.len()));
        ui.label(format!("{} destination conflicts", plan.conflicts.len()));

        ui.add_space(6.0);
        if plan.elected_games.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Nothing can be created yet - resolve the unresolved groups below, or \
                     relax a preference.",
                )
                .color(theme::muted(ui)),
            );
        } else if !plan.conflicts.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Destination name conflicts must be resolved before creating the library.",
                )
                .color(theme::WARNING),
            );
        } else if widgets::action_button(
            ui,
            "Create Playing Library",
            widgets::ActionStyle::Primary,
            true,
        )
        .clicked()
        {
            *action = Some(PlayingLibraryPageAction::RequestApply);
        }

        if !plan.unresolved_groups.is_empty() {
            widgets::technical_details(ui, ("playing_library_unresolved", "unresolved"), |ui| {
                for group in &plan.unresolved_groups {
                    ui.label(format!(
                        "{}: {}",
                        group.family_root_name,
                        group.tied_candidates.join(", ")
                    ));
                }
            });
        }

        ui.add_space(8.0);
        for elected in &plan.elected_games {
            ui.horizontal(|ui| {
                ui.label(&elected.dat_entry_name);
                let selected = state.selected_family() == Some(elected.dat_entry_name.as_str());
                let label = if selected { "Hide" } else { "Why this one?" };
                if widgets::action_button(ui, label, widgets::ActionStyle::Quiet, true).clicked() {
                    *action = Some(PlayingLibraryPageAction::SelectFamily(if selected {
                        None
                    } else {
                        Some(elected.dat_entry_name.clone())
                    }));
                }
            });
            if state.selected_family() == Some(elected.dat_entry_name.as_str()) {
                ui.label(egui::RichText::new("Selected because:").strong());
                if elected.explanation.steps.is_empty() {
                    ui.label("- the only election-eligible release in its family");
                }
                for step in &elected.explanation.steps {
                    ui.label(format!("- {step}"));
                }
                ui.label(evidence_summary_line(&elected.explanation.winner_evidence));

                if !elected.explanation.rejected.is_empty() {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Not selected:").strong());
                    for rejected in &elected.explanation.rejected {
                        ui.label(format!(
                            "{} - not selected because:",
                            rejected.dat_entry_name
                        ));
                        for reason in &rejected.reasons {
                            ui.label(format!("  - {reason}"));
                        }
                        ui.label(format!("  {}", evidence_summary_line(&rejected.evidence)));
                    }
                }

                widgets::technical_details(
                    ui,
                    (
                        "playing_library_election_evidence",
                        elected.dat_entry_name.as_str(),
                    ),
                    |ui| {
                        ui.label(format!("winner: {:?}", elected.explanation.winner_evidence));
                        for rejected in &elected.explanation.rejected {
                            ui.label(format!(
                                "{}: {:?}",
                                rejected.dat_entry_name, rejected.evidence
                            ));
                        }
                    },
                );
            }
        }
    });

    if let Some(count) = state.pending_apply {
        widgets::card(ui, |ui| {
            ui.label(format!("Create {count} link(s)?"));
            if count > TYPED_CONFIRMATION_THRESHOLD {
                ui.label(format!(
                    "Type \"{}\" to confirm:",
                    playing_library_confirmation_phrase(count)
                ));
                ui.label(egui::RichText::new(&state.confirm_text).monospace());
            }
            ui.horizontal(|ui| {
                if widgets::action_button(ui, "Confirm", widgets::ActionStyle::Destructive, true)
                    .clicked()
                {
                    *action = Some(PlayingLibraryPageAction::ConfirmApply);
                }
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked()
                {
                    *action = Some(PlayingLibraryPageAction::CancelApply);
                }
            });
        });
    }
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
    widgets::section_header(ui, "Publish to ES-DE", None);
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
                egui::RichText::new("ES-DE's previous menu was restored.").color(theme::SUCCESS),
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
            ui.label(format!("{} new game(s)", publication.added.len()));
            ui.label(format!(
                "{} already in ES-DE",
                publication.already_present.len()
            ));
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
                            .color(theme::SUCCESS),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!(
                            "Published {} game(s) to ES-DE.",
                            publication.added.len()
                        ))
                        .color(theme::SUCCESS)
                        .strong(),
                    );
                }
            } else if publication.is_unchanged() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("ES-DE is already up to date - nothing to publish.")
                        .color(theme::muted(ui)),
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
                    "Publish to ES-DE",
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
            widgets::banner(
                ui,
                "Could not publish to ES-DE",
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
