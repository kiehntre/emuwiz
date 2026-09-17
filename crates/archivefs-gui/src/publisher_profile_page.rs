//! Publisher / Frontend Library planning view - task section 19.
//!
//! A thin GUI over `archivefs_core::publisher_profile`. This page never
//! scans or elects anything itself: it takes an
//! already-built `PlayingLibraryPlan` (produced by the existing "Build
//! Playing Library" flow), a chosen target profile, and a destination root.
//! Planning remains read-only; explicit execution delegates to the shared
//! journaled transaction engine and is available only after typed confirmation.
//!
use std::path::PathBuf;

use archivefs_core::dat::rename_apply::{ApplyError, ApplyOutcome, TransactionState};
use archivefs_core::platform_evidence_fusion::romm_platform_mapping::FrontendPlatformMapping;
use archivefs_core::playing_library::PlayingLibraryPlan;
use archivefs_core::publisher_profile::es_de::{es_de_profile, resolve_es_de_platform_mapping};
use archivefs_core::publisher_profile::romm::{resolve_romm_platform_mapping, romm_profile};
use archivefs_core::publisher_profile::{
    PublisherActionSafety, PublisherFrontend, PublisherLinkMode, PublisherPlan, PublisherPlanItem,
    PublisherPlanRequest, PublisherTransaction, apply_publisher_transaction, build_publisher_plan,
    build_publisher_transaction_with_policy, rollback_publisher_transaction,
};
use eframe::egui;

// The page adapter moved here from `main.rs` still spells this page's own
// items by module name; keeping that name in scope leaves the moved body
// byte-for-byte identical to what `main.rs` ran.
use crate::publisher_profile_page;
use crate::ui::{components as widgets, theme};
use crate::{ArchiveFsApp, MainView};

/// Which result bucket the user is currently filtering to - task section
/// 19's exact filter list, plus "All".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PublisherResultFilter {
    #[default]
    All,
    Ready,
    AlreadyPresent,
    ReviewRequired,
    Blocked,
    Unsupported,
}

impl PublisherResultFilter {
    const ALL: [Self; 6] = [
        Self::All,
        Self::Ready,
        Self::AlreadyPresent,
        Self::ReviewRequired,
        Self::Blocked,
        Self::Unsupported,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Ready => "Ready",
            Self::AlreadyPresent => "Already present",
            Self::ReviewRequired => "Review required",
            Self::Blocked => "Blocked",
            Self::Unsupported => "Unsupported",
        }
    }

    fn matches(self, item: &PublisherPlanItem) -> bool {
        match self {
            Self::All => true,
            Self::Ready => {
                item.safety == PublisherActionSafety::SafeToAct
                    && item.destination_state
                        != archivefs_core::publisher_profile::DestinationState::AlreadyCorrect
            }
            Self::AlreadyPresent => {
                item.destination_state
                    == archivefs_core::publisher_profile::DestinationState::AlreadyCorrect
            }
            Self::ReviewRequired => item.safety == PublisherActionSafety::ReviewRequired,
            Self::Blocked => item.safety == PublisherActionSafety::Blocked,
            Self::Unsupported => item.safety == PublisherActionSafety::Unsupported,
        }
    }
}

#[derive(Default)]
pub(crate) struct PublisherProfilePageState {
    pub(crate) target: Option<PublisherFrontend>,
    pub(crate) destination_root: String,
    /// Supplied by the existing Playing Library flow - this page never
    /// builds one itself.
    pub(crate) source_plan: Option<PlayingLibraryPlan>,
    pub(crate) canonical_platform_id: String,
    pub(crate) result: Option<PublisherPlan>,
    pub(crate) error: Option<String>,
    pub(crate) filter: PublisherResultFilter,
    pub(crate) advanced: bool,
    pub(crate) selected_item: Option<usize>,
    pub(crate) preview_generation: u64,
    pub(crate) link_mode: Option<PublisherLinkMode>,
    pub(crate) execution_transaction: Option<PublisherTransaction>,
    pub(crate) execution_stage: PublisherExecutionStage,
    pub(crate) confirmation_text: String,
    pub(crate) rollback_confirmation_text: String,
    pub(crate) execution_result: Option<PublisherExecutionResult>,
    pub(crate) execution_error: Option<String>,
    preview_target: Option<PublisherFrontend>,
    preview_platform_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PublisherExecutionStage {
    #[default]
    Preview,
    Review,
    Confirming,
    Applying,
    Applied,
    Failed,
    RolledBack,
    NeedsReconciliation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublisherExecutionResult {
    pub(crate) mode: PublisherLinkMode,
    pub(crate) summary: archivefs_core::dat::rename_apply::TransactionSummary,
    pub(crate) transaction_id: String,
    pub(crate) directories_created: usize,
    pub(crate) operations_created: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublisherProfilePageAction {
    OpenLibraryOrganisation,
}

impl PublisherProfilePageState {
    pub(crate) fn set_source_plan(&mut self, source_plan: Option<PlayingLibraryPlan>) {
        if self.source_plan != source_plan {
            self.source_plan = source_plan;
            self.result = None;
            self.execution_transaction = None;
            self.execution_result = None;
            self.execution_error = None;
            self.execution_stage = PublisherExecutionStage::Preview;
            self.preview_target = None;
            self.preview_platform_id.clear();
        }
    }

    /// Builds the read-only preview. Never called automatically - only
    /// from an explicit "Preview plan" button.
    pub(crate) fn preview(&mut self) {
        self.preview_generation = self.preview_generation.saturating_add(1);
        self.error = None;
        self.result = None;
        self.execution_transaction = None;
        self.execution_result = None;
        self.execution_error = None;
        self.confirmation_text.clear();
        self.rollback_confirmation_text.clear();
        self.execution_stage = PublisherExecutionStage::Preview;
        self.preview_target = None;
        self.preview_platform_id.clear();
        let Some(target) = self.target else {
            self.error = Some("Choose a target profile first.".to_string());
            return;
        };
        let Some(plan) = &self.source_plan else {
            self.error =
                Some("Choose a source (a Playing Library plan) to preview first.".to_string());
            return;
        };
        let destination_root = std::path::PathBuf::from(self.destination_root.trim());
        if !destination_root.is_absolute() {
            self.error = Some("The destination root must be an absolute path.".to_string());
            return;
        }
        let (profile, mapping) = match target {
            PublisherFrontend::RomM => (
                romm_profile(),
                resolve_romm_platform_mapping(
                    &self.canonical_platform_id,
                    &FrontendPlatformMapping::default(),
                    None,
                ),
            ),
            PublisherFrontend::EsDe => (
                es_de_profile(),
                resolve_es_de_platform_mapping(&self.canonical_platform_id),
            ),
        };
        let request = PublisherPlanRequest {
            profile: &profile,
            playing_library_plan: plan,
            platform_mapping: mapping,
            destination_root,
            // The explicitly entered root is also the read-only inspection
            // root. `build_publisher_plan` only checks existing paths; it
            // never creates the root or any parent directories.
            existing_destination_root: Some(std::path::Path::new(self.destination_root.trim())),
        };
        match build_publisher_plan(&request) {
            Ok(result) => {
                self.preview_target = Some(result.frontend);
                self.preview_platform_id = self.canonical_platform_id.clone();
                self.result = Some(result);
                self.execution_stage = PublisherExecutionStage::Review;
            }
            Err(message) => self.error = Some(message),
        }
    }

    pub(crate) fn execution_review(&mut self) {
        self.execution_error = None;
        self.execution_result = None;
        self.execution_transaction = None;
        let Some(plan) = self.result.as_ref() else {
            self.execution_error =
                Some("Create a fresh preview before reviewing execution.".into());
            return;
        };
        let Some(mode) = self.link_mode else {
            self.execution_error = Some("Choose HARDLINK or SYMLINK first.".into());
            return;
        };
        match build_publisher_transaction_with_policy(plan, self.preview_generation, mode) {
            Ok(transaction) => {
                self.execution_transaction = Some(transaction);
                self.execution_stage = PublisherExecutionStage::Confirming;
            }
            Err(error) => {
                self.execution_error = Some(publisher_execution_error_text(&error));
                self.execution_stage = PublisherExecutionStage::Failed;
            }
        }
    }

    pub(crate) fn executable_count(&self) -> usize {
        self.execution_transaction
            .as_ref()
            .map_or(0, |transaction| transaction.transaction.entries.len())
    }

    pub(crate) fn apply_confirmation_phrase(&self) -> Option<String> {
        let count = self.executable_count();
        (count > 0).then(|| format!("PUBLISH {count} ITEMS"))
    }

    pub(crate) fn can_apply(&self) -> bool {
        self.execution_stage == PublisherExecutionStage::Confirming
            && self.link_mode
                == self
                    .execution_transaction
                    .as_ref()
                    .map(|transaction| transaction.link_mode)
            && self
                .apply_confirmation_phrase()
                .is_some_and(|phrase| self.confirmation_text == phrase)
    }

    pub(crate) fn apply(&mut self) {
        let journal_dir =
            archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
                .unwrap_or_else(|_| PathBuf::from("rename-transactions"));
        self.apply_with_journal_dir(&journal_dir);
    }

    pub(crate) fn apply_with_journal_dir(&mut self, journal_dir: &std::path::Path) {
        if !self.can_apply() {
            self.execution_error =
                Some("Type the exact publish confirmation phrase before applying.".into());
            return;
        }
        let Some(plan) = self.result.clone() else {
            self.execution_error = Some("The publisher preview is missing; preview again.".into());
            return;
        };
        if *self.destination_root.trim() != plan.destination_root {
            self.execution_error = Some(
                "Library changed since preview. Review the updated plan before applying.".into(),
            );
            self.execution_stage = PublisherExecutionStage::Failed;
            return;
        }
        let Some(mode) = self.link_mode else {
            self.execution_error = Some("Choose HARDLINK or SYMLINK first.".into());
            return;
        };
        if self.preview_target != self.target
            || self.preview_platform_id != self.canonical_platform_id
            || self.preview_target != Some(plan.frontend)
        {
            self.execution_error = Some(
                "Library changed since preview. Review the updated plan before applying.".into(),
            );
            self.execution_stage = PublisherExecutionStage::Failed;
            return;
        }
        if self
            .execution_transaction
            .as_ref()
            .is_some_and(|transaction| transaction.link_mode != mode)
        {
            self.execution_error =
                Some("The link method changed. Review execution again before applying.".into());
            self.execution_stage = PublisherExecutionStage::Failed;
            return;
        }
        let Some(mut transaction) = self.execution_transaction.take() else {
            self.execution_error = Some("Review the publisher transaction before applying.".into());
            self.execution_stage = PublisherExecutionStage::Failed;
            return;
        };
        self.execution_stage = PublisherExecutionStage::Applying;
        if let Err(error) = std::fs::create_dir_all(journal_dir) {
            self.execution_transaction = Some(transaction);
            self.execution_error = Some(format!("Could not prepare transaction journal: {error}"));
            self.execution_stage = PublisherExecutionStage::Failed;
            return;
        }
        let mut trusted_roots = vec![plan.destination_root.clone()];
        for operation in &plan.items {
            if let Some(parent) = operation.source_path.parent() {
                trusted_roots.push(parent.to_path_buf());
            }
            for companion in &operation.companions {
                if let Some(parent) = companion.source_path.parent() {
                    trusted_roots.push(parent.to_path_buf());
                }
            }
        }
        let cancel = std::sync::atomic::AtomicBool::new(false);
        match apply_publisher_transaction(
            &mut transaction,
            self.preview_generation,
            archivefs_core::safe_read::TrustedRoots::from_paths(trusted_roots),
            journal_dir,
            &cancel,
        ) {
            Ok(outcome) => self.record_success(transaction, outcome),
            Err(error) => {
                let state = transaction.transaction.state;
                self.execution_transaction = Some(transaction);
                self.execution_error = Some(format_apply_error(&error, state));
                self.execution_stage = if matches!(state, TransactionState::ApplyFailed)
                    && self
                        .execution_transaction
                        .as_ref()
                        .is_some_and(|transaction| transaction.transaction.applied_count() > 0)
                {
                    PublisherExecutionStage::NeedsReconciliation
                } else {
                    PublisherExecutionStage::Failed
                };
            }
        }
    }

    fn record_success(&mut self, transaction: PublisherTransaction, outcome: ApplyOutcome) {
        self.execution_result = Some(PublisherExecutionResult {
            mode: self.link_mode.expect("execution mode selected for apply"),
            summary: outcome.summary.clone(),
            transaction_id: outcome.transaction.transaction_id.clone(),
            directories_created: transaction.transaction.created_directories.len(),
            operations_created: outcome.summary.applied,
        });
        self.execution_transaction = Some(transaction);
        self.execution_stage = PublisherExecutionStage::Applied;
        self.execution_error = None;
    }

    pub(crate) fn rollback_confirmation_phrase(&self) -> Option<String> {
        self.execution_result
            .as_ref()
            .map(|result| format!("ROLL BACK {} ITEMS", result.operations_created))
    }

    pub(crate) fn can_rollback(&self) -> bool {
        self.execution_stage == PublisherExecutionStage::Applied
            && self
                .execution_transaction
                .as_ref()
                .is_some_and(|transaction| transaction.transaction.is_rollbackable())
            && self
                .rollback_confirmation_phrase()
                .is_some_and(|phrase| self.rollback_confirmation_text == phrase)
    }

    pub(crate) fn rollback(&mut self) {
        let journal_dir =
            archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
                .unwrap_or_else(|_| PathBuf::from("rename-transactions"));
        self.rollback_with_journal_dir(&journal_dir);
    }

    pub(crate) fn rollback_with_journal_dir(&mut self, journal_dir: &std::path::Path) {
        if !self.can_rollback() {
            self.execution_error =
                Some("Type the exact rollback confirmation phrase before rolling back.".into());
            return;
        }
        let Some(transaction) = self.execution_transaction.as_mut() else {
            return;
        };
        let mut trusted_roots = vec![PathBuf::from(&transaction.transaction.source_scan_root)];
        for entry in &transaction.transaction.entries {
            if let Some(parent) = entry.source_path.parent() {
                trusted_roots.push(parent.to_path_buf());
            }
        }
        let trusted = archivefs_core::safe_read::TrustedRoots::from_paths(trusted_roots);
        match rollback_publisher_transaction(
            transaction,
            journal_dir,
            &std::sync::atomic::AtomicBool::new(false),
            &trusted,
        ) {
            Ok(outcome)
                if matches!(
                    outcome.rollback.result,
                    archivefs_core::dat::rename_apply::RollbackResult::FullyRolledBack
                ) =>
            {
                self.execution_stage = PublisherExecutionStage::RolledBack;
                self.rollback_confirmation_text.clear();
                self.execution_error = None;
            }
            Ok(outcome) => {
                self.execution_stage = PublisherExecutionStage::NeedsReconciliation;
                self.execution_error = Some(format!(
                    "Rollback was not complete: {:?}. Review transaction {}.",
                    outcome.rollback.result, transaction.transaction.transaction_id
                ));
            }
            Err(error) => {
                self.execution_stage = PublisherExecutionStage::NeedsReconciliation;
                self.execution_error = Some(format!("Rollback needs reconciliation: {error}"));
            }
        }
    }
}

fn publisher_execution_error_text(
    error: &archivefs_core::publisher_profile::PublisherExecutionError,
) -> String {
    match error {
        archivefs_core::publisher_profile::PublisherExecutionError::StalePlan { .. } => {
            "Library changed since preview. Review the updated plan before applying.".into()
        }
        archivefs_core::publisher_profile::PublisherExecutionError::DestinationChanged {
            ..
        }
        | archivefs_core::publisher_profile::PublisherExecutionError::Collision { .. }
        | archivefs_core::publisher_profile::PublisherExecutionError::SourceInvalid { .. } => {
            "Library changed since preview. Review the updated plan before applying.".into()
        }
        archivefs_core::publisher_profile::PublisherExecutionError::HardlinkUnavailable {
            detail,
            ..
        } => format!("HARDLINK is unavailable: {detail}"),
        other => format!("Publisher execution review blocked: {other:?}"),
    }
}

fn format_apply_error(error: &ApplyError, state: TransactionState) -> String {
    format!("Publisher execution {state:?}: {error:?}")
}

/// Renders the Publisher / Frontend Library planning and explicitly confirmed
/// execution view. Filesystem mutation remains behind the shared transaction
/// adapter and typed confirmation path.
pub(crate) fn show_publisher_profile_page(
    ui: &mut egui::Ui,
    state: &mut PublisherProfilePageState,
) -> Option<PublisherProfilePageAction> {
    let mut action = None;
    let execution_complete = matches!(
        state.execution_stage,
        PublisherExecutionStage::Applied | PublisherExecutionStage::RolledBack
    );
    widgets::section_header(
        ui,
        "Publisher / Frontend Library",
        Some(if execution_complete {
            "Review the current publisher transaction and its result."
        } else {
            "Preview how your library would look for another app. Nothing is changed here."
        }),
    );
    if !execution_complete {
        ui.label(
            egui::RichText::new("PREVIEW ONLY — nothing will be changed.")
                .strong()
                .color(theme::muted(ui)),
        );
    }
    ui.add_space(8.0);

    widgets::card(ui, |ui| {
        ui.label("Choose what you want to create:");
        ui.horizontal(|ui| {
            if ui
                .selectable_label(
                    state.target == Some(PublisherFrontend::RomM),
                    PublisherFrontend::RomM.plain_language_goal(),
                )
                .clicked()
            {
                state.target = Some(PublisherFrontend::RomM);
            }
            if ui
                .selectable_label(
                    state.target == Some(PublisherFrontend::EsDe),
                    PublisherFrontend::EsDe.plain_language_goal(),
                )
                .clicked()
            {
                state.target = Some(PublisherFrontend::EsDe);
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Destination folder:");
            ui.text_edit_singleline(&mut state.destination_root);
        });
        ui.horizontal(|ui| {
            ui.label("Platform (canonical id):");
            ui.text_edit_singleline(&mut state.canonical_platform_id);
        });
        if state.source_plan.is_none() {
            ui.label(
                egui::RichText::new(
                    "No Playing Library plan selected yet - build one first on the Playing \
                     Library page.",
                )
                .color(theme::muted(ui)),
            );
            if ui.button("Open Library Organisation").clicked() {
                action = Some(PublisherProfilePageAction::OpenLibraryOrganisation);
            }
        }
        if ui.button("Preview plan").clicked() {
            state.preview();
        }
        if let Some(error) = &state.error {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
        }
    });

    let Some(result) = state.result.clone() else {
        return action;
    };

    ui.add_space(10.0);
    let mut request_execution_review = false;
    let summary = result.summary();
    widgets::card(ui, |ui| {
        ui.label(format!("Target: {}", result.frontend.label()));
        ui.label(format!(
            "Destination root: {}",
            result.destination_root.display()
        ));
        ui.label(format!(
            "Playing Library / 1G1R items: {}",
            summary.source_items
        ));
        ui.label(format!("Source items: {}", summary.source_items));
        ui.label(format!("Will publish: {}", summary.will_publish));
        ui.label(format!("Already present: {}", summary.already_present));
        ui.label(format!("Review required: {}", summary.review_required));
        ui.label(format!("Blocked: {}", summary.blocked));
        ui.label(format!("Unsupported: {}", summary.unsupported));
        ui.label(format!(
            "Planned future actions: {} symlinks, {} hardlinks, {} copies",
            summary.planned_symlinks, summary.planned_hardlinks, summary.planned_copies
        ));
    });

    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Execution review").strong());
        ui.label("Choose how the published entries should point to your source library.");
        ui.radio_value(
            &mut state.link_mode,
            Some(PublisherLinkMode::Hardlink),
            "HARDLINK",
        );
        ui.label("Uses almost no extra storage. Best on the same filesystem: both names refer to the same file data.");
        ui.radio_value(
            &mut state.link_mode,
            Some(PublisherLinkMode::Symlink),
            "SYMLINK",
        );
        ui.label("Uses almost no extra storage and can work across filesystems. The published entry points back to the source file.");
        ui.label(
            "Game data is not duplicated; filesystem entries and link metadata are still created.",
        );
        if ui
            .add_enabled(
                summary.will_publish > 0 && state.link_mode.is_some(),
                egui::Button::new("Review execution"),
            )
            .clicked()
        {
            request_execution_review = true;
        }
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for candidate in PublisherResultFilter::ALL {
            if ui
                .selectable_label(state.filter == candidate, candidate.label())
                .clicked()
            {
                state.filter = candidate;
            }
        }
        ui.checkbox(&mut state.advanced, "Advanced");
    });

    ui.add_space(6.0);
    let filtered: Vec<(usize, &PublisherPlanItem)> = result
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| state.filter.matches(item))
        .collect();
    for (index, item) in &filtered {
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new(&item.dat_entry_name).strong());
            ui.label(&item.reason);
            if state.advanced {
                ui.label(format!(
                    "canonical platform: {}",
                    item.platform_mapping.canonical_platform_id()
                ));
                if let Some(folder) = item.platform_mapping.folder() {
                    ui.label(format!("target folder: {folder}"));
                }
                ui.label(format!("action: {:?}", item.planned_action.kind));
                ui.label(format!("safety: {:?}", item.safety));
                if let Some(destination) = &item.planned_destination {
                    ui.label(format!("destination path: {}", destination.display()));
                }
            }
            if ui
                .selectable_label(state.selected_item == Some(*index), "Details")
                .clicked()
            {
                state.selected_item = Some(*index);
            }
        });
    }
    if filtered.is_empty() {
        ui.label(egui::RichText::new("No items match this filter.").color(theme::muted(ui)));
    }

    if let Some(index) = state.selected_item
        && let Some(item) = result.items.get(index)
    {
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new("Item details").strong());
            ui.label(format!("Title: {}", item.dat_entry_name));
            ui.label(format!(
                "Canonical platform: {}",
                item.platform_mapping.canonical_platform_id()
            ));
            ui.label(format!("Target profile: {}", result.frontend.label()));
            ui.label(format!("Source path: {}", item.source_path.display()));
            ui.label(format!(
                "Destination path: {}",
                item.planned_destination.as_deref().map_or_else(
                    || "Not available".to_string(),
                    |path| path.display().to_string()
                )
            ));
            ui.label(format!("Planned action: {:?}", item.planned_action.kind));
            ui.label(format!("Action safety: {:?}", item.safety));
            ui.label(format!("Destination state: {:?}", item.destination_state));
            ui.label(format!("Mapping: {:?}", item.platform_mapping));
            ui.label(format!(
                "Media-set state: {} companion item(s)",
                item.companions.len()
            ));
            if !item.warnings.is_empty() {
                ui.label(format!("Warnings: {:?}", item.warnings));
            }
            if !item.conflicts.is_empty() {
                ui.label(format!("Conflicts: {:?}", item.conflicts));
            }
            ui.label(format!("Reason: {}", item.reason));
        });
    }

    if request_execution_review {
        state.execution_review();
    }

    let mut request_apply = false;
    let mut request_rollback = false;
    if let Some(transaction) = state.execution_transaction.clone() {
        ui.add_space(10.0);
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new("Ready to publish").strong());
            ui.label("PREVIEW REVIEW COMPLETE — nothing has been changed yet.");
            ui.label(format!("Mode: {:?}", transaction.link_mode));
            ui.label(format!(
                "File/link operations: {}",
                transaction.transaction.entries.len()
            ));
            ui.label(format!(
                "Destination directories: {} planned ({} already exist)",
                transaction.directories.len(),
                transaction
                    .directories
                    .iter()
                    .filter(|directory| {
                        directory.state
                            == archivefs_core::publisher_profile::PublisherDirectoryState::PreExisting
                    })
                    .count()
            ));
            ui.label(format!(
                "Multi-file companion sets: {} companion file(s)",
                result
                    .items
                    .iter()
                    .map(|item| item.companions.len())
                    .sum::<usize>()
            ));
            ui.label("The source library will not be renamed or deleted. Rollback removes only transaction-created destination links and empty directories.");

            match state.execution_stage {
                PublisherExecutionStage::Confirming => {
                    let phrase = state.apply_confirmation_phrase().unwrap_or_default();
                    ui.label(format!("Type {phrase} to confirm publishing."));
                    ui.text_edit_singleline(&mut state.confirmation_text);
                    if ui
                        .add_enabled(state.can_apply(), egui::Button::new("Apply publisher plan"))
                        .clicked()
                    {
                        request_apply = true;
                    }
                }
                PublisherExecutionStage::Applied => {
                    if let Some(outcome) = &state.execution_result {
                        ui.label(format!("Published {} item(s).", outcome.summary.applied));
                        ui.label(format!(
                            "Already present/no-op: {}.",
                            summary.already_present
                        ));
                        ui.label(format!(
                            "Skipped/non-executable: {}.",
                            summary.review_required + summary.blocked + summary.unsupported
                        ));
                        ui.label(format!(
                            "Warnings: {}.",
                            result
                                .items
                                .iter()
                                .map(|item| item.warnings.len())
                                .sum::<usize>()
                        ));
                        ui.label(format!(
                            "Created {} destination director{}.",
                            outcome.directories_created,
                            if outcome.directories_created == 1 {
                                "y"
                            } else {
                                "ies"
                            }
                        ));
                        ui.label(format!("Transaction: {}", outcome.transaction_id));
                        match outcome.mode {
                            PublisherLinkMode::Hardlink => {
                                ui.label("Published without duplicating the game data.");
                            }
                            PublisherLinkMode::Symlink => {
                                ui.label("Published as links to the source library.");
                            }
                        }
                        if let Some(phrase) = state.rollback_confirmation_phrase() {
                            ui.label(format!(
                                "Type {phrase} to remove the created destination entries."
                            ));
                            ui.text_edit_singleline(&mut state.rollback_confirmation_text);
                            if ui
                                .add_enabled(
                                    state.can_rollback(),
                                    egui::Button::new("Roll back publisher transaction"),
                                )
                                .clicked()
                            {
                                request_rollback = true;
                            }
                        }
                    }
                }
                PublisherExecutionStage::Applying => {
                    ui.label("Applying transaction…");
                }
                PublisherExecutionStage::RolledBack => {
                    ui.label("Publisher transaction rolled back. Source files and pre-existing destination data remain untouched.");
                }
                PublisherExecutionStage::Failed | PublisherExecutionStage::NeedsReconciliation => {
                    ui.label(match state.execution_stage {
                        PublisherExecutionStage::Failed => {
                            "Publishing failed; nothing is claimed as successful."
                        }
                        _ => "Publishing needs reconciliation; do not retry this transaction here.",
                    });
                }
                PublisherExecutionStage::Preview | PublisherExecutionStage::Review => {}
            }
            if let Some(error) = &state.execution_error {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
            }
        });
    }
    if state.execution_transaction.is_none()
        && let Some(error) = &state.execution_error
    {
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new("Publishing unavailable").strong());
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
            ui.label(
                "Refresh the preview after correcting the issue. No destination changes were made.",
            );
        });
    }
    if request_apply {
        state.apply();
    }
    if request_rollback {
        state.rollback();
    }

    action
}

#[cfg(test)]
mod tests;

impl ArchiveFsApp {
    /// Draws the read-only publisher projection page. The source is only the
    /// already-built Playing Library plan from Library Organisation; this
    /// page never scans or re-elects games itself.
    pub(crate) fn show_publisher_profile_page(&mut self, ui: &mut egui::Ui) {
        let source_plan = self
            .rom_organisation_page
            .as_ref()
            .and_then(|page| page.playing_library.plan().cloned());
        let page = self
            .publisher_profile_page
            .get_or_insert_with(publisher_profile_page::PublisherProfilePageState::default);
        page.set_source_plan(source_plan);
        if let Some(action) = publisher_profile_page::show_publisher_profile_page(ui, page) {
            match action {
                publisher_profile_page::PublisherProfilePageAction::OpenLibraryOrganisation => {
                    self.navigate_to_main_view(MainView::CanonicalOrganisation)
                }
            }
        }
    }
}
