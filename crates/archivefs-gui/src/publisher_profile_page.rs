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
    worker: Option<std::sync::mpsc::Receiver<Self>>,
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
    /// The run finished but recorded no change: every item was skipped.
    NothingPublished,
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

#[derive(Clone, Copy)]
pub(crate) enum PublisherJob {
    Preview,
    Review,
    Apply,
    Rollback,
}

impl PublisherProfilePageState {
    pub(crate) fn is_busy(&self) -> bool {
        self.worker.is_some()
    }

    pub(crate) fn poll(&mut self, context: &egui::Context) {
        let Some(worker) = &self.worker else { return };
        match worker.try_recv() {
            Ok(completed) => *self = completed,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                context.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.worker = None;
                self.execution_stage = PublisherExecutionStage::NeedsReconciliation;
                self.execution_error = Some("Export worker stopped. Inspect the transaction journals in Repair History before retrying.".into());
            }
        }
    }

    pub(crate) fn start(&mut self, job: PublisherJob, context: egui::Context) {
        if self.is_busy() {
            return;
        }
        let mut state = std::mem::take(self);
        let (sender, receiver) = std::sync::mpsc::channel();
        self.worker = Some(receiver);
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                PublisherJob::Preview => state.preview(),
                PublisherJob::Review => state.execution_review(),
                PublisherJob::Apply => state.apply(),
                PublisherJob::Rollback => state.rollback(),
            }));
            if outcome.is_err() {
                state.execution_stage = PublisherExecutionStage::NeedsReconciliation;
                state.execution_error = Some("Export worker stopped unexpectedly. Inspect the transaction journals in Repair History before retrying.".into());
            }
            let _ = sender.send(state);
            context.request_repaint();
        });
    }

    /// Accepted write outcomes belong to the export until the user explicitly
    /// chooses another source preview, including after leaving either GUI.
    pub(crate) fn sync_source_plan(
        &mut self,
        ui: &mut egui::Ui,
        source: Option<&PlayingLibraryPlan>,
    ) {
        if self.is_busy() || self.source_plan.as_ref() == source {
            return;
        }
        let has_outcome = matches!(
            self.execution_stage,
            PublisherExecutionStage::Applied
                | PublisherExecutionStage::NothingPublished
                | PublisherExecutionStage::Failed
                | PublisherExecutionStage::NeedsReconciliation
                | PublisherExecutionStage::RolledBack
        );
        if has_outcome {
            ui.label("The Playing Library preview has changed. Your export result and supported rollback remain below.");
            if ui.button("Use current Playing Library preview").clicked() {
                self.set_source_plan(source.cloned());
            }
        } else {
            self.set_source_plan(source.cloned());
        }
    }

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
        self.confirmation_text.clear();
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
            && self.result.as_ref().is_some_and(|plan| {
                plan.destination_root == std::path::Path::new(self.destination_root.trim())
                    && self.preview_target == self.target
                    && self.preview_platform_id == self.canonical_platform_id
            })
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
        match archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir() {
            Ok(journal_dir) => self.apply_with_journal_dir(&journal_dir),
            Err(error) => {
                self.execution_error = Some(format!(
                    "Cannot locate transaction journals: {error}. Correct the EmuWiz data location and preview again."
                ));
                self.execution_stage = PublisherExecutionStage::Failed;
            }
        }
    }

    pub(crate) fn apply_with_journal_dir(&mut self, journal_dir: &std::path::Path) {
        if self.result.as_ref().is_some_and(|plan| {
            plan.destination_root != std::path::Path::new(self.destination_root.trim())
                || self.preview_target != self.target
                || self.preview_platform_id != self.canonical_platform_id
        }) {
            self.execution_error = Some(
                "Library changed since preview. Review the updated plan before applying.".into(),
            );
            self.execution_stage = PublisherExecutionStage::Failed;
            return;
        }
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
                self.execution_stage = stage_after_apply_error(
                    &self
                        .execution_transaction
                        .as_ref()
                        .expect("just set")
                        .transaction,
                );
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
        // `Ok(outcome)` only means the executor returned; a batch with in-flight,
        // failed or not-attempted entries is NOT a published library.
        let summary = &self.execution_result.as_ref().expect("just set").summary;
        self.execution_stage = execution_stage_for(summary, &transaction.transaction);
        self.execution_error = incomplete_publish_message(summary, &transaction.transaction);
        self.execution_transaction = Some(transaction);
    }

    /// Items the journal says rollback can still act on: applied, plus in-flight
    /// entries it reconciles against the disk. A failed rollback is not counted
    /// because links are never retried. A run that needs reconciliation keeps
    /// this route: rollback re-verifies the journal and fences writes itself.
    fn reversible_item_count(&self) -> usize {
        let settled = matches!(
            self.execution_stage,
            PublisherExecutionStage::Applied | PublisherExecutionStage::NeedsReconciliation
        );
        self.execution_transaction
            .as_ref()
            .filter(|transaction| settled && transaction.transaction.is_rollbackable())
            .map_or(0, |transaction| {
                transaction.transaction.applied_count() + in_flight_count(&transaction.transaction)
            })
    }

    pub(crate) fn rollback_confirmation_phrase(&self) -> Option<String> {
        let count = self.reversible_item_count();
        (count > 0).then(|| format!("ROLL BACK {count} ITEMS"))
    }

    pub(crate) fn can_rollback(&self) -> bool {
        self.rollback_confirmation_phrase()
            .is_some_and(|phrase| self.rollback_confirmation_text == phrase)
    }

    pub(crate) fn rollback(&mut self) {
        match archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir() {
            Ok(journal_dir) => self.rollback_with_journal_dir(&journal_dir),
            Err(error) => {
                self.execution_error = Some(format!(
                    "Cannot locate transaction journals: {error}. Correct the EmuWiz data location before retrying Undo."
                ));
            }
        }
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
                let remaining = destination_left_after_rollback(&transaction.transaction, &outcome);
                self.rollback_confirmation_text.clear();
                if remaining.is_empty() {
                    self.execution_stage = PublisherExecutionStage::RolledBack;
                    self.execution_error = None;
                } else {
                    self.execution_stage = PublisherExecutionStage::NeedsReconciliation;
                    self.execution_error = Some(format!(
                        "Rollback finished, but these destination paths remain: {}. Review transaction {}.",
                        remaining.join(", "),
                        transaction.transaction.transaction_id
                    ));
                }
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

/// Entries whose completion was never recorded.
fn in_flight_count(transaction: &archivefs_core::dat::rename_apply::RenameTransaction) -> usize {
    use archivefs_core::dat::rename_apply::EntryState;
    transaction
        .entries
        .iter()
        .filter(|entry| matches!(entry.state, EntryState::Applying | EntryState::RollingBack))
        .count()
}

/// What a rollback reported as complete did not remove: created directories
/// holding anything beyond what rollback recorded, and links from entries
/// recorded as failed, which rollback does not touch but which can exist when
/// verification failed after the link was created.
fn destination_left_after_rollback(
    transaction: &archivefs_core::dat::rename_apply::RenameTransaction,
    outcome: &archivefs_core::platform_evidence_fusion::plan_transaction::PlanRollbackOutcome,
) -> Vec<String> {
    use archivefs_core::dat::rename_apply::EntryState;
    let tombstones: std::collections::BTreeSet<PathBuf> = transaction
        .entries
        .iter()
        .filter_map(|entry| entry.unknown.get("emuwiz_link_undo_tombstone_v1"))
        .filter_map(|value| value.as_str().map(PathBuf::from))
        .collect();
    transaction
        .entries
        .iter()
        .filter(|entry| {
            entry.state == EntryState::ApplyFailed
                && std::fs::symlink_metadata(&entry.destination_path).is_ok()
        })
        .map(|entry| &entry.destination_path)
        .chain(
            outcome.directories_remaining.iter().filter(|directory| {
                !holds_only_rollback_records(directory, &tombstones, transaction)
            }),
        )
        .map(|path| path.display().to_string())
        .collect()
}

/// Rollback parks each removed symlink as a tombstone it records in the
/// journal, so a created directory can survive a clean rollback. Only exact
/// recorded tombstones and created directories holding nothing else are
/// rollback's own. An empty or unreadable directory is real leftover.
fn holds_only_rollback_records(
    directory: &std::path::Path,
    tombstones: &std::collections::BTreeSet<PathBuf>,
    transaction: &archivefs_core::dat::rename_apply::RenameTransaction,
) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    let mut held = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let path = entry.path();
        let ours = tombstones.contains(&path)
            || (transaction.created_directories.contains(&path)
                && holds_only_rollback_records(&path, tombstones, transaction));
        if !ours {
            return false;
        }
        held += 1;
    }
    held > 0
}

/// Whether the journalled record shows the destination may have changed: an
/// entry is applied or in flight, or the run created a directory.
fn destination_may_have_changed(
    transaction: &archivefs_core::dat::rename_apply::RenameTransaction,
) -> bool {
    !transaction.created_directories.is_empty()
        || transaction
            .entries
            .iter()
            .any(archivefs_core::dat::rename_apply::TransactionEntry::blocks_full_rollback)
}

/// Applied only for a fully resolved batch that applied something and can
/// still be rolled back. A resolved batch that applied nothing changed
/// nothing, unless the journal says it did.
pub(crate) fn execution_stage_for(
    summary: &archivefs_core::dat::rename_apply::TransactionSummary,
    transaction: &archivefs_core::dat::rename_apply::RenameTransaction,
) -> PublisherExecutionStage {
    if !summary.fully_resolved() {
        PublisherExecutionStage::NeedsReconciliation
    } else if summary.applied > 0 && transaction.is_rollbackable() {
        PublisherExecutionStage::Applied
    } else if summary.applied > 0 {
        PublisherExecutionStage::NeedsReconciliation
    } else if summary.rolled_back > 0 && transaction.is_fully_rolled_back() {
        PublisherExecutionStage::RolledBack
    } else if summary.rolled_back > 0 || destination_may_have_changed(transaction) {
        PublisherExecutionStage::NeedsReconciliation
    } else {
        PublisherExecutionStage::NothingPublished
    }
}

/// A refused or failed Apply is only a plain failure when the journal records
/// no possible destination change.
fn stage_after_apply_error(
    transaction: &archivefs_core::dat::rename_apply::RenameTransaction,
) -> PublisherExecutionStage {
    if destination_may_have_changed(transaction) {
        PublisherExecutionStage::NeedsReconciliation
    } else {
        PublisherExecutionStage::Failed
    }
}

/// What this run did, for a batch that published at least one item.
fn published_outcome_line(
    summary: &archivefs_core::dat::rename_apply::TransactionSummary,
) -> String {
    if summary.skipped == 0 {
        format!("Published {} item(s).", summary.applied)
    } else {
        format!(
            "Partly published: {} published, {} skipped and left untouched.",
            summary.applied, summary.skipped
        )
    }
}

/// The result card's heading: "Ready" only while nothing has been attempted.
fn execution_card_title(
    stage: PublisherExecutionStage,
    summary: Option<&archivefs_core::dat::rename_apply::TransactionSummary>,
) -> &'static str {
    match stage {
        PublisherExecutionStage::Preview
        | PublisherExecutionStage::Review
        | PublisherExecutionStage::Confirming => "Ready to publish",
        PublisherExecutionStage::Applying => "Publishing",
        PublisherExecutionStage::Applied if summary.is_some_and(|s| s.skipped > 0) => {
            "Export partly published"
        }
        PublisherExecutionStage::Applied => "Export published",
        PublisherExecutionStage::NothingPublished => "Nothing published",
        PublisherExecutionStage::Failed => "Publishing failed",
        PublisherExecutionStage::RolledBack => "Export rolled back",
        PublisherExecutionStage::NeedsReconciliation => "Publishing needs reconciliation",
    }
}

/// The actionable explanation for an incomplete batch (None when complete).
pub(crate) fn incomplete_publish_message(
    summary: &archivefs_core::dat::rename_apply::TransactionSummary,
    transaction: &archivefs_core::dat::rename_apply::RenameTransaction,
) -> Option<String> {
    if summary.fully_resolved() {
        let unsettled = execution_stage_for(summary, transaction)
            == PublisherExecutionStage::NeedsReconciliation;
        return unsettled.then(|| {
            format!(
                "Publishing is NOT settled: the record shows {} applied and {} destination director(ies) created, and the destination may have changed. Review transaction {}.",
                summary.applied,
                transaction.created_directories.len(),
                transaction.transaction_id
            )
        });
    }
    let mut message = format!(
        "Publishing is NOT complete: {} applied, {} failed, {} unresolved (in flight), {} not attempted.",
        summary.applied, summary.failed, summary.unresolved, summary.pending
    );
    for entry in &summary.unresolved_entries {
        message.push_str(&format!(
            " Recovery needed for {} ({}): {}",
            entry.destination_path.display(),
            entry.stage,
            entry.required_action
        ));
    }
    Some(message)
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
    state.poll(ui.ctx());
    if state.is_busy() {
        ui.spinner();
        ui.label(
            "Working on the frontend export. Leaving this page does not cancel accepted work.",
        );
        return None;
    }
    let mut action = None;
    let execution_complete = matches!(
        state.execution_stage,
        PublisherExecutionStage::Applied | PublisherExecutionStage::RolledBack
    );
    widgets::section_header(
        ui,
        "Frontend export profiles",
        Some(if execution_complete {
            "Review your export and its result."
        } else {
            "Choose an output and destination, preview, then review before applying."
        }),
    );
    if !execution_complete {
        ui.label(
            egui::RichText::new(
                "Preview first. Applying requires a separate review and confirmation.",
            )
            .strong()
            .color(theme::muted(ui)),
        );
    }
    ui.add_space(8.0);

    widgets::card(ui, |ui| {
        ui.label("Choose what you want to create:");
        ui.horizontal_wrapped(|ui| {
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
        widgets::folder_picker(ui, "Destination folder", &mut state.destination_root);
        ui.horizontal(|ui| {
            ui.label("System:");
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
            if ui.button("Choose games in Build Libraries").clicked() {
                action = Some(PublisherProfilePageAction::OpenLibraryOrganisation);
            }
        }
        ui.label("Supported profiles: RomM and ES-DE link layouts. Copies, other frontends, configuration generation and artwork export are unavailable.");
        ui.label("Existing gamelists, configuration and artwork are preserved. This profile only creates missing game links; it never replaces existing files.");
        if ui.button("Preview plan").clicked() {
            state.start(PublisherJob::Preview, ui.ctx().clone());
        }
        if let Some(error) = &state.error {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
        }
    });

    let Some(result) = state.result.clone() else {
        if let Some(error) = &state.execution_error {
            ui.colored_label(theme::AMBER, error);
        }
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
            "Planner actions: {} symlinks, {} hardlinks, {} copies (copy execution unavailable)",
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
                summary.will_publish > 0
                    && state.link_mode.is_some()
                    && matches!(
                        state.execution_stage,
                        PublisherExecutionStage::Review | PublisherExecutionStage::Confirming
                    ),
                egui::Button::new("Review execution"),
            )
            .clicked()
        {
            request_execution_review = true;
        }
    });

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
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
            if let Some(destination) = &item.planned_destination {
                ui.label(format!("Planned file: {}", destination.display()));
            }
            for companion in &item.companions {
                ui.label(format!(
                    "Companion file: {}",
                    companion.planned_destination.display()
                ));
            }
            ui.label(format!(
                "Existing destination: {:?}; safety: {:?}",
                item.destination_state, item.safety
            ));
            if !item.conflicts.is_empty() {
                ui.label(format!(
                    "Conflicts (will not replace): {:?}",
                    item.conflicts
                ));
            }
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
        state.start(PublisherJob::Review, ui.ctx().clone());
        return action;
    }

    let mut request_apply = false;
    let mut request_rollback = false;
    if let Some(transaction) = state.execution_transaction.clone() {
        ui.add_space(10.0);
        widgets::card(ui, |ui| {
            let title = execution_card_title(
                state.execution_stage,
                state
                    .execution_result
                    .as_ref()
                    .map(|result| &result.summary),
            );
            ui.label(egui::RichText::new(title).strong());
            if !matches!(
                state.execution_stage,
                PublisherExecutionStage::Preview
                    | PublisherExecutionStage::Review
                    | PublisherExecutionStage::Confirming
                    | PublisherExecutionStage::Applying
            ) {
                let summary = state
                    .execution_result
                    .as_ref()
                    .map(|result| result.summary.clone())
                    .unwrap_or_else(|| {
                        archivefs_core::dat::rename_apply::TransactionSummary::from_transaction(
                            &transaction.transaction,
                        )
                    });
                crate::transaction_outcome::show_with_summary(
                    ui,
                    &summary,
                    &transaction.transaction,
                    state.rollback_confirmation_phrase().is_some(),
                );
            }
            if state.execution_stage == PublisherExecutionStage::Confirming {
                ui.label("Preview reviewed. Nothing has been changed yet.");
            }
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
                    let label = ui.label(format!("Type {phrase} to confirm publishing."));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.confirmation_text)
                            .id_salt("publisher_apply_confirmation"),
                    )
                    .labelled_by(label.id);
                    let apply = ui
                        .add_enabled(state.can_apply(), egui::Button::new("Apply publisher plan"));
                    if apply.gained_focus() {
                        apply.scroll_to_me(Some(egui::Align::Center));
                    }
                    if apply.clicked() {
                        request_apply = true;
                    }
                }
                PublisherExecutionStage::Applied => {
                    if let Some(outcome) = &state.execution_result {
                        ui.label(published_outcome_line(&outcome.summary));
                        ui.label(format!(
                            "Already present/no-op: {}.",
                            summary.already_present
                        ));
                        ui.label(format!(
                            "Not part of this export (review, blocked or unsupported): {}.",
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
                    }
                }
                PublisherExecutionStage::NothingPublished => {
                    let skipped = state
                        .execution_result
                        .as_ref()
                        .map_or(0, |result| result.summary.skipped);
                    ui.label(format!(
                        "Nothing was published by this Apply: {skipped} item(s) were skipped and none were applied. It left nothing to roll back."
                    ));
                }
                PublisherExecutionStage::Applying => {
                    ui.label("Applying transaction…");
                }
                PublisherExecutionStage::RolledBack => {
                    ui.label("Publisher transaction rolled back. Source files and pre-existing destination data remain untouched.");
                }
                PublisherExecutionStage::Failed => {
                    ui.label("Publishing failed; nothing is claimed as successful.");
                }
                PublisherExecutionStage::NeedsReconciliation => {
                    ui.label(
                        "Publishing needs reconciliation; do not retry this transaction here.",
                    );
                    let record = &transaction.transaction;
                    ui.label(format!(
                        "The record shows {} item(s) applied and {} destination director(ies) created.",
                        record.applied_count(),
                        record.created_directories.len()
                    ));
                    if state.rollback_confirmation_phrase().is_none() {
                        ui.label("This page cannot reverse this transaction. Its journal can be reviewed in Repair History, but Undo for links is not available there.");
                    }
                }
                PublisherExecutionStage::Preview | PublisherExecutionStage::Review => {}
            }
            if let Some(phrase) = state.rollback_confirmation_phrase() {
                ui.label(format!(
                    "Type {phrase} to remove the created destination entries."
                ));
                ui.label(
                    "Rollback here is available only until you preview again or close EmuWiz.",
                );
                let in_flight = in_flight_count(&transaction.transaction);
                if in_flight > 0 {
                    ui.label(format!(
                        "{in_flight} of these item(s): completion was not recorded. Rollback checks each against the disk."
                    ));
                }
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
            ui.label(if state.execution_stage == PublisherExecutionStage::NeedsReconciliation {
                "Destination changes may have occurred. Inspect the transaction journals in Repair History before starting another export."
            } else {
                "Refresh the preview after correcting the issue. No destination changes were made."
            });
        });
    }
    if request_apply {
        state.start(PublisherJob::Apply, ui.ctx().clone());
    }
    if request_rollback {
        state.start(PublisherJob::Rollback, ui.ctx().clone());
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
        page.sync_source_plan(ui, source_plan.as_ref());
        if let Some(action) = publisher_profile_page::show_publisher_profile_page(ui, page) {
            match action {
                publisher_profile_page::PublisherProfilePageAction::OpenLibraryOrganisation => {
                    self.navigate_to_main_view(MainView::CanonicalOrganisation)
                }
            }
        }
    }
}
