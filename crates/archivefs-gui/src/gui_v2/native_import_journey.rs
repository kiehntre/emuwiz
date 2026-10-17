//! Small queue over native saved evidence, never a second import or journal.
use archivefs_core::ingestion::{
    bounded_import::BoundedImportPlan,
    preview::{MediaDisposition, MediaPreview},
    saved_discovery::{SavedDiscovery, SubsetPreview},
};
use eframe::egui;
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

pub(super) struct Journey {
    evidence: Arc<SavedDiscovery>,
    manual_rows: Arc<Vec<archivefs_core::ingestion::saved_discovery::SelectionRow>>,
    batches: VecDeque<Vec<String>>,
    total_batches: usize,
    eligible: usize,
    existing: usize,
    excluded: usize,
    not_ready: usize,
    platforms: BTreeMap<String, usize>,
    deferred: Vec<(PathBuf, String)>,
    rejected: Vec<(PathBuf, String)>,
    submitted: usize,
    discovery_notes: Vec<(PathBuf, String)>,
    discovery_problems: Vec<String>,
    notes_page: usize,
    job: Option<Receiver<Result<SubsetPreview, String>>>,
    cancel: Arc<AtomicBool>,
    automatic: bool,
    error: Option<String>,
}

impl Journey {
    /// Called only on the discovery worker: hashing identities and retaining
    /// evidence must not block an egui frame.
    pub(super) fn new(report: &MediaPreview, plan: &BoundedImportPlan) -> Result<Self, String> {
        if plan.candidate_count() > archivefs_core::ingestion::saved_discovery::MAX_SAVED_CANDIDATES
        {
            return Err("Discovery exceeds the native retained-evidence candidate limit. Choose a narrower folder and prepare again.".into());
        }
        let schedule = plan.automatic_batches();
        Ok(Self {
            evidence: Arc::new(plan.saved_evidence()?),
            manual_rows: Arc::new(plan.selection_rows()),
            total_batches: schedule.batches.len(),
            batches: schedule.batches.into(),
            eligible: plan.projected_insertions(),
            existing: report.count(MediaDisposition::Existing)
                + report.count(MediaDisposition::RepresentedArchive),
            excluded: report.count(MediaDisposition::Excluded) + report.omitted_exclusion_details,
            not_ready: report
                .count(MediaDisposition::Eligible)
                .saturating_sub(plan.projected_insertions()),
            platforms: plan.platform_counts(),
            deferred: schedule.deferred,
            rejected: Vec::new(),
            submitted: 0,
            discovery_notes: report
                .items
                .iter()
                .filter(|i| {
                    i.disposition != MediaDisposition::Eligible
                        || !i.relationship_checks_complete
                        || i.alternate
                            .as_deref()
                            .is_some_and(|r| r.starts_with("Ambiguous"))
                })
                .map(|i| {
                    (
                        i.path.clone(),
                        match &i.alternate {
                            Some(reason) => format!("{} {reason}", i.explanation),
                            None => i.explanation.clone(),
                        },
                    )
                })
                .collect(),
            discovery_problems: report.problems.clone(),
            notes_page: 0,
            job: None,
            cancel: Arc::new(AtomicBool::new(false)),
            automatic: true,
            error: None,
        })
    }
    pub(super) fn prepare_first(
        &mut self,
        database: &Path,
        config: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Option<SubsetPreview>, String> {
        let result = self
            .batches
            .front()
            .map(|ids| {
                self.evidence
                    .prepare_subset(ids, database, config, cancel, progress)
            })
            .transpose()?;
        if let Some(p) = &result {
            self.rejected = p.rejected.clone();
        }
        Ok(result)
    }
    pub(super) fn preparation_failed(&mut self, reason: String) {
        self.error = Some(reason);
        self.automatic = false;
    }
    pub(super) fn submitted(&mut self) {
        self.batches.pop_front();
        self.submitted += 1;
        self.automatic = true;
    }
    pub(super) fn continue_after_apply(&mut self) {
        self.automatic = true;
    }
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }
    pub(super) fn cancel_preparation(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.job = None;
        self.automatic = false;
    }
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        database: &Path,
        report: &mut Option<MediaPreview>,
        busy: bool,
    ) {
        ui.strong("Your folder import");
        ui.label(format!(
            "At discovery: {} supported new media | {} already represented | {} excluded | {} automatic batches",
            self.eligible, self.existing, self.excluded, self.total_batches
        ));
        if self.not_ready > 0 {
            ui.label(format!("{} further candidates need complete relationship checks before import. Review comparison problems below; some systems require a compatible validated discovery DAT first. Open DATs & Verification to add or validate one, then discover again.", self.not_ready));
        }
        ui.small(format!("{} batches submitted for Apply. History confirms committed outcomes; a submitted or prepared batch is not a successful import.", self.submitted));
        ui.collapsing("How approval and resume work", |ui| {
            ui.small("Mr Wiz · Review one prepared batch at a time. I choose the batches; you approve every database import. After interruption, select this folder and prepare again: fresh checks protect existing records, and History retains committed receipts. Cancelled or refused files need a fresh discovery before retrying.");
        });
        if let Some(job) = &self.job {
            match job.try_recv() {
                Ok(result) => {
                    self.job = None;
                    if !busy && !self.cancel.load(Ordering::Relaxed) {
                        match result {
                            Ok(p) => {
                                self.rejected = p.rejected.clone();
                                *report = Some(super::native_media_subset::subset_report(p));
                            }
                            Err(e) => {
                                self.error = Some(e);
                                self.automatic = false;
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.error = Some(
                        "Batch preparation stopped. No import was started. Prepare again.".into(),
                    );
                    self.automatic = false;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.spinner();
                    ui.label("Preparing the next exact batch with fresh native checks…");
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        let ready = report
            .as_ref()
            .and_then(|r| r.bounded_import.as_ref())
            .is_some();
        if let Some(e) = &self.error {
            ui.label(e);
        }
        if !self.batches.is_empty() && !ready && !busy && self.job.is_none() {
            if self.automatic || ui.button("Prepare next safe batch").clicked() {
                self.automatic = false;
                self.error = None;
                let ids = self.batches.front().unwrap().clone();
                let evidence = self.evidence.clone();
                let database = database.to_path_buf();
                self.cancel = Arc::new(AtomicBool::new(false));
                let cancel = self.cancel.clone();
                let ctx = ui.ctx().clone();
                let (tx, rx) = mpsc::channel();
                self.job = Some(rx);
                std::thread::spawn(move || {
                    let result = archivefs_core::default_config_path()
                        .map_err(|e| e.to_string())
                        .and_then(|cfg| {
                            evidence.prepare_subset(&ids, &database, &cfg, &cancel, &mut |_, _| {})
                        });
                    let _ = tx.send(result);
                    ctx.request_repaint();
                });
            }
        }
        if ready
            && report
                .as_ref()
                .and_then(|r| r.bounded_import.as_ref())
                .is_some_and(|p| p.projected_insertions() == 0)
        {
            ui.label("No eligible files remain in this batch. Reasons are shown below; nothing has been imported.");
            if ui.button("Continue past refused batch").clicked() {
                self.batches.pop_front();
                self.automatic = true;
                *report = None;
            }
        } else if ready {
            ui.label(format!(
                "Review batch {} of {} below.",
                self.total_batches - self.batches.len() + 1,
                self.total_batches
            ));
        } else if self.batches.is_empty() {
            ui.label(if self.total_batches == 0 { "No safe import batches are available. Review exclusions and comparison problems, then discover again after resolving them." }
                else { "All prepared batches have been submitted or refused. Check History for committed results, then verify imported games below." });
        }
        ui.collapsing("Discovery exclusions and assessment problems", |ui| {
            self.notes_page = self.notes_page.min(self.discovery_notes.len().saturating_sub(1) / 25);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(self.notes_page > 0, egui::Button::new("Previous exclusions")).clicked() { self.notes_page -= 1; }
                if ui.add_enabled((self.notes_page + 1) * 25 < self.discovery_notes.len(), egui::Button::new("Next exclusions")).clicked() { self.notes_page += 1; }
                ui.label(format!("{} retained explanations | page {}", self.discovery_notes.len(), self.notes_page + 1));
            });
            for (path, reason) in self.discovery_notes.iter().skip(self.notes_page * 25).take(25) { ui.label(format!("{}: {reason}", path.display())); }
            for problem in self.discovery_problems.iter().take(25) { ui.label(problem); }
            ui.small("Discovery samples may omit further exclusions. These counts are media records, not unique-game or verification counts.");
        });
        if self.job.is_some() && ui.button("Cancel batch preparation").clicked() {
            self.cancel_preparation();
        }
        if !self.deferred.is_empty() || !self.rejected.is_empty() {
            ui.collapsing("Files needing attention before import", |ui| {
                for (path, reason) in self.deferred.iter().chain(&self.rejected).take(50) { ui.label(format!("{}: {reason}", path.display())); }
                ui.small("At most 50 reasons shown here. Exact exclusions remain in the batch and its receipt.");
            });
        }
    }
    pub(super) fn manual_evidence(
        &self,
    ) -> (
        Arc<SavedDiscovery>,
        Arc<Vec<archivefs_core::ingestion::saved_discovery::SelectionRow>>,
    ) {
        (self.evidence.clone(), self.manual_rows.clone())
    }
    pub(super) fn platforms(&self) -> impl Iterator<Item = &String> {
        self.platforms.keys()
    }
}
impl Drop for Journey {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "native_import_journey_tests.rs"]
mod tests;
