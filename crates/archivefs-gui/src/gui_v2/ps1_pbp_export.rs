//! Selected-game PS1 PBP playing-copy journey. Core alone admits, writes,
//! round-trips, and publishes; this module owns only interaction and workers.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};

use archivefs_core::dat::sources::{
    DatSourceEntry, DatSourceRegistry, load_dat_sources_config_default,
};
use archivefs_core::media_set::MediaSet;
use archivefs_core::repair::ps1_pbp_export::{
    DERIVED_LABEL, PbpExportError, PbpExportPlan, PbpExportProgress, PbpExportResult,
    PbpRecoveryStatus, SUPPORTED_INPUTS, WRITER_RELEASE, execute_ps1_pbp_export_with_progress,
    plan_ps1_pbp_export,
};
use eframe::egui;

use super::{Route, Section, library::Game, media_sets};

enum Event {
    Planned(Result<PbpExportPlan, PbpExportError>),
    Progress(PbpExportProgress),
    Finished(Result<PbpExportResult, PbpExportError>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Plan,
    Export,
}

#[derive(Debug, PartialEq, Eq)]
enum ConfirmationAction {
    Back,
    Export,
    None,
}

fn confirmation_contents(
    ui: &mut egui::Ui,
    disc_lines: &[String],
    destination: &Path,
    acknowledged: &mut bool,
) -> ConfirmationAction {
    ui.set_max_width(520.0);
    ui.strong(DERIVED_LABEL);
    ui.label(format!(
        "{} discs, in the verified order below:",
        disc_lines.len()
    ));
    egui::ScrollArea::vertical()
        .max_height(160.0)
        .show(ui, |ui| {
            for line in disc_lines {
                ui.label(line);
            }
        });
    ui.label(format!("New output: {}", destination.display()));
    ui.small("Core will extract all PBP discs and compare serials, order, track layout and every raw sector before journaled publication.");
    ui.checkbox(
        acknowledged,
        "I will retain the original discs as my preservation copy.",
    );
    let mut action = ConfirmationAction::None;
    ui.horizontal(|ui| {
        if ui.button("Back").clicked() {
            action = ConfirmationAction::Back;
        }
        if ui
            .add_enabled(*acknowledged, egui::Button::new("Create verified PBP"))
            .clicked()
        {
            action = ConfirmationAction::Export;
        }
    });
    action
}

struct Job {
    receiver: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    kind: JobKind,
    path: PathBuf,
    library_key: usize,
}

#[derive(Default)]
pub(super) struct State {
    scope: Option<(PathBuf, usize)>,
    writer: String,
    destination: String,
    preview: Option<PbpExportPlan>,
    refusal: Option<PbpExportError>,
    progress: Option<PbpExportProgress>,
    confirm: bool,
    retain_acknowledged: bool,
    job: Option<Job>,
    last: Option<(PathBuf, usize, Result<PbpExportResult, PbpExportError>)>,
    history_dirty: bool,
}

fn failure(detail: impl Into<String>) -> PbpExportError {
    PbpExportError {
        detail: detail.into(),
        recovery: None,
    }
}

fn current_sources() -> Result<Vec<DatSourceEntry>, PbpExportError> {
    let config = load_dat_sources_config_default().map_err(|e| failure(e.to_string()))?;
    let (registry, problems) = DatSourceRegistry::from_config(&config);
    if !problems.is_empty() {
        return Err(failure(format!(
            "DAT source configuration needs review: {}",
            problems.join("; ")
        )));
    }
    Ok(registry.sorted_all().into_iter().cloned().collect())
}

fn plan_on_worker(
    set: &MediaSet,
    destination: &Path,
    writer: &Path,
    cancel: &AtomicBool,
) -> Result<PbpExportPlan, PbpExportError> {
    let db_path = archivefs_core::default_database_path().map_err(|e| failure(e.to_string()))?;
    let database =
        archivefs_core::Database::open_read_only(db_path).map_err(|e| failure(e.to_string()))?;
    plan_ps1_pbp_export(
        &database,
        &current_sources()?,
        set,
        destination,
        writer,
        cancel,
    )
}

fn export_on_worker(
    plan: &PbpExportPlan,
    cancel: &AtomicBool,
    progress: &dyn Fn(PbpExportProgress),
) -> Result<PbpExportResult, PbpExportError> {
    let db_path = archivefs_core::default_database_path().map_err(|e| failure(e.to_string()))?;
    let database =
        archivefs_core::Database::open_read_only(db_path).map_err(|e| failure(e.to_string()))?;
    let journal = archivefs_core::dat::rename_apply::default_rename_transaction_dir()
        .map_err(|e| failure(e.to_string()))?;
    execute_ps1_pbp_export_with_progress(
        &database,
        &current_sources()?,
        plan,
        &journal,
        cancel,
        progress,
    )
}

fn suggested_destination(game: &Game, set: Option<&MediaSet>) -> String {
    let release = set
        .map(|s| s.identity.key.value.as_str())
        .unwrap_or(&game.title);
    let stem: String = release
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '(' | ')') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    game.archive
        .absolute_path
        .parent()
        .unwrap_or(Path::new("/"))
        .join(format!("{} - derived.pbp", stem.trim()))
        .display()
        .to_string()
}

fn progress_text(progress: PbpExportProgress) -> String {
    match progress {
        PbpExportProgress::Rechecking => "Rechecking DAT and original discs".into(),
        PbpExportProgress::Copying { disc, total } => {
            format!("Copying verified disc {disc} of {total} privately")
        }
        PbpExportProgress::Packaging => "Creating private PBP with pinned PSXPackager".into(),
        PbpExportProgress::Extracting => "Extracting every PBP disc for verification".into(),
        PbpExportProgress::Comparing { disc, total } => {
            format!("Comparing disc {disc} of {total} with its original")
        }
        PbpExportProgress::Publishing => {
            "Publishing verified PBP and receipt through Repair journal".into()
        }
        PbpExportProgress::Recovering => "Checking rollback and recovery state".into(),
    }
}

impl State {
    fn observe_selection(
        &mut self,
        path: &Path,
        library_key: usize,
        game: &Game,
        set: Option<&MediaSet>,
    ) {
        if self
            .scope
            .as_ref()
            .is_some_and(|(old, key)| old == path && *key == library_key)
        {
            return;
        }
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.kind == JobKind::Plan)
            && let Some(job) = self.job.take()
        {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.scope = Some((path.to_path_buf(), library_key));
        self.destination = suggested_destination(game, set);
        self.preview = None;
        self.refusal = None;
        if self
            .job
            .as_ref()
            .is_none_or(|job| job.kind != JobKind::Export)
        {
            self.progress = None;
        }
        self.confirm = false;
        self.retain_acknowledged = false;
    }

    pub(super) fn poll(&mut self) {
        let Some(job) = self.job.as_ref() else {
            return;
        };
        let mut done = false;
        loop {
            match job.receiver.try_recv() {
                Ok(Event::Progress(progress)) => self.progress = Some(progress),
                Ok(Event::Planned(result)) => {
                    if self
                        .scope
                        .as_ref()
                        .is_some_and(|(path, key)| path == &job.path && *key == job.library_key)
                    {
                        match result {
                            Ok(plan) => {
                                self.preview = Some(plan);
                                self.refusal = None;
                            }
                            Err(error) => {
                                self.preview = None;
                                self.refusal = Some(error);
                            }
                        }
                    }
                    done = true;
                }
                Ok(Event::Finished(result)) => {
                    self.last = Some((job.path.clone(), job.library_key, result));
                    self.history_dirty = true;
                    done = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if job.kind == JobKind::Export {
                        let error = PbpExportError {
                            detail: "The export worker stopped without a result. Publication state is unknown; inspect the destination, receipt and Repair History before retrying.".into(),
                            recovery: Some(PbpRecoveryStatus::Unknown),
                        };
                        self.last = Some((job.path.clone(), job.library_key, Err(error)));
                        self.history_dirty = true;
                    } else {
                        self.refusal = Some(failure(
                            "Planning stopped before conversion. No export was started.",
                        ));
                    }
                    done = true;
                }
            }
            if done {
                break;
            }
        }
        if done {
            self.job = None;
            self.progress = None;
        }
    }

    fn start_plan(&mut self, ui: &egui::Ui, set: MediaSet, path: &Path, library_key: usize) {
        self.preview = None;
        self.refusal = None;
        self.confirm = false;
        let destination = PathBuf::from(self.destination.trim());
        let writer = PathBuf::from(self.writer.trim());
        if !destination.is_absolute() || !writer.is_absolute() {
            self.refusal = Some(failure(
                "Choose absolute destination and PSXPackager paths before checking.",
            ));
            return;
        }
        let (tx, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let context = ui.ctx().clone();
        std::thread::spawn(move || {
            let result = plan_on_worker(&set, &destination, &writer, &worker_cancel);
            let _ = tx.send(Event::Planned(result));
            context.request_repaint();
        });
        self.job = Some(Job {
            receiver,
            cancel,
            kind: JobKind::Plan,
            path: path.to_path_buf(),
            library_key,
        });
    }

    fn start_export(&mut self, ui: &egui::Ui, path: &Path, library_key: usize) {
        let Some(plan) = self.preview.take() else {
            return;
        };
        self.confirm = false;
        self.retain_acknowledged = false;
        self.refusal = None;
        let (tx, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let context = ui.ctx().clone();
        std::thread::spawn(move || {
            let notify = |progress| {
                let _ = tx.send(Event::Progress(progress));
                context.request_repaint();
            };
            let result = export_on_worker(&plan, &worker_cancel, &notify);
            let _ = tx.send(Event::Finished(result));
            context.request_repaint();
        });
        self.job = Some(Job {
            receiver,
            cancel,
            kind: JobKind::Export,
            path: path.to_path_buf(),
            library_key,
        });
        self.progress = Some(PbpExportProgress::Rechecking);
    }

    fn clear_preview(&mut self) {
        self.preview = None;
        self.confirm = false;
        self.retain_acknowledged = false;
    }

    pub(super) fn take_history_refresh(&mut self) -> bool {
        std::mem::take(&mut self.history_dirty)
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        set: Option<&MediaSet>,
        library_key: usize,
    ) -> Option<Route> {
        if game.platform != "PSX" && set.is_none_or(|set| set.platform.as_deref() != Some("PSX")) {
            return None;
        }
        let path = game.archive.absolute_path.as_path();
        self.observe_selection(path, library_key, game, set);
        let mut route = None;
        ui.separator();
        ui.strong("Make one PBP playing copy");
        ui.label(SUPPORTED_INPUTS);
        ui.small("ISO, CHD, multitrack CUE, six-disc sets and uncertain or mixed releases are unsupported.");
        ui.small("The original DAT-verified discs remain the preservation copy. A PBP is not guaranteed to play or switch discs in every emulator.");
        if let Some(set) = set {
            let view = media_sets::view_of(set, &std::collections::HashMap::new());
            ui.label(format!("{} · {}", view.title, view.completeness));
            for member in &view.members {
                ui.label(format!("{}: {}", member.label, member.paths.join(", ")));
            }
            for missing in &view.missing {
                ui.colored_label(crate::ui::theme::warning(ui), format!("Missing: {missing}"));
            }
            for conflict in &view.conflicts {
                ui.colored_label(crate::ui::theme::danger(ui), conflict);
            }
            if view.filename_only || !set.identity.verified {
                ui.label("This set is suggested by catalogue evidence only; export needs current verified DAT evidence.");
            }
        } else {
            ui.label("No current media set is available for this selected game. Check every disc and its DAT evidence first.");
        }
        if ui.button("Review multi-disc evidence").clicked() {
            route = Some(Route::Section(Section::MultiDisc));
        }

        let busy = self.job.is_some();
        if !busy {
            let old_writer = self.writer.clone();
            let old_destination = self.destination.clone();
            ui.label("PSXPackager executable");
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 90.0).max(120.0);
                ui.add(egui::TextEdit::singleline(&mut self.writer).desired_width(width));
                if ui.button("Browse…").clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    self.writer = path.display().to_string();
                }
            });
            ui.small(format!("Choose the user-installed, pinned {WRITER_RELEASE} Linux build; Core checks its binary and required resources."));
            ui.label("New PBP destination");
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 90.0).max(120.0);
                ui.add(egui::TextEdit::singleline(&mut self.destination).desired_width(width));
                if ui.button("Choose…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("PBP", &["pbp"])
                        .save_file()
                {
                    self.destination = path.display().to_string();
                }
            });
            if self.writer != old_writer || self.destination != old_destination {
                self.clear_preview();
                self.refusal = None;
            }
            if ui
                .add_enabled(set.is_some(), egui::Button::new("Check this export"))
                .clicked()
            {
                self.start_plan(
                    ui,
                    set.expect("enabled only with set").clone(),
                    path,
                    library_key,
                );
            }
        } else if let Some(job) = &self.job {
            if job.path != path {
                ui.label(format!(
                    "Export in progress for another selected game: {}",
                    job.path.display()
                ));
            }
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(self.progress.map(progress_text).unwrap_or_else(|| {
                    if job.kind == JobKind::Plan {
                        "Checking current evidence and writer".into()
                    } else {
                        "Export worker is running".into()
                    }
                }));
            });
            let cancellable = !matches!(
                self.progress,
                Some(PbpExportProgress::Publishing | PbpExportProgress::Recovering)
            );
            if ui
                .add_enabled(
                    cancellable && !job.cancel.load(Ordering::Relaxed),
                    egui::Button::new("Cancel"),
                )
                .clicked()
            {
                job.cancel.store(true, Ordering::Relaxed);
            }
            if job.cancel.load(Ordering::Relaxed) {
                ui.label("Cancellation requested; waiting for the worker to settle safely.");
            }
            if !cancellable {
                ui.label("Publication or recovery is settling; cancellation is unavailable now.");
            }
        }
        if let Some(error) = &self.refusal {
            show_error(ui, error);
        }
        if let Some(plan) = &self.preview {
            ui.separator();
            ui.strong("Verified export preview");
            ui.label(format!(
                "{} · {} · {} discs",
                plan.release(),
                plan.region(),
                plan.disc_count()
            ));
            for disc in plan.discs() {
                ui.label(format!(
                    "Disc {}: {} · {} · {}",
                    disc.ordinal(),
                    disc.cue().display(),
                    disc.serial(),
                    disc.dat_entry()
                ));
                ui.small(format!(
                    "Source SHA-256 · CUE {} · raw BIN {}",
                    disc.cue_sha256(),
                    disc.bin_sha256()
                ));
            }
            ui.label(format!("Destination: {}", plan.destination().display()));
            ui.label(format!(
                "Verification receipt: {}",
                plan.receipt_path().display()
            ));
            ui.small(format!(
                "DAT source {} · revision {} · catalogue SHA-256 {}",
                plan.dat_source(),
                plan.dat_revision(),
                plan.dat_catalogue_sha256()
            ));
            ui.small("Before publication, Core extracts every generated disc and compares its order, serial, track layout and full raw BIN bytes with the verified originals. Existing output paths are refused.");
            if !busy && ui.button("Continue to confirmation").clicked() {
                self.confirm = true;
                self.retain_acknowledged = false;
            }
        }
        if let Some((result_path, result_library_key, result)) = &self.last
            && result_path == path
            && *result_library_key == library_key
        {
            ui.separator();
            match result {
                Ok(result) => {
                    ui.strong(DERIVED_LABEL);
                    ui.label(format!(
                        "Verified PBP: {}",
                        result
                            .transaction
                            .transaction
                            .entries
                            .iter()
                            .find(|entry| entry
                                .destination_path
                                .extension()
                                .is_some_and(|ext| ext == "pbp"))
                            .map(|entry| entry.destination_path.display().to_string())
                            .unwrap_or_default()
                    ));
                    if let Some(receipt_path) =
                        result.transaction.transaction.entries.iter().find(|entry| {
                            entry
                                .destination_path
                                .extension()
                                .is_some_and(|ext| ext == "json")
                        })
                    {
                        ui.label(format!(
                            "Verification receipt: {}",
                            receipt_path.destination_path.display()
                        ));
                    }
                    ui.small(format!(
                        "{} discs verified by full raw-sector round trip",
                        result.receipt.discs.len()
                    ));
                    ui.small(format!(
                        "DAT {} · revision {} · writer {}",
                        result.receipt.dat_source,
                        result.receipt.dat_revision,
                        result.receipt.writer_release
                    ));
                    ui.small(format!("PBP SHA-256: {}", result.receipt.pbp_sha256));
                    ui.label(format!(
                        "Repair journal {}: applied and verified.",
                        result.transaction.transaction.transaction_id
                    ));
                    ui.small(format!(
                        "Recovery stage retained: {}",
                        result.recovery_stage.display()
                    ));
                    if let Some(warning) = &result.cleanup_warning {
                        ui.colored_label(crate::ui::theme::warning(ui), warning);
                    }
                }
                Err(error) => show_error(ui, error),
            }
            if ui.button("Open repair history").clicked() {
                route = Some(Route::Section(Section::History));
            }
        }
        if self.confirm && self.preview.is_some() {
            let mut open = true;
            egui::Window::new("Confirm derived PBP export")
                .id(egui::Id::new("ps1_pbp_export_confirmation"))
                .collapsible(false)
                .resizable(false)
                .default_width(520.0)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    if let Some(plan) = &self.preview {
                        let lines = plan
                            .discs()
                            .iter()
                            .map(|disc| {
                                format!(
                                    "Disc {} · {} · {}",
                                    disc.ordinal(),
                                    disc.serial(),
                                    disc.cue().display()
                                )
                            })
                            .collect::<Vec<_>>();
                        let destination = plan.destination().to_path_buf();
                        match confirmation_contents(
                            ui,
                            &lines,
                            &destination,
                            &mut self.retain_acknowledged,
                        ) {
                            ConfirmationAction::Back => self.confirm = false,
                            ConfirmationAction::Export => self.start_export(ui, path, library_key),
                            ConfirmationAction::None => {}
                        }
                    }
                });
            if !open {
                self.confirm = false;
            }
        }
        route
    }
}

fn show_error(ui: &mut egui::Ui, error: &PbpExportError) {
    ui.colored_label(
        crate::ui::theme::danger(ui),
        format!("Export refused: {}", error.detail),
    );
    match &error.recovery {
        Some(PbpRecoveryStatus::Unknown) => ui.label("Publication state is unknown. Inspect the destination, receipt and Repair History before retrying."),
        Some(PbpRecoveryStatus::NothingPublished { transaction_id, stage }) => ui.label(format!("Nothing was published. Repair journal: {transaction_id}. Recovery stage: {}.", stage.display())),
        Some(PbpRecoveryStatus::RolledBack { transaction_id, stage }) => ui.label(format!("Publication was fully rolled back. Repair journal: {transaction_id}. Recovery stage: {}.", stage.display())),
        Some(PbpRecoveryStatus::NeedsReview { transaction_id, stage, detail }) => {
            ui.label(format!("Recovery needs review in Repair History. Journal: {transaction_id}. Stage: {}. {detail}", stage.display()))
        }
        None => ui.label("No verified PBP was accepted. Originals remain in place."),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(id: i64) -> Game {
        Game::from_archive(crate::gui_v2::tests::archive(
            id,
            &format!("PS1 Disc {id}"),
            Some("PSX"),
        ))
    }

    #[test]
    fn changed_selection_drops_unconfirmed_preview_and_stale_plan_result() {
        let first = game(1);
        let second = game(2);
        let mut state = State::default();
        state.observe_selection(&first.archive.absolute_path, 1, &first, None);
        state.confirm = true;
        state.retain_acknowledged = true;
        state.refusal = Some(failure("old evidence"));
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        state.job = Some(Job {
            receiver,
            cancel: cancel.clone(),
            kind: JobKind::Plan,
            path: first.archive.absolute_path.clone(),
            library_key: 1,
        });
        state.observe_selection(&second.archive.absolute_path, 1, &second, None);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(state.job.is_none());
        assert!(!state.confirm && !state.retain_acknowledged && state.refusal.is_none());
        assert!(sender.send(Event::Planned(Err(failure("stale")))).is_err());
        state.poll();
        assert!(state.refusal.is_none());
    }

    #[test]
    fn running_export_remains_targeted_and_unknown_interruption_requests_history() {
        let first = game(1);
        let second = game(2);
        let mut state = State::default();
        state.observe_selection(&first.archive.absolute_path, 1, &first, None);
        let (sender, receiver) = mpsc::channel();
        state.job = Some(Job {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            kind: JobKind::Export,
            path: first.archive.absolute_path.clone(),
            library_key: 1,
        });
        state.progress = Some(PbpExportProgress::Packaging);
        state.observe_selection(&second.archive.absolute_path, 1, &second, None);
        assert_eq!(state.progress, Some(PbpExportProgress::Packaging));
        assert_eq!(
            state.job.as_ref().unwrap().path,
            first.archive.absolute_path
        );
        drop(sender);
        state.poll();
        assert!(state.take_history_refresh());
        assert!(!state.take_history_refresh());
        assert!(matches!(
            state.last.as_ref().map(|(_, _, result)| result),
            Some(Err(PbpExportError {
                recovery: Some(PbpRecoveryStatus::Unknown),
                ..
            }))
        ));
    }

    #[test]
    fn running_worker_keeps_card_responsive_and_target_identified() {
        let first = game(1);
        let second = game(2);
        let mut state = State::default();
        state.observe_selection(&first.archive.absolute_path, 1, &first, None);
        let (_sender, receiver) = mpsc::channel();
        state.job = Some(Job {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            kind: JobKind::Export,
            path: first.archive.absolute_path.clone(),
            library_key: 1,
        });
        state.progress = Some(PbpExportProgress::Packaging);
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.show(ui, &second, None, 1);
                });
            },
        );
        let painted = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(painted.contains("Export in progress for another selected game"));
        assert!(painted.contains("Creating private PBP"));
        assert!(painted.contains("Cancel"));
    }

    #[test]
    fn same_path_in_new_library_does_not_show_old_result() {
        let selected = game(1);
        let mut state = State {
            last: Some((
                selected.archive.absolute_path.clone(),
                1,
                Err(failure("old library result")),
            )),
            ..Default::default()
        };
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.show(ui, &selected, None, 2);
                });
            },
        );
        let painted = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!painted.contains("old library result"));
    }

    #[test]
    fn five_disc_confirmation_keeps_acknowledgement_and_action_visible_at_1024x600() {
        let context = egui::Context::default();
        let discs = (1..=5)
            .map(|n| format!("Disc {n} · SLUS-0000{n} · /games/disc{n}.cue"))
            .collect::<Vec<_>>();
        let render = || {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1024.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::Window::new("Confirm derived PBP export")
                        .default_width(520.0)
                        .show(ctx, |ui| {
                            let mut acknowledged = false;
                            assert_eq!(
                                confirmation_contents(
                                    ui,
                                    &discs,
                                    Path::new("/games/derived.pbp"),
                                    &mut acknowledged
                                ),
                                ConfirmationAction::None
                            );
                        });
                },
            )
        };
        let _ = render();
        let output = render();
        assert!(crate::gui_v2::tests::page_reachability::visible_text(
            &output,
            "I will retain the original discs"
        ));
        assert!(crate::gui_v2::tests::page_reachability::visible_text(
            &output,
            "Create verified PBP"
        ));
    }
}
