//! Blue host for the existing Collection Discovery report and source scanner.
//! No traversal, classification or persistence is implemented here.

#[path = "native_media_preview.rs"]
mod media_preview;

use super::NativeWorkflows;
use crate::{DatabaseState, SourceAction};
use archivefs_core::{Database, SourceFolderView};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub(super) struct State {
    media: media_preview::State,
    review: Option<ScanReview>,
    pub(super) refusal: Option<String>,
    scope_job: Option<Receiver<Result<ScanReview, String>>>,
}

impl State {
    pub(super) fn preparing(&self) -> bool {
        self.scope_job.is_some()
    }
}

#[derive(Clone)]
struct ScanReview {
    action: SourceAction,
    generation: u64,
    database_path: PathBuf,
    database_existed: bool,
    sources: Vec<SourceFolderView>,
    known_systems: BTreeMap<i64, BTreeSet<String>>,
}

// The existing snapshot already includes the newest partial run here. A
// completed-only fallback would hide a newer scan's committed partial results.
pub(super) fn latest_saved_scan(
    snapshot: &crate::database_load::CachedLibrarySnapshot,
) -> Option<&archivefs_core::CompletedScanSummary> {
    snapshot
        .recently_found
        .as_ref()
        .map(|recent| &recent.scan)
        .or(snapshot.last_completed_scan.as_ref())
}

fn known_systems(
    snapshot: &crate::database_load::CachedLibrarySnapshot,
) -> BTreeMap<i64, BTreeSet<String>> {
    let mut systems = BTreeMap::<i64, BTreeSet<String>>::new();
    for archive in &snapshot.archives {
        systems.entry(archive.source_folder_id).or_default().insert(
            archive
                .platform
                .clone()
                .unwrap_or_else(|| "Unidentified records".into()),
        );
    }
    systems
}

fn scans(action: &SourceAction) -> bool {
    matches!(
        action,
        SourceAction::ScanOne(_)
            | SourceAction::ScanAll
            | SourceAction::SetEnabled { enabled: true, .. }
    )
}

impl ScanReview {
    fn without_database_at(
        config_path: &Path,
        database_path: PathBuf,
        generation: u64,
        action: SourceAction,
    ) -> Result<Self, String> {
        require_absent_database(&database_path)?;
        let configs = archivefs_core::load_source_folder_configs_from(config_path)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            action,
            generation,
            database_path,
            database_existed: false,
            sources: archivefs_core::build_source_folder_views(&configs, &[]),
            known_systems: BTreeMap::new(),
        })
    }

    fn selected(&self, source: &SourceFolderView) -> bool {
        match &self.action {
            SourceAction::ScanAll => source.enabled,
            SourceAction::ScanOne(path)
            | SourceAction::SetEnabled {
                path,
                enabled: true,
            } => source.path == *path,
            _ => false,
        }
    }

    fn current(&self, generation: u64, sources: &[SourceFolderView]) -> bool {
        self.generation == generation && self.sources == sources
    }

    /// This check only reads saved configuration and metadata. Keep it on the
    /// source worker; never open a writable DB just to validate a review.
    fn validate_saved_at(&self, config_path: &Path, database_path: &Path) -> Result<(), String> {
        if database_path != self.database_path {
            return Err(
                "The library location changed. Reload saved status and review again.".into(),
            );
        }
        let configs = archivefs_core::load_source_folder_configs_from(config_path)
            .map_err(|error| error.to_string())?;
        let records = if self.database_existed {
            let database =
                Database::open_read_only(database_path).map_err(|error| error.to_string())?;
            database
                .list_source_folders()
                .map_err(|error| error.to_string())?
        } else {
            require_absent_database(database_path)?;
            Vec::new()
        };
        let sources = archivefs_core::build_source_folder_views(&configs, &records);
        if sources != self.sources {
            return Err("Saved source state changed after review. Nothing was scanned. Reload saved status and review again.".into());
        }
        Ok(())
    }

    fn validate_saved(&self) -> Result<(), String> {
        let config = archivefs_core::default_config_path().map_err(|error| error.to_string())?;
        let database =
            archivefs_core::default_database_path().map_err(|error| error.to_string())?;
        self.validate_saved_at(&config, &database)
    }

    fn show(&self, ui: &mut egui::Ui) {
        ui.label(format!(
            "Library database: {}",
            self.database_path.display()
        ));
        if !self.database_existed {
            ui.strong("This first scan creates the library database using the existing scanner. Source roles start with the backend's normal default.");
        }
        ui.label("This scan updates library records: it can add or update games and mark previously catalogued files missing. It does not move, rename or delete game files.");
        if matches!(self.action, SourceAction::SetEnabled { enabled: true, .. }) {
            ui.strong(
                "Enable and scan: the existing backend scans immediately when enabling a source.",
            );
        }
        ui.strong("Selected roots and affected systems");
        for source in self.sources.iter().filter(|source| self.selected(source)) {
            ui.label(source.path.display().to_string());
            ui.label(format!(
                "Role: {} | Assigned system: {}",
                source.role.label(),
                source
                    .assigned_platform
                    .as_deref()
                    .unwrap_or("Not assigned; existing identification rules apply")
            ));
            if let Some(systems) = source.id.and_then(|id| self.known_systems.get(&id)) {
                ui.label(format!(
                    "Known systems in saved catalogue: {}",
                    systems.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            } else {
                ui.weak("No catalogued systems yet. New content may identify additional systems.");
            }
            if !source.role.may_enter_game_scan() {
                ui.weak("Excluded from game traversal by its saved source role.");
            }
            if let Some(error) = &source.last_scan_error {
                ui.label(format!("Last scan problem: {error}"));
            }
        }
        ui.strong("Excluded roots");
        let excluded: Vec<_> = self
            .sources
            .iter()
            .filter(|source| !self.selected(source))
            .collect();
        if excluded.is_empty() {
            ui.weak("None outside the selected scope.");
        }
        for source in excluded {
            ui.label(format!(
                "{} ({})",
                source.path.display(),
                if source.enabled {
                    "outside selected scope"
                } else {
                    "disabled"
                }
            ));
        }
        ui.label("Every system under the selected roots is in scope. The existing scanner cannot filter by system or preview database changes. Uncertain platforms stay unassigned; this review does not assign systems.");
        ui.label("Cancel is available before starting. Once started, this source scanner has no cancellation or incremental progress API. Results appear when it returns; leaving the page does not stop it.");
    }
}

fn require_absent_database(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("The library database appeared after scope preparation. Reload saved status and review again.".into()),
        Err(error) => Err(format!("Cannot inspect the library database location: {error}")),
    }
}

impl NativeWorkflows {
    /// All Blue controls that can trigger a source scan pass through the same
    /// review, including Enable (which the existing backend makes a scan).
    pub(super) fn review_or_start_source_action(
        &mut self,
        ctx: egui::Context,
        action: SourceAction,
    ) {
        if !self.app.source_action_available() || self.collection.preparing() {
            return;
        }
        if !scans(&action) {
            self.app.start_source_action(ctx, action);
            return;
        }
        if let DatabaseState::NotCreated { database_path } = &self.app.database_state {
            let database_path = database_path.clone();
            let generation = self.app.database_generation.0;
            let (sender, receiver) = mpsc::channel();
            self.collection.scope_job = Some(receiver);
            self.collection.refusal = None;
            std::thread::spawn(move || {
                let result = archivefs_core::default_config_path()
                    .map_err(|error| error.to_string())
                    .and_then(|config| {
                        ScanReview::without_database_at(&config, database_path, generation, action)
                    });
                let _ = sender.send(result);
                ctx.request_repaint();
            });
            return;
        }
        let DatabaseState::Ready { snapshot, .. } = &self.app.database_state else {
            self.collection.refusal = Some("Reload saved status before reviewing a scan. The library must be ready so its source roles and exclusions are known.".into());
            return;
        };
        let review = ScanReview {
            action,
            generation: self.app.database_generation.0,
            database_path: snapshot.database_path.clone(),
            database_existed: true,
            sources: snapshot.source_views.clone(),
            known_systems: known_systems(snapshot),
        };
        if !review.sources.iter().any(|source| review.selected(source)) {
            self.collection.refusal = Some("No configured roots in this scope. Review enabled folders and reload saved status.".into());
            return;
        }
        self.collection.refusal = None;
        self.collection.review = Some(review);
    }

    pub(super) fn show_source_scan_review(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.collection.scope_job {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(
                    "Saved scope could not be loaded. Nothing was scanned.".into(),
                )),
            };
            if let Some(result) = result {
                self.collection.scope_job = None;
                match result {
                    Ok(review) if self.review_is_current(&review) => {
                        if review.sources.iter().any(|source| review.selected(source)) {
                            self.collection.review = Some(review);
                        } else {
                            self.collection.refusal = Some("No configured roots in this scope. Reload saved status and review enabled roots.".into());
                        }
                    }
                    Ok(_) => self.collection.refusal = Some("Source state changed while loading the review. Reload saved status and review again.".into()),
                    Err(error) => self.collection.refusal = Some(error),
                }
            }
        }
        let Some(review) = self.collection.review.as_ref() else {
            return;
        };
        let current = self.review_is_current(review);
        let available = self.app.source_action_available();
        let mut cancel = false;
        let mut confirm = false;
        let response = egui::Modal::new(egui::Id::new("blue_collection_scan_review")).show(ctx, |ui| {
            ui.set_width(ui.available_width().min(620.0));
            ui.heading("Review library scan");
            // Primary controls stay above the scrolling scope at 1024x600.
            ui.horizontal_wrapped(|ui| {
                cancel = ui.button("Cancel").clicked();
                confirm = ui.add_enabled(current && available, egui::Button::new("Confirm scan and update library")).clicked();
            });
            if !current {
                ui.label("Source state changed after review. Cancel, reload saved status and review again.");
            } else if !available {
                ui.label("Another library action is in progress. Scanning is unavailable.");
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| review.show(ui));
        });
        if cancel || response.should_close() {
            self.collection.review = None;
        } else if confirm && current && available {
            let review = self.collection.review.take().unwrap();
            self.app
                .start_source_action_checked(ctx.clone(), review.action.clone(), move || {
                    review.validate_saved()
                });
        }
    }

    fn review_is_current(&self, review: &ScanReview) -> bool {
        match &self.app.database_state {
            DatabaseState::Ready { snapshot, .. } => {
                review.database_existed
                    && snapshot.database_path == review.database_path
                    && review.current(self.app.database_generation.0, &snapshot.source_views)
            }
            DatabaseState::NotCreated { database_path } => {
                !review.database_existed
                    && *database_path == review.database_path
                    && self.app.database_generation.0 == review.generation
            }
            _ => false,
        }
    }

    pub(super) fn show_scope_loading(&mut self, ui: &mut egui::Ui) {
        if self.collection.preparing() {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label("Reading saved scan scope. No collection scan has started.");
                if ui.button("Cancel scope review").clicked() {
                    self.collection.scope_job = None;
                }
            });
        }
    }

    pub(super) fn show_collection_report(&mut self, ui: &mut egui::Ui) {
        ui.heading("Collection Discovery");
        if let DatabaseState::Ready { snapshot, .. } = &self.app.database_state {
            self.collection.media.show(
                ui,
                &snapshot.database_path,
                self.app.database_generation.0,
                &snapshot.source_views,
                !self.app.source_action_available(),
            );
        }
        ui.label("Read-only results from the latest saved scan, including partial coverage. Browsing, filtering and paging do not scan or update your library.");
        if let Some(message) = &self.collection.refusal {
            ui.label(message);
        }
        let (summary, discovery_run, completed) = match &self.app.database_state {
            DatabaseState::Ready {
                snapshot,
                last_scan_summary,
            } => (
                last_scan_summary.as_ref().filter(|summary| {
                    latest_saved_scan(snapshot)
                        .is_some_and(|scan| scan.scan_run_id == summary.scan_run_id)
                }),
                latest_saved_scan(snapshot)
                    .map(|scan| (snapshot.database_path.as_path(), scan.scan_run_id)),
                latest_saved_scan(snapshot),
            ),
            _ => {
                ui.label("Saved discovery results are unavailable while the library is loading or needs attention. Reload saved status; no scan starts automatically.");
                if let DatabaseState::Error { message, .. } = &self.app.database_state {
                    ui.label(message);
                }
                return;
            }
        };
        if let Some(scan) = completed {
            let partial = self.app.database_state.snapshot().is_some_and(|snapshot| {
                snapshot
                    .last_completed_scan
                    .as_ref()
                    .map(|complete| complete.scan_run_id)
                    != Some(scan.scan_run_id)
            });
            if partial {
                ui.strong("Partial coverage: this scan returned with incomplete sources. Positive observations are saved; missing-file reconciliation is withheld where coverage was incomplete.");
            }
            ui.label(format!(
                "Saved scan {} | Finished: {}",
                scan.scan_run_id,
                scan.finished_at.as_deref().unwrap_or("Not recorded")
            ));
            ui.label(format!("Library records: {} seen | {} new | {} updated | {} existing unchanged | {} missing | {} errors",
                scan.archives_seen, scan.archives_added, scan.archives_updated, scan.archives_unchanged, scan.archives_missing, scan.errors_count));
            ui.small("These counts describe catalogued records. Discovery item counts below describe recognised content and exclusions; the totals measure different things.");
            if let Some(error) = &scan.error_message {
                ui.label(error);
            }
        }
        if let Some(summary) = summary {
            for (path, error) in &summary.folder_errors {
                ui.label(format!(
                    "Source failed: {}: {error}. Its previous catalogue entries were preserved.",
                    path.display()
                ));
            }
            for (path, warning) in &summary.platform_assignment_warnings {
                ui.label(format!(
                    "Platform needs review: {}: {warning}",
                    path.display()
                ));
            }
            if ui
                .button("Inspect skipped files (bounded sample)")
                .clicked()
            {
                self.app.show_skipped_files = true;
            }
        }
        crate::collection_discovery_page::show_collection_discovery_panel(
            ui,
            summary,
            discovery_run,
        );
        if self.app.show_skipped_files {
            crate::collection_discovery_page::show_skipped_files_window(
                ui.ctx(),
                &mut self.app.show_skipped_files,
                summary,
                &mut self.app.skipped_files_filter,
            );
        }
    }
}

#[cfg(test)]
#[path = "native_collection_tests.rs"]
mod tests;
