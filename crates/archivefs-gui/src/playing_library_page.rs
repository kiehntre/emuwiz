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

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::parsers::parse_dat_file;
use archivefs_core::dat::rename_apply::executor::{
    ApplyExecution, HardConflictMode, apply_transaction,
};
use archivefs_core::dat::rename_apply::journal::{default_rename_transaction_dir, write_journal};
use archivefs_core::dat::rename_apply::model::{RenameTransaction, TransactionState};
use archivefs_core::dat::rename_apply::preflight::DirectoryPolicy;
use archivefs_core::dat::rename_apply::rollback::rollback_transaction;
use archivefs_core::playing_library::{
    DatArchiveMatch, PlayingLibraryPlan, PlayingLibraryPolicy, PlayingLibraryRequest, ReleaseClass,
    build_playing_library_plan, build_playing_library_transaction, match_loose_files_against_dat,
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
    plan_generation: u64,
    error: Option<String>,
    /// The elected family currently shown in "Why this one?", identified by
    /// its own `dat_entry_name` (unique per election within one plan).
    selected_family: Option<String>,
    pending_apply: Option<usize>,
    confirm_text: String,
    applied: Option<RenameTransaction>,
    apply_error: Option<String>,
    journal_dir: PathBuf,
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
            plan_generation: 0,
            error: None,
            selected_family: None,
            pending_apply: None,
            confirm_text: String::new(),
            applied: None,
            apply_error: None,
            journal_dir: default_rename_transaction_dir()
                .unwrap_or_else(|_| PathBuf::from("rename-transactions")),
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

        let candidates = collect_source_files(std::slice::from_ref(&source_root));
        let trusted = TrustedRoots::from_paths([&source_root]);
        let matches: Vec<DatArchiveMatch> = match_loose_files_against_dat(
            &outcome.dat,
            &candidates,
            &trusted,
            &AtomicBool::new(false),
        );

        let request = PlayingLibraryRequest {
            dat: &outcome.dat,
            matches,
            destination_root,
            policy: self.build_policy(),
        };
        match build_playing_library_plan(&request) {
            Ok(plan) => self.plan = Some(plan),
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
                self.applied = Some(outcome.transaction);
                self.plan = None;
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
}

pub(crate) enum PlayingLibraryPageAction {
    Preview,
    SelectFamily(Option<String>),
    RequestApply,
    CancelApply,
    ConfirmApply,
    RollbackLast,
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

    action
}

/// Whether `draft` names something that plainly is not there yet: non-empty
/// but the path does not exist, or (when `must_be_dir`) exists but is not a
/// directory. An empty draft is never "missing" - that is the separate
/// "choose one first" state the Preview button's own disabled hint already
/// covers, not a bad-path error.
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
                ui.label(egui::RichText::new("Why:").strong());
                if elected.explanation.steps.is_empty() {
                    ui.label("- the only election-eligible release in its family");
                }
                for step in &elected.explanation.steps {
                    ui.label(format!("- {step}"));
                }
                if !elected.explanation.rejected.is_empty() {
                    ui.label("Other verified releases:");
                    for rejected in &elected.explanation.rejected {
                        ui.label(format!("- {}", rejected.dat_entry_name));
                    }
                }
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

#[cfg(test)]
mod tests;
