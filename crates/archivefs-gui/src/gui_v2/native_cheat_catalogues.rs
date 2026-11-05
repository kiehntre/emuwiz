//! Blue "Cheat catalogues" tab: a thin host for Gold's existing cheat
//! catalogue management.
//!
//! * RetroArch (libretro) `.cht` database and Dolphin Gecko catalogue cards -
//!   the same Review-then-Confirm cards, fetch/verify/rebuild/remove jobs and
//!   progress/cancel controls Gold uses (`sources_page`, `cheats_mods::controller`).
//! * Cheat sources: provider on/off, priority, per-platform participation,
//!   health and the BSFree card (`cheat_sources_page`).
//! * Cache maintenance: inventory, verify, pin and a previewed-then-confirmed
//!   prune of the RetroArch snapshot cache, all via `archivefs-core`'s
//!   `cheat_cache_maintenance` (locking, revalidation before every delete).
//!
//! Nothing here downloads on its own: opening the tab performs local reads
//! only, and every network fetch is an explicit confirmed click on a Gold
//! card. Per-game fetching in the Cheats tab is untouched.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use archivefs_core::patch_manager::{
    CachePruneDisposition, CachePruneEntryStatus, CachePruneExecutionResult,
    CachePruneExecutionStatus, CachePrunePlan, CachePrunePolicy, CheatSourceError,
    SnapshotInventoryEntry, SnapshotInventoryReport, SnapshotPinResult, SnapshotPinStatus,
    SnapshotVerificationReport, SnapshotVerificationState, default_cheat_source_cache_root,
    execute_retroarch_cheat_cache_prune, inventory_retroarch_cheat_snapshots,
    plan_retroarch_cheat_cache_prune, set_retroarch_cheat_snapshot_pin,
    verify_retroarch_cheat_snapshots,
};
use eframe::egui;

use super::{Activity, NativeWorkflows, Route, Section};
use crate::ui::components as widgets;
use crate::ui::theme;

/// Fewest snapshots per source the core will ever prune down to.
const MINIMUM_KEEP: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SubTab {
    #[default]
    Catalogues,
    Sources,
    Cache,
}

enum CacheJob {
    Inventory,
    Verify,
    Pin { snapshot_id: String, pinned: bool },
    Plan(CachePrunePolicy),
    Prune(Box<CachePrunePlan>),
}

enum CacheOutcome {
    Inventory(Result<SnapshotInventoryReport, CheatSourceError>),
    Verify(Result<SnapshotVerificationReport, CheatSourceError>),
    Pin(Result<SnapshotPinResult, CheatSourceError>),
    Plan(Result<CachePrunePlan, CheatSourceError>),
    Prune(Result<CachePruneExecutionResult, CheatSourceError>),
}

/// Runs one maintenance operation against `root`. Pure with respect to the UI
/// so tests can drive it against a temporary cache.
fn run_cache_job(root: &std::path::Path, job: CacheJob) -> CacheOutcome {
    match job {
        CacheJob::Inventory => CacheOutcome::Inventory(inventory_retroarch_cheat_snapshots(root)),
        CacheJob::Verify => {
            CacheOutcome::Verify(verify_retroarch_cheat_snapshots(root, None, None))
        }
        CacheJob::Pin {
            snapshot_id,
            pinned,
        } => CacheOutcome::Pin(set_retroarch_cheat_snapshot_pin(root, &snapshot_id, pinned)),
        CacheJob::Plan(policy) => {
            CacheOutcome::Plan(plan_retroarch_cheat_cache_prune(root, &policy))
        }
        CacheJob::Prune(plan) => {
            CacheOutcome::Prune(execute_retroarch_cheat_cache_prune(root, &plan, true))
        }
    }
}

pub(super) struct State {
    pub(super) tab: SubTab,
    /// Test seam: never set in production, where the trusted default cache
    /// root is used.
    cache_root_override: Option<PathBuf>,
    worker: Option<Receiver<CacheOutcome>>,
    inventory: Option<Result<SnapshotInventoryReport, String>>,
    verification: Option<Result<SnapshotVerificationReport, String>>,
    notice: Option<(bool, String)>,
    keep: usize,
    include_staging: bool,
    plan: Option<CachePrunePlan>,
    /// Set after a change so the list is re-read (without clearing the result notice).
    refresh_pending: bool,
    /// Whether a RetroArch retrieval was running last frame.
    retrieving_before: bool,
    // Activity bookkeeping.
    retroarch_job: Option<u64>,
    dolphin_job: Option<u64>,
    cache_job: Option<u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tab: SubTab::default(),
            cache_root_override: None,
            worker: None,
            inventory: None,
            verification: None,
            notice: None,
            keep: MINIMUM_KEEP,
            include_staging: false,
            plan: None,
            refresh_pending: false,
            retrieving_before: false,
            retroarch_job: None,
            dolphin_job: None,
            cache_job: None,
        }
    }
}

impl State {
    fn busy(&self) -> bool {
        self.worker.is_some()
    }

    fn root(&self) -> Result<PathBuf, CheatSourceError> {
        match &self.cache_root_override {
            Some(root) => Ok(root.clone()),
            None => default_cheat_source_cache_root(),
        }
    }

    /// Starts a user-requested job; a new request clears the previous result.
    fn start(&mut self, context: &egui::Context, job: CacheJob) {
        if self.busy() {
            return;
        }
        self.notice = None;
        self.start_job(context, job);
    }

    /// Forgets everything read from the cache: the on-disk state changed.
    fn invalidate(&mut self) {
        self.plan = None;
        self.inventory = None;
        self.verification = None;
    }

    fn start_job(&mut self, context: &egui::Context, job: CacheJob) {
        if self.busy() {
            return;
        }
        let root = match self.root() {
            Ok(root) => root,
            Err(error) => {
                self.notice = Some((false, error.to_string()));
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.worker = Some(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let _ = sender.send(run_cache_job(&root, job));
            context.request_repaint();
        });
    }

    /// Collects a finished cache job. Returns whether anything changed.
    fn poll(&mut self) -> bool {
        let Some(worker) = &self.worker else {
            return false;
        };
        let outcome = match worker.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.worker = None;
                self.invalidate();
                self.notice = Some((
                    false,
                    "The cache check stopped unexpectedly. Saved copies may have been partly changed; list them again to see what is there."
                        .to_string(),
                ));
                return true;
            }
        };
        self.worker = None;
        self.apply(outcome);
        true
    }

    fn apply(&mut self, outcome: CacheOutcome) {
        match outcome {
            CacheOutcome::Inventory(result) => {
                self.verification = None;
                self.inventory = Some(result.map_err(|error| error.to_string()));
            }
            CacheOutcome::Verify(result) => {
                let result = result.map_err(|error| error.to_string());
                if let Ok(report) = &result {
                    self.notice = Some((
                        report.invalid_count == 0,
                        format!(
                            "Verified {} saved database copies: {} valid, {} with problems.",
                            report.entries.len(),
                            report.valid_count,
                            report.invalid_count
                        ),
                    ));
                }
                self.verification = Some(result);
            }
            CacheOutcome::Pin(result) => match result {
                Ok(result) => {
                    let text = match result.status {
                        SnapshotPinStatus::Pinned => {
                            "Kept: this copy is now protected from clean-up."
                        }
                        SnapshotPinStatus::AlreadyPinned => "That copy was already protected.",
                        SnapshotPinStatus::Unpinned => "Protection removed from that copy.",
                        SnapshotPinStatus::AlreadyUnpinned => "That copy was not protected.",
                    };
                    self.notice = Some((true, text.to_string()));
                    // Pins change inventory and invalidate any reviewed plan.
                    self.invalidate();
                    self.refresh_pending = true;
                }
                Err(error) => self.notice = Some((false, error.to_string())),
            },
            CacheOutcome::Plan(result) => match result {
                Ok(plan) => self.plan = Some(plan),
                Err(error) => {
                    self.plan = None;
                    self.notice = Some((false, error.to_string()));
                }
            },
            CacheOutcome::Prune(result) => {
                self.invalidate();
                self.refresh_pending = true;
                self.notice = Some(match result {
                    Ok(result) => {
                        let untouched = result
                            .entries
                            .iter()
                            .filter(|entry| entry.status != CachePruneEntryStatus::Deleted)
                            .collect::<Vec<_>>();
                        let mut text = format!(
                            "Clean-up {}: {} old copies and {} leftovers removed, {} freed.",
                            match result.status {
                                CachePruneExecutionStatus::Completed => "finished",
                                CachePruneExecutionStatus::PartialFailure =>
                                    "finished with problems (some items were left untouched)",
                                CachePruneExecutionStatus::Preview => "previewed",
                            },
                            result.snapshots_deleted,
                            result.staging_entries_removed,
                            human_bytes(result.bytes_reclaimed)
                        );
                        if let Some(first) = untouched.first() {
                            text.push_str(&format!(
                                " {} item(s) were left untouched, for example {}: {}",
                                untouched.len(),
                                first.path.display,
                                first.detail
                            ));
                        }
                        (result.status == CachePruneExecutionStatus::Completed, text)
                    }
                    Err(error) => (false, error.to_string()),
                });
            }
        }
    }

    fn policy(&self) -> CachePrunePolicy {
        CachePrunePolicy {
            keep_newest_per_source: Some(self.keep.max(MINIMUM_KEEP)),
            include_abandoned_staging: self.include_staging,
            ..CachePrunePolicy::default()
        }
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn verification_label(state: SnapshotVerificationState) -> &'static str {
    match state {
        SnapshotVerificationState::Valid => "Verified",
        SnapshotVerificationState::InvalidManifest => "Damaged: file list unreadable",
        SnapshotVerificationState::IdentityMismatch => "Damaged: identity does not match",
        SnapshotVerificationState::MissingFile => "Damaged: a file is missing",
        SnapshotVerificationState::ChangedFile => "Damaged: a file was changed",
        SnapshotVerificationState::UnexpectedFile => "Damaged: unexpected extra file",
        SnapshotVerificationState::SizeMismatch => "Damaged: file size differs",
        SnapshotVerificationState::DigestMismatch => "Damaged: checksum differs",
        SnapshotVerificationState::UnsupportedSchema => "Unsupported format",
        SnapshotVerificationState::UnsafePath => "Refused: unsafe path",
        SnapshotVerificationState::IncompleteStagingArtifact => "Unfinished download",
        SnapshotVerificationState::UnreadablePath => "Unreadable",
    }
}

impl NativeWorkflows {
    /// Collects the cache-maintenance job. Called once per frame from `poll`,
    /// so a result never waits for the tab to be reopened.
    pub(in crate::gui_v2) fn poll_cheat_catalogues(&mut self, context: &egui::Context) {
        let mut changed = self.cheat_catalogues.poll();
        // A finished download changes what is saved: drop stale reads.
        let retrieving = self.app.catalogue_bsfree_ui.catalogue_retrieval.is_some();
        if self.cheat_catalogues.retrieving_before && !retrieving {
            self.cheat_catalogues.invalidate();
            changed = true;
        }
        self.cheat_catalogues.retrieving_before = retrieving;
        if self.cheat_catalogues.refresh_pending && !self.cheat_catalogues.busy() && !retrieving {
            self.cheat_catalogues.refresh_pending = false;
            self.cheat_catalogues
                .start_job(context, CacheJob::Inventory);
            changed = true;
        }
        if changed || self.cheat_catalogues.busy() {
            context.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Shows RetroArch, Dolphin and cache-maintenance jobs in Activity with
    /// their outcome. The cards keep their own progress and Cancel controls.
    pub(in crate::gui_v2) fn observe_catalogue_activity(&mut self, activity: &mut Activity) {
        let ui_state = &self.app.catalogue_bsfree_ui;
        let retroarch_running = ui_state.catalogue_retrieval.is_some();
        let retroarch_outcome = match &ui_state.catalogue_last_result {
            Some(Err(error)) => (
                "Cheat database retrieval did not complete.".to_string(),
                Some(error.to_string()),
            ),
            _ => ("Cheat database retrieval finished.".to_string(), None),
        };
        let dolphin_running = ui_state.dolphin_catalogue_retrieval.is_some();
        let dolphin_outcome = match &ui_state.dolphin_catalogue_last_result {
            Some(Err(error)) => (
                "Dolphin cheat catalogue retrieval did not complete.".to_string(),
                Some(error.to_string()),
            ),
            _ => (
                "Dolphin cheat catalogue retrieval finished.".to_string(),
                None,
            ),
        };
        let cache_running = self.cheat_catalogues.busy();
        let cache_outcome = match &self.cheat_catalogues.notice {
            Some((false, message)) => (message.clone(), Some(message.clone())),
            Some((true, message)) => (message.clone(), None),
            None => ("Cheat cache check finished.".to_string(), None),
        };
        track_job(
            activity,
            &mut self.cheat_catalogues.retroarch_job,
            retroarch_running,
            "Retrieving the RetroArch cheat database",
            retroarch_outcome,
        );
        track_job(
            activity,
            &mut self.cheat_catalogues.dolphin_job,
            dolphin_running,
            "Retrieving the Dolphin cheat catalogue",
            dolphin_outcome,
        );
        track_job(
            activity,
            &mut self.cheat_catalogues.cache_job,
            cache_running,
            "Checking saved cheat databases",
            cache_outcome,
        );
    }

    pub(in crate::gui_v2) fn show_cheat_catalogues(&mut self, ui: &mut egui::Ui) -> Option<Route> {
        ui.heading("Cheat catalogues");
        ui.label("Manage the cheat databases EmuWiz searches for every game: the RetroArch (libretro) database and the Dolphin catalogue. Opening this tab only reads what is already on this computer. Downloads happen only when you review and confirm them, and downloaded cheat text is data - it is never run.");
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (SubTab::Catalogues, "Databases"),
                (SubTab::Sources, "Sources & priority"),
                (SubTab::Cache, "Saved copies"),
            ] {
                if ui
                    .selectable_label(self.cheat_catalogues.tab == tab, label)
                    .clicked()
                {
                    self.cheat_catalogues.tab = tab;
                }
            }
        });
        ui.separator();
        match self.cheat_catalogues.tab {
            SubTab::Catalogues => self.show_catalogue_cards(ui),
            SubTab::Sources => {
                let context = ui.ctx().clone();
                self.app.show_cheat_sources_page(&context, ui);
            }
            SubTab::Cache => self.show_cache_maintenance(ui),
        }
        None
    }

    fn show_catalogue_cards(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        // Local reads only; the network is reached solely by a confirmed
        // action on the cards below.
        if matches!(
            self.app.catalogue_bsfree_ui.catalogue_manager,
            crate::sources_page::CatalogueManagerState::NotLoaded
        ) {
            self.app.start_catalogue_status_load(context.clone());
        }
        if matches!(
            self.app.catalogue_bsfree_ui.dolphin_catalogue_manager,
            crate::sources_page::DolphinCatalogueManagerState::NotLoaded
        ) {
            self.app
                .start_dolphin_catalogue_status_load(context.clone());
        }
        let now_unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let ui_state = &mut self.app.catalogue_bsfree_ui;
        let retroarch_action = crate::sources_page::show_retroarch_catalogue_manager(
            ui,
            &ui_state.catalogue_manager,
            ui_state.catalogue_review.as_ref(),
            ui_state.catalogue_retrieval.as_ref(),
            ui_state.catalogue_last_result.as_ref(),
            &mut self.app.clipboard,
        );
        ui.add_space(theme::SECTION_GAP);
        let ui_state = &mut self.app.catalogue_bsfree_ui;
        let dolphin_action = crate::sources_page::show_dolphin_catalogue_manager(
            ui,
            &ui_state.dolphin_catalogue_manager,
            ui_state.dolphin_catalogue_retrieval.as_ref(),
            ui_state.dolphin_catalogue_last_result.as_ref(),
            crate::sources_page::DolphinCatalogueCardContext {
                review: ui_state.dolphin_catalogue_review,
                update_available: ui_state.dolphin_catalogue_update_available,
                remove_confirm: ui_state.dolphin_catalogue_remove_confirm,
                now_unix_seconds,
            },
            &mut self.app.clipboard,
        );
        if let Some(action) = retroarch_action {
            self.app.handle_catalogue_manager_action(&context, action);
        }
        if let Some(action) = dolphin_action {
            self.app
                .handle_dolphin_catalogue_manager_action(&context, action);
        }
    }

    fn show_cache_maintenance(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        let state = &mut self.cheat_catalogues;
        let retrieving = self.app.catalogue_bsfree_ui.catalogue_retrieval.is_some();
        ui.label("Each RetroArch database download is kept as a separate saved copy. You can check that they are intact, protect one from clean-up, and remove old copies. Clean-up never touches cheats already installed in an emulator, and it always shows what it would remove first.");
        if retrieving {
            ui.colored_label(
                theme::warning(ui),
                "A database download is running. Saved copies cannot be changed until it finishes.",
            );
        }
        let idle = !state.busy() && !retrieving;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(idle, egui::Button::new("List saved copies"))
                .clicked()
            {
                state.start(&context, CacheJob::Inventory);
            }
            if ui
                .add_enabled(idle, egui::Button::new("Verify all copies"))
                .clicked()
            {
                state.start(&context, CacheJob::Verify);
            }
            if state.busy() {
                ui.spinner();
                ui.label("Working in the background…");
            }
        });
        if let Some((ok, message)) = &state.notice {
            let tone = if *ok {
                widgets::StatusTone::Info
            } else {
                widgets::StatusTone::Blocked
            };
            widgets::banner(
                ui,
                if *ok { "Done" } else { "Not completed" },
                message,
                tone,
            );
        }
        let mut pin_request = None;
        let entries: Option<&[SnapshotInventoryEntry]> =
            match (&state.verification, &state.inventory) {
                (Some(Ok(report)), _) => Some(&report.entries),
                (None, Some(Ok(report))) => Some(&report.entries),
                _ => None,
            };
        if let (Some(Err(error)), _) | (None, Some(Err(error))) =
            (&state.verification, &state.inventory)
        {
            widgets::banner(
                ui,
                "No saved copies to show",
                &format!(
                    "{error}\nIf you have not downloaded the RetroArch database yet, use the Databases tab first."
                ),
                widgets::StatusTone::Info,
            );
        }
        if let Some(entries) = entries {
            if entries.is_empty() {
                ui.label("No saved copies yet.");
            }
            for entry in entries {
                ui.group(|ui| {
                    ui.strong(format!(
                        "{} / {}",
                        entry.source_id.as_deref().unwrap_or("unsafe source name"),
                        entry.snapshot_id.as_deref().unwrap_or("unsafe copy name")
                    ));
                    ui.label(verification_label(entry.verification_state));
                    ui.horizontal_wrapped(|ui| {
                        if entry.current {
                            ui.label("In use");
                        }
                        if entry.last_known_good {
                            ui.label("Last known good");
                        }
                        if entry.pinned {
                            ui.label("Protected");
                        }
                        if let Some(size) = entry.extracted_size {
                            ui.label(human_bytes(size));
                        }
                    });
                    for finding in &entry.verification_findings {
                        ui.label(&finding.message);
                    }
                    if let Some(id) = &entry.snapshot_id
                        && entry.pin_metadata_valid
                        && entry.verification_state == SnapshotVerificationState::Valid
                    {
                        let label = if entry.pinned {
                            "Remove protection"
                        } else {
                            "Protect from clean-up"
                        };
                        if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                            pin_request = Some((id.clone(), !entry.pinned));
                        }
                    }
                    crate::ui::components::technical_details(
                        ui,
                        ("cheat-cache-entry", &entry.cache_path.display),
                        |ui| {
                            ui.label(&entry.cache_path.display);
                            if let Some(hash) = &entry.archive_sha256 {
                                ui.label(format!("Archive SHA-256: {hash}"));
                            }
                            if let Some(url) = &entry.source_url {
                                ui.label(format!("Source: {url}"));
                            }
                        },
                    );
                });
            }
        }
        if let Some((snapshot_id, pinned)) = pin_request {
            state.start(
                &context,
                CacheJob::Pin {
                    snapshot_id,
                    pinned,
                },
            );
        }

        ui.add_space(theme::SECTION_GAP);
        ui.strong("Clean up old copies");
        ui.horizontal_wrapped(|ui| {
            ui.label("Keep the newest");
            if ui
                .add(egui::DragValue::new(&mut state.keep).range(MINIMUM_KEEP..=50))
                .changed()
            {
                state.plan = None;
            }
            ui.label("copies of each database (never fewer than 2)");
        });
        if ui
            .checkbox(
                &mut state.include_staging,
                "Also remove unfinished downloads older than 24 hours",
            )
            .changed()
        {
            state.plan = None;
        }
        if ui
            .add_enabled(idle, egui::Button::new("Preview clean-up"))
            .clicked()
        {
            let policy = state.policy();
            state.plan = None;
            state.start(&context, CacheJob::Plan(policy));
        }
        let mut confirm = None;
        if let Some(plan) = &state.plan {
            if plan.candidate_count == 0 {
                ui.label(format!(
                    "Nothing to remove. {} items are protected.",
                    plan.protected_count
                ));
            } else {
                ui.label(format!(
                    "This would remove {} items and free {}. {} items are protected and stay.",
                    plan.candidate_count,
                    human_bytes(plan.candidate_bytes),
                    plan.protected_count
                ));
                for entry in plan
                    .entries
                    .iter()
                    .filter(|entry| entry.disposition == CachePruneDisposition::Candidate)
                {
                    ui.label(format!(
                        "• {} ({})",
                        entry.path.display,
                        human_bytes(entry.bytes)
                    ));
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(format!("Remove {} items", plan.candidate_count)),
                    )
                    .clicked()
                {
                    confirm = Some(plan.clone());
                }
            }
        }
        if let Some(plan) = confirm {
            state.start(&context, CacheJob::Prune(Box::new(plan)));
        }
    }
}

/// Mirrors a background job into Activity: queue+start when it begins,
/// finish (with the failure text, if any) when it ends.
fn track_job(
    activity: &mut Activity,
    slot: &mut Option<u64>,
    running: bool,
    title: &str,
    outcome: (String, Option<String>),
) {
    if running && slot.is_none() {
        let id = activity.queue(title, Route::Section(Section::Mods), false);
        activity.start(id);
        *slot = Some(id);
    } else if !running && let Some(id) = slot.take() {
        activity.finish(id, outcome.0, outcome.1);
    }
}

#[cfg(test)]
#[path = "native_cheat_catalogues/tests.rs"]
mod tests;
