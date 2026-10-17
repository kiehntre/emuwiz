//! Exact selection over retained native Discovery evidence. All filesystem work runs off-frame.
use archivefs_core::ingestion::{
    bounded_import::BoundedImportPlan,
    preview::{MediaDisposition, MediaPreview, MediaPreviewItem},
    saved_discovery::{SavedDiscovery, SelectionRow, SubsetPreview, load_saved_evidence},
};
use eframe::egui;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};
enum ResultValue {
    Loaded(SavedDiscovery, Vec<SelectionRow>),
    Captured(SavedDiscovery, Vec<SelectionRow>),
    Saved(String),
    Prepared(SubsetPreview),
}
#[derive(Default)]
pub(super) struct State {
    evidence: Option<Arc<SavedDiscovery>>,
    rows: Arc<Vec<SelectionRow>>,
    selected: BTreeSet<String>,
    request_path: String,
    request_pin: String,
    path: String,
    pin: String,
    job: Option<Receiver<Result<ResultValue, String>>>,
    cancel: Arc<AtomicBool>,
    page: usize,
    batch: usize,
    message: String,
}
impl State {
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }
    #[cfg(test)]
    pub(super) fn load_for_test(&mut self, ctx: egui::Context, path: PathBuf, pin: String) {
        self.start(ctx, move |_| {
            let e = load_saved_evidence(&path, &pin)?;
            let rows = e.rows();
            Ok(ResultValue::Loaded(e, rows))
        });
    }
    #[cfg(test)]
    pub(super) fn selected_for_test(&self) -> usize {
        self.selected.len()
    }
    #[cfg(test)]
    pub(super) fn ready_for_test(&self) -> bool {
        self.evidence.is_some() && !self.busy()
    }
    pub(super) fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.evidence = None;
        self.rows = Arc::new(Vec::new());
        self.selected.clear();
    }
    fn start(
        &mut self,
        ctx: egui::Context,
        f: impl FnOnce(Arc<AtomicBool>) -> Result<ResultValue, String> + Send + 'static,
    ) {
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        std::thread::spawn(move || {
            let r = f(cancel);
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }
    fn start_explicit(
        &mut self,
        ctx: egui::Context,
        database: PathBuf,
        path: PathBuf,
        pin: String,
        report: &mut Option<MediaPreview>,
    ) {
        self.evidence = None;
        self.rows = Arc::new(Vec::new());
        self.selected.clear();
        *report = None;
        self.start(ctx, move |cancel| {
            let cfg = archivefs_core::default_config_path().map_err(|e| e.to_string())?;
            archivefs_core::ingestion::explicit_preview::preview_pinned_paths(
                &database,
                &cfg,
                &path,
                &pin,
                &cancel,
                &mut |_, _| {},
            )
            .map(ResultValue::Prepared)
        });
    }
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        database: &Path,
        report: &mut Option<MediaPreview>,
        busy: bool,
    ) -> bool {
        let mut invalidated = false;
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok(result) => {
                    self.job = None;
                    if !self.cancel.load(Ordering::Relaxed) && !busy {
                        match result {
                            Ok(ResultValue::Captured(e, rows)) => {
                                self.evidence = Some(Arc::new(e));
                                self.rows = Arc::new(rows);
                            }
                            Ok(ResultValue::Loaded(e, rows)) => {
                                self.evidence = Some(Arc::new(e));
                                self.rows = Arc::new(rows);
                                self.selected.clear();
                                *report = None;
                                self.message="Saved evidence loaded read-only. Select exact candidate identities.".into();
                                invalidated = true;
                            }
                            Ok(ResultValue::Saved(pin)) => {
                                self.pin = pin;
                                self.message =
                                    "Saved. Retain this SHA-256 separately; reload requires it."
                                        .into();
                            }
                            Ok(ResultValue::Prepared(p)) => {
                                self.message = format!(
                                    "Saved eligible: {} | selected: {} | eligible now: {} | refused: {} | unselected: {} | projected insertions: {}",
                                    p.saved_eligible,
                                    p.requested,
                                    p.plan.projected_insertions(),
                                    p.rejected.len(),
                                    p.unselected,
                                    p.plan.projected_insertions()
                                );
                                let mut r = MediaPreview::default();
                                r.charged_input_bytes = p.inspected_input_bytes;
                                r.catalogue_basis = p.plan.catalogue_basis().to_vec();
                                r.roots.push(p.plan.root().into());
                                for row in p.plan.selection_rows() {
                                    r.items.push(MediaPreviewItem {
                                        path: row.path,
                                        disposition: MediaDisposition::Eligible,
                                        explanation: format!(
                                            "{} | {} | exact selected identity {}",
                                            row.platform, row.kind, row.id
                                        ),
                                        alternate: row.alternate,
                                        multipart: row.multipart,
                                        relationship_checks_complete: row.eligible,
                                    });
                                }
                                for (path, reason) in p.rejected {
                                    r.items.push(MediaPreviewItem {
                                        path,
                                        disposition: MediaDisposition::Excluded,
                                        explanation: reason,
                                        alternate: None,
                                        multipart: None,
                                        relationship_checks_complete: false,
                                    });
                                }
                                r.bounded_import = Some(p.plan);
                                *report = Some(r);
                                invalidated = true;
                            }
                            Err(e) => {
                                self.message = e;
                                *report = None;
                                invalidated = true;
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.message = "Evidence worker stopped; no changes made.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        // Digesting retained evidence is worker work too;
        // Subsets keep their parent pool.
        if self.evidence.is_none()
            && self.job.is_none()
            && !busy
            && let Some(plan) = report.as_ref().and_then(|r| r.bounded_import.as_ref())
        {
            let plan = plan.clone();
            self.start(ui.ctx().clone(), move |_| {
                let e = plan.saved_evidence()?;
                let rows = e.rows();
                Ok(ResultValue::Captured(e, rows))
            });
        }
        ui.collapsing("Select an exact bounded batch / saved evidence", |ui| {
            ui.small("Loading and selection are read-only. Subset preparation visits selected paths and explicit references only. Apply requires separate confirmation.");
            let enabled = !busy && self.job.is_none();
            if let Some(evidence) = self.evidence.clone() {
                let rows = self.rows.clone();
                let eligible = rows.iter().filter(|r| r.eligible).count();
                ui.label(format!("Saved scope: {} | eligible {eligible} | selected {} | excluded {}", evidence.root().display(), self.selected.len(), evidence.excluded_count()));
                ui.horizontal_wrapped(|ui| {
                    if self.batch == 0 { self.batch = 100; }
                    ui.add_enabled(enabled, egui::DragValue::new(&mut self.batch).range(1..=1000));
                    if ui.add_enabled(enabled, egui::Button::new("Select first bounded batch")).clicked() {
                        self.selected = rows.iter().filter(|r| r.eligible).take(self.batch).map(|r| r.id.clone()).collect();
                        invalidated = true;
                        *report = None;
                    }
                    if ui.add_enabled(enabled, egui::Button::new("Clear selection")).clicked() {
                        self.selected.clear();
                        *report = None;
                        invalidated = true;
                    }
                    if ui.add_enabled(enabled && !self.selected.is_empty() && self.selected.len() <= 1000, egui::Button::new("Prepare exact subset preview")).clicked() {
                        let ids: Vec<_> = self.selected.iter().cloned().collect();
                        let db = database.to_path_buf();
                        *report = None;
                        invalidated = true;
                        self.start(ui.ctx().clone(), move |cancel| {
                            let cfg = archivefs_core::default_config_path().map_err(|e| e.to_string())?;
                            evidence.prepare_subset(&ids, &db, &cfg, &cancel, &mut |_, _| {}).map(ResultValue::Prepared)
                        });
                    }
                });
                self.page = self.page.min(rows.len().saturating_sub(1) / 25);
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(enabled && self.page > 0, egui::Button::new("Previous candidates")).clicked() { self.page -= 1; }
                    if ui.add_enabled(enabled && (self.page + 1) * 25 < rows.len(), egui::Button::new("Next candidates")).clicked() { self.page += 1; }
                    ui.label(format!("{} candidates | page {}", rows.len(), self.page + 1));
                });
                egui::ScrollArea::vertical().id_salt("bounded_subset_selection").max_height(140.0).show(ui, |ui| {
                    for row in rows.iter().skip(self.page * 25).take(25) {
                        let mut selected = self.selected.contains(&row.id);
                        let label = format!("{} | {} | {} | {} companions", row.path.display(), row.platform, &row.id[..12], row.companion_count);
                        if ui.add_enabled(enabled && row.eligible, egui::Checkbox::new(&mut selected, label)).changed() {
                            if selected { self.selected.insert(row.id.clone()); } else { self.selected.remove(&row.id); }
                            *report = None;
                            invalidated = true;
                        }
                    }
                });
            }
            ui.collapsing("Prepare from reviewed paths without a root scan", |ui| {
                ui.small("Read-only request: 1–1000 explicit paths and preserved exclusions. Each path is freshly inspected; native source and content checks still apply. No directory walk.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Reviewed path request:");
                    ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.request_path).desired_width(360.0));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Request SHA-256:");
                    ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.request_pin).desired_width(360.0));
                });
                if ui.add_enabled(enabled && !self.request_path.is_empty() && !self.request_pin.is_empty(), egui::Button::new("Prepare exact path preview")).clicked() {
                    let path = PathBuf::from(self.request_path.clone());
                    let pin = self.request_pin.clone();
                    let db = database.to_path_buf();
                    invalidated = true;
                    self.start_explicit(ui.ctx().clone(), db, path, pin, report);
                }
            });
            ui.collapsing("Save or reload pinned evidence", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Evidence file (outside ROM roots):");
                    ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.path).desired_width(420.0));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Separately retained SHA-256:");
                    ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.pin).desired_width(420.0));
                });
                ui.small("Load only native evidence you saved and whose SHA-256 you retained separately. A recomputed digest does not authenticate an edited file.");
                ui.horizontal_wrapped(|ui| {
                    let plan = report.as_ref().and_then(|r| r.bounded_import.as_ref());
                    if ui.add_enabled(enabled && !self.path.is_empty() && plan.is_some(), egui::Button::new("Save reviewed evidence")).clicked() {
                        let plan: BoundedImportPlan = plan.unwrap().clone();
                        let path = PathBuf::from(&self.path);
                        self.start(ui.ctx().clone(), move |_| plan.save_evidence(&path).map(ResultValue::Saved));
                    }
                    if ui.add_enabled(enabled && !self.path.is_empty() && !self.pin.is_empty(), egui::Button::new("Load pinned evidence")).clicked() {
                        let path = PathBuf::from(&self.path);
                        let pin = self.pin.clone();
                        *report = None;
                        invalidated = true;
                        self.start(ui.ctx().clone(), move |_| {
                            let e = load_saved_evidence(&path, &pin)?;
                            let rows = e.rows();
                            Ok(ResultValue::Loaded(e, rows))
                        });
                    }
                });
            });
            if self.job.is_some() {
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.label("Validating bounded evidence…");
                    if ui.button("Cancel evidence operation").clicked() { self.cancel.store(true, Ordering::Relaxed); }
                });
            }
            ui.label(&self.message);
        });
        invalidated || self.job.is_some()
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_request_worker_invalidates_old_preview_and_never_creates_library() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("missing-library.sqlite3");
        let request = t.path().join("request.json");
        std::fs::write(&request, b"{}").unwrap();
        let ctx = egui::Context::default();
        let mut state = State::default();
        state.selected.insert("old-selected-identity".into());
        let mut report = Some(MediaPreview::default());
        state.start_explicit(
            ctx.clone(),
            db.clone(),
            request,
            "0".repeat(64),
            &mut report,
        );
        assert!(report.is_none());
        assert!(state.selected.is_empty());
        assert!(state.busy());
        for _ in 0..100 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.show(ui, &db, &mut report, false);
                });
            });
            if !state.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!state.busy());
        assert!(report.is_none());
        assert!(state.message.contains("SHA-256 changed"));
        assert!(!db.exists());
    }
}
