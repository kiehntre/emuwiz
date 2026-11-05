//! Blue's front end for the existing emulator update workflow.
//!
//! This module adds no update logic.  Every decision, check and file operation is
//! made by `archivefs_core::emulator_update` (`check_updates`,
//! `plan_staged_update`, `execute_staged_update`, `rollback_staged_update`,
//! `recover_update`).  What Blue adds is presentation (availability, preview,
//! typed confirmation, progress, Undo) and a background worker so network and
//! file work never freezes the window.  Results are applied through the same
//! `finish_*` functions Gold uses, so state transitions are identical.

use super::{
    EmulatorInventoryPageState, UpdateReview, channel_label, eligibility_label,
    installation_type_label, rollback_confirmation_phrase, update_artifact,
    update_confirmation_phrase, update_status_label,
};
use archivefs_core::emulator_inventory::EmulatorInstallation;
use archivefs_core::emulator_update::{
    self, HttpsUpdateDownloader, OfficialMetadataProvider, QuiescenceEvidence, UpdateArtifact,
    UpdateDownloader, UpdateExecutionEligibility, UpdateExecutionError, UpdateJournal,
    UpdateReport, UpdateResult, UpdateStatus, actionable_undo, execute_staged_update,
    plan_staged_update, probe_executable_quiescence, probe_update_quiescence, recover_update,
    rollback_staged_update,
};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// Everything that touches the network or the process table.  The live
/// implementation is the only one used outside tests; tests substitute
/// disposable fixtures.  No method can relax a core check: the core still
/// validates the plan, artifact checksum, quiescence and filesystem identity.
pub(super) trait UpdateBackend: Send + Sync {
    fn check(&self, installations: &[EmulatorInstallation]) -> UpdateReport;
    fn artifact(
        &self,
        installation: &EmulatorInstallation,
        update: &UpdateResult,
    ) -> Result<UpdateArtifact, String>;
    fn downloader(&self) -> Box<dyn UpdateDownloader + Send>;
    fn apply_quiescence(&self, tracked: bool, executable: &Path) -> QuiescenceEvidence;
    fn journal_quiescence(&self, tracked: bool, journal: &UpdateJournal) -> QuiescenceEvidence;
}

struct LiveBackend;

impl UpdateBackend for LiveBackend {
    fn check(&self, installations: &[EmulatorInstallation]) -> UpdateReport {
        emulator_update::check_updates(installations, &mut OfficialMetadataProvider::default())
    }
    fn artifact(
        &self,
        installation: &EmulatorInstallation,
        update: &UpdateResult,
    ) -> Result<UpdateArtifact, String> {
        update_artifact(installation, update)
    }
    fn downloader(&self) -> Box<dyn UpdateDownloader + Send> {
        Box::new(HttpsUpdateDownloader)
    }
    fn apply_quiescence(&self, tracked: bool, executable: &Path) -> QuiescenceEvidence {
        QuiescenceEvidence::from_tracked_flag(tracked)
            .combine(probe_executable_quiescence(executable))
    }
    fn journal_quiescence(&self, tracked: bool, journal: &UpdateJournal) -> QuiescenceEvidence {
        QuiescenceEvidence::from_tracked_flag(tracked).combine(probe_update_quiescence(journal))
    }
}

type Phase = Arc<Mutex<&'static str>>;

/// Reports the download/verify phases to the progress line.
struct PhaseDownloader {
    inner: Box<dyn UpdateDownloader + Send>,
    phase: Phase,
}

impl UpdateDownloader for PhaseDownloader {
    fn download(
        &mut self,
        url: &str,
        destination: &mut std::fs::File,
    ) -> Result<(), UpdateExecutionError> {
        set_phase(
            &self.phase,
            "Downloading the update from its published source…",
        );
        let result = self.inner.download(url, destination);
        set_phase(
            &self.phase,
            "Download finished. Checking its SHA-256 and replacing the executable…",
        );
        result
    }
}

fn set_phase(phase: &Phase, text: &'static str) {
    if let Ok(mut slot) = phase.lock() {
        *slot = text;
    }
}

enum JobDone {
    Checked(UpdateReport),
    Prepared(Result<Box<UpdateReview>, String>),
    Applied(Box<Result<UpdateJournal, UpdateExecutionError>>),
    RolledBack(
        Box<UpdateJournal>,
        Box<Result<UpdateJournal, UpdateExecutionError>>,
    ),
    Recovered(Box<Result<UpdateJournal, UpdateExecutionError>>),
}

struct Job {
    phase: Phase,
    rx: mpsc::Receiver<JobDone>,
    /// Installation directory to re-read if the worker dies without reporting.
    root: PathBuf,
}

#[derive(Default)]
pub(super) struct BlueUpdateState {
    job: Option<Job>,
    /// Live "an EmuWiz-launched emulator is running" flag.  Written every frame
    /// by the app and read by the worker at the moment each quiescence check
    /// runs, so a launch started during a long download is still seen.
    tracked: Arc<AtomicBool>,
    backend: Option<Arc<dyn UpdateBackend>>,
}

/// What the user asked for during one frame; applied after drawing.
enum Intent {
    Check,
    Review(Box<(EmulatorInstallation, UpdateResult)>),
    CloseReview,
    Apply,
    ReviewUndo(PathBuf),
    Undo,
    Recover(PathBuf, Box<UpdateJournal>),
}

impl EmulatorInventoryPageState {
    fn backend(&self) -> Arc<dyn UpdateBackend> {
        self.blue
            .backend
            .clone()
            .unwrap_or_else(|| Arc::new(LiveBackend))
    }

    /// Per-frame hook from the app: keeps the live running flag current and
    /// collects any finished operation even when the page is not on screen.
    pub(crate) fn sync_blue(&mut self, emulator_running: bool) {
        self.set_tracked(emulator_running);
        self.poll_blue();
    }

    fn set_tracked(&mut self, running: bool) {
        self.tracked_running = running;
        self.blue.tracked.store(running, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn blue_busy(&self) -> bool {
        self.blue.job.is_some()
    }

    fn spawn<F>(&mut self, ctx: &egui::Context, root: PathBuf, first_phase: &'static str, work: F)
    where
        F: FnOnce(Phase) -> JobDone + Send + 'static,
    {
        if self.blue.job.is_some() {
            return;
        }
        let phase: Phase = Arc::new(Mutex::new(first_phase));
        let (tx, rx) = mpsc::channel();
        let worker_phase = phase.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let done = work(worker_phase);
            let _ = tx.send(done);
            ctx.request_repaint();
        });
        self.blue.job = Some(Job { phase, rx, root });
    }

    /// Collects a finished operation, if any.  Returns true while one is running.
    pub(crate) fn poll_blue(&mut self) -> bool {
        let Some(job) = &self.blue.job else {
            return false;
        };
        let done = match job.rx.try_recv() {
            Ok(done) => done,
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => {
                // The worker died without reporting: the disk may have changed.
                let root = job.root.clone();
                self.blue.job = None;
                if root.as_os_str().is_empty() {
                    // A read-only Check owns no installation: nothing on disk
                    // can have changed.
                    self.error = Some(
                        "The update check stopped unexpectedly. Nothing was changed; try again."
                            .into(),
                    );
                    return false;
                }
                self.reload_after_failed_operation(&root);
                self.feedback = None;
                self.error = Some(
                    "The update worker stopped unexpectedly. Saved update records were re-read; check \"Interrupted or unreadable update records\" before trying again."
                        .into(),
                );
                return false;
            }
        };
        let root = job.root.clone();
        self.blue.job = None;
        match done {
            JobDone::Checked(report) => {
                self.error = None;
                self.feedback = None;
                // A preview was built from the previous check; never keep it
                // (or its typed confirmation) next to fresh results.
                self.review = None;
                self.confirmation.clear();
                self.update_report = Some(report);
            }
            JobDone::Prepared(Ok(review)) => {
                self.error = None;
                self.confirmation.clear();
                self.review = Some(*review);
            }
            JobDone::Prepared(Err(message)) => {
                self.review = None;
                self.error = Some(message);
            }
            JobDone::Applied(result) => self.finish_apply(&root, *result),
            JobDone::RolledBack(journal, result) => self.finish_rollback(&journal, *result),
            JobDone::Recovered(result) => {
                self.feedback = None;
                self.finish_recover(*result);
            }
        }
        false
    }

    fn start_check(&mut self, ctx: &egui::Context) {
        if self.inventory.is_none() {
            self.refresh();
        }
        let Some(inventory) = self.inventory.clone() else {
            return;
        };
        let backend = self.backend();
        self.spawn(
            ctx,
            PathBuf::new(),
            "Asking each emulator's official release source for its latest version…",
            move |_| JobDone::Checked(backend.check(&inventory.installations)),
        );
    }

    fn start_review(
        &mut self,
        ctx: &egui::Context,
        installation: EmulatorInstallation,
        update: UpdateResult,
    ) {
        let backend = self.backend();
        let tracked = self.blue.tracked.clone();
        let root = installation.installation_root.clone();
        self.spawn(
            ctx,
            root,
            "Looking up the official download and its published checksum…",
            move |_| {
                let artifact = match backend.artifact(&installation, &update) {
                    Ok(artifact) => artifact,
                    Err(error) => return JobDone::Prepared(Err(error)),
                };
                let evidence = backend.apply_quiescence(
                    tracked.load(Ordering::SeqCst),
                    &installation.executable_path,
                );
                let plan = plan_staged_update(&installation, &update, artifact, evidence);
                JobDone::Prepared(Ok(Box::new(UpdateReview {
                    installation,
                    update,
                    plan,
                })))
            },
        );
    }

    fn start_apply(&mut self, ctx: &egui::Context) {
        let Some(review) = self.review.clone() else {
            return;
        };
        let expected = update_confirmation_phrase(review.installation.emulator);
        if self.confirmation.trim() != expected {
            self.error = Some(format!("Type {expected} exactly to confirm this update."));
            return;
        }
        if review.plan.eligibility != UpdateExecutionEligibility::Ready {
            self.error = Some(format!("Update is blocked: {:?}.", review.plan.eligibility));
            return;
        }
        let backend = self.backend();
        let tracked = self.blue.tracked.clone();
        let root = review.installation.installation_root.clone();
        self.error = None;
        self.spawn(
            ctx,
            root,
            "Re-checking that the emulator is stopped…",
            move |phase| {
                let target = review.plan.target_path.clone();
                let quiescence = {
                    let backend = backend.clone();
                    // Re-evaluated at apply time and again by the core right
                    // before the executable moves.
                    move || backend.apply_quiescence(tracked.load(Ordering::SeqCst), &target)
                };
                let mut downloader = PhaseDownloader {
                    inner: backend.downloader(),
                    phase,
                };
                JobDone::Applied(Box::new(execute_staged_update(
                    &review.plan,
                    &review.installation,
                    &review.update,
                    quiescence,
                    &mut downloader,
                )))
            },
        );
    }

    fn start_undo(&mut self, ctx: &egui::Context) {
        let Some(journal) = self.review_journal.clone() else {
            return;
        };
        let expected = rollback_confirmation_phrase(journal.emulator);
        if self.rollback_confirmation.trim() != expected {
            self.error = Some(format!("Type {expected} exactly to confirm Undo."));
            return;
        }
        let backend = self.backend();
        let tracked = self.blue.tracked.clone();
        let root = journal
            .target_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        self.error = None;
        self.spawn(
            ctx,
            root,
            "Checking both executables against their recorded hashes and restoring the previous one…",
            move |_| {
                let probe = journal.clone();
                let result =
                    rollback_staged_update(&journal, move || backend.journal_quiescence(tracked.load(Ordering::SeqCst), &probe));
                JobDone::RolledBack(Box::new(journal), Box::new(result))
            },
        );
    }

    fn start_recover(&mut self, ctx: &egui::Context, record: PathBuf, journal: UpdateJournal) {
        let backend = self.backend();
        let tracked = self.blue.tracked.clone();
        let root = journal
            .target_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        self.spawn(
            ctx,
            root,
            "Reconciling the interrupted update from file hashes…",
            move |_| {
                JobDone::Recovered(Box::new(recover_update(&record, move || {
                    backend.journal_quiescence(tracked.load(Ordering::SeqCst), &journal)
                })))
            },
        );
    }

    /// Blue's "Emulator updates" panel.  Returns nothing; all effects go through
    /// the shared state and the background worker.
    pub(crate) fn show_blue(&mut self, ui: &mut egui::Ui, emulator_running: bool) {
        self.set_tracked(emulator_running);
        let busy = self.poll_blue();
        let ctx = ui.ctx().clone();
        let mut intent = None;

        ui.heading("Emulator updates");
        ui.label(
            "EmuWiz replaces one standalone emulator program at a time. It only updates an install it can prove is its own, from the emulator's official release, after the download matches a published SHA-256 and the emulator is proven stopped. Package-manager and Flatpak installs are updated by their own tools.",
        );
        ui.label("The previous program is always kept, and Undo puts it back. Nothing is deleted.");

        if busy {
            let phase = self
                .blue
                .job
                .as_ref()
                .and_then(|job| job.phase.lock().ok().map(|p| *p))
                .unwrap_or("Working…");
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(phase);
            });
            ui.small(
                "Please keep EmuWiz open. Other controls here are paused until this finishes.",
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Check for emulator updates"))
            .on_hover_text("Contacts each emulator's official release source. Nothing is changed.")
            .clicked()
        {
            intent = Some(Intent::Check);
        }
        if emulator_running {
            ui.colored_label(
                egui::Color32::from_rgb(235, 178, 76),
                "An EmuWiz-launched emulator is running, so updates are blocked until it is closed.",
            );
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(235, 120, 100), error);
        }
        if let Some(feedback) = &self.feedback {
            ui.colored_label(egui::Color32::from_rgb(110, 190, 120), feedback);
        }

        self.blue_availability(ui, busy, &mut intent);
        self.blue_review(ui, busy, &mut intent);
        self.blue_undo(ui, busy, &mut intent);
        self.blue_recovery(ui, busy, &mut intent);

        match intent {
            Some(Intent::Check) => self.start_check(&ctx),
            Some(Intent::Review(pair)) => {
                let (installation, update) = *pair;
                self.start_review(&ctx, installation, update)
            }
            Some(Intent::CloseReview) => {
                self.review = None;
                self.confirmation.clear();
            }
            Some(Intent::Apply) => self.start_apply(&ctx),
            Some(Intent::ReviewUndo(target)) => self.review_undo_for(&target),
            Some(Intent::Undo) => self.start_undo(&ctx),
            Some(Intent::Recover(record, journal)) => self.start_recover(&ctx, record, *journal),
            None => {}
        }
    }

    fn blue_availability(&self, ui: &mut egui::Ui, busy: bool, intent: &mut Option<Intent>) {
        let Some(report) = &self.update_report else {
            ui.label("Not checked yet.");
            return;
        };
        let Some(inventory) = &self.inventory else {
            return;
        };
        if report.results.is_empty() {
            ui.label("No installed emulator was found to check.");
        }
        for result in &report.results {
            let installation = inventory
                .installations
                .iter()
                .find(|i| i.executable_path == result.executable_path);
            ui.push_id(("blue_update", &result.executable_path), |ui| {
                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(result.emulator.label());
                        ui.label(update_status_label(result.status));
                        ui.label(format!(
                            "Installed {} · Latest {}",
                            result.installed_version.as_deref().unwrap_or("unknown"),
                            result.available_version.as_deref().unwrap_or("unknown")
                        ));
                    });
                    ui.small(result.executable_path.display().to_string());
                    if let Some(warning) = &result.warning {
                        ui.small(warning);
                    }
                    if result.status == UpdateStatus::UpdateAvailable
                        && let Some(installation) = installation
                        && ui
                            .add_enabled(!busy, egui::Button::new("Review update"))
                            .clicked()
                    {
                        *intent = Some(Intent::Review(Box::new((
                            installation.clone(),
                            result.clone(),
                        ))));
                    }
                });
            });
        }
    }

    fn blue_review(&mut self, ui: &mut egui::Ui, busy: bool, intent: &mut Option<Intent>) {
        let Some(review) = self.review.clone() else {
            return;
        };
        let plan = &review.plan;
        ui.separator();
        ui.heading(format!(
            "Review {} update",
            review.installation.emulator.label()
        ));
        ui.label("Nothing changes while you review.");
        for (label, value) in [
            ("Installed version", plan.installed_version.clone()),
            ("New version", plan.new_version.clone()),
            ("Channel", channel_label(plan.new_channel).to_string()),
            (
                "Install type",
                installation_type_label(plan.installation_type).to_string(),
            ),
            ("Program to replace", plan.target_path.display().to_string()),
            ("Official download", plan.artifact.provenance.clone()),
            (
                "Published SHA-256",
                plan.artifact
                    .sha256
                    .clone()
                    .unwrap_or_else(|| "Unavailable".into()),
            ),
            (
                "Safety checks",
                eligibility_label(plan.eligibility).to_string(),
            ),
        ] {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("{label}:"));
                ui.label(value);
            });
        }
        if plan.save_state_warning {
            ui.colored_label(
                egui::Color32::from_rgb(235, 178, 76),
                "Save states made by the old version may not load in the new one. Your save files are not touched.",
            );
        }
        if let Some(warning) = &plan.warning {
            ui.label(warning);
        }
        if plan.eligibility == UpdateExecutionEligibility::Ready {
            ui.label(
                "Nothing is changed until you type the confirmation below and press Update now.",
            );
            ui.label("What will happen: the file is downloaded and checked against the published SHA-256; the emulator is checked again to be stopped; the current program is moved aside and kept; the new program takes its place. If anything looks wrong, nothing is replaced.");
            let expected = update_confirmation_phrase(review.installation.emulator);
            ui.label(format!("Type {expected} to confirm."));
            ui.add_enabled(!busy, egui::TextEdit::singleline(&mut self.confirmation));
            let typed = self.confirmation.trim() == expected;
            if ui
                .add_enabled(!busy && typed, egui::Button::new("Update now"))
                .clicked()
            {
                *intent = Some(Intent::Apply);
            }
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(235, 178, 76),
                "This update cannot be applied safely right now. EmuWiz will not work around that; nothing was changed.",
            );
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Close review"))
            .clicked()
        {
            *intent = Some(Intent::CloseReview);
        }
    }

    fn blue_undo(&mut self, ui: &mut egui::Ui, busy: bool, intent: &mut Option<Intent>) {
        let mut targets: Vec<PathBuf> = self
            .records
            .iter()
            .filter_map(|e| e.journal.as_ref().ok())
            .map(|j| j.target_path.clone())
            .collect();
        targets.sort();
        targets.dedup();
        let mut heading = false;
        for target in targets {
            if actionable_undo(&self.records, &target).is_none() {
                continue;
            }
            if !heading {
                ui.separator();
                ui.heading("Undo an update");
                heading = true;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(target.display().to_string());
                if ui
                    .add_enabled(!busy, egui::Button::new("Review Undo"))
                    .clicked()
                {
                    *intent = Some(Intent::ReviewUndo(target.clone()));
                }
            });
        }
        if let Some(journal) = self.review_journal.clone() {
            ui.group(|ui| {
                ui.strong(format!("Undo {} update", journal.emulator.label()));
                ui.label(format!(
                    "Restores {} {} at {}. Undo re-checks the installed program and the kept copy by hash and refuses if either changed. The newer program is kept beside it, not deleted.",
                    journal.emulator.label(),
                    journal.old_version,
                    journal.target_path.display()
                ));
                let expected = rollback_confirmation_phrase(journal.emulator);
                ui.label(format!("Type {expected} to confirm."));
                ui.add_enabled(!busy, egui::TextEdit::singleline(&mut self.rollback_confirmation));
                let typed = self.rollback_confirmation.trim() == expected;
                if ui
                    .add_enabled(!busy && typed, egui::Button::new("Undo update"))
                    .clicked()
                {
                    *intent = Some(Intent::Undo);
                }
            });
        }
    }

    fn blue_recovery(&self, ui: &mut egui::Ui, busy: bool, intent: &mut Option<Intent>) {
        let attention: Vec<_> = self
            .records
            .iter()
            .filter(|e| e.needs_attention())
            .collect();
        if attention.is_empty() {
            return;
        }
        ui.separator();
        ui.heading("Recovery required");
        ui.label("An earlier update or Undo did not finish. Recovery only acts when file hashes prove the outcome; otherwise it changes nothing and says why. No program is deleted.");
        for entry in attention {
            match &entry.journal {
                Ok(journal) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{} {} → {}: {:?} — {}",
                            journal.emulator.label(),
                            journal.old_version,
                            journal.new_version,
                            journal.state,
                            journal.target_path.display()
                        ));
                        if ui
                            .add_enabled(!busy, egui::Button::new("Recover"))
                            .clicked()
                        {
                            *intent = Some(Intent::Recover(
                                entry.path.clone(),
                                Box::new(journal.clone()),
                            ));
                        }
                    });
                }
                Err(problem) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(235, 120, 100),
                        format!(
                            "Record needs manual review at {}: {problem}. Undo and recovery are refused; nothing was changed.",
                            entry.path.display()
                        ),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
