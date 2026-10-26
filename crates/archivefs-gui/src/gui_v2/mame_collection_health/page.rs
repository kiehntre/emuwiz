use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
use std::time::{Duration, Instant};

use archivefs_core::dat::catalogue_selection::CatalogueRef;
use archivefs_core::mame_collection_analyser::inspection::InspectionProgress;
use eframe::egui;

use super::{
    view,
    worker::{self, Report, Request},
};
use crate::dat_catalogue_picker::{DatCataloguePickerState, DatCatalogueWorkflow};

pub(in crate::gui_v2) enum Message {
    Progress(u64, InspectionProgress),
    Finished(u64, Result<Report, String>),
}
pub(in crate::gui_v2) struct Job {
    pub request: Request,
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<Message>,
}

struct SourceCheck {
    report: Arc<Report>,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<bool>,
}
impl Drop for SourceCheck {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::gui_v2) enum Filter {
    #[default]
    All,
    Good,
    Bad,
    Unknown,
}

#[derive(Default)]
pub(in crate::gui_v2) struct HealthPage {
    pub(in crate::gui_v2) picker: DatCataloguePickerState,
    pub(in crate::gui_v2) selected: Option<CatalogueRef>,
    pub(in crate::gui_v2) root: String,
    pub(in crate::gui_v2) version_output: String,
    pub(in crate::gui_v2) report: Option<Arc<Report>>,
    pub(in crate::gui_v2) job: Option<Job>,
    pub(in crate::gui_v2) progress: Option<InspectionProgress>,
    pub(in crate::gui_v2) message: Option<String>,
    pub(in crate::gui_v2) query: String,
    pub(in crate::gui_v2) filter: Filter,
    pub(in crate::gui_v2) selected_set: Option<String>,
    pub(in crate::gui_v2) offset: usize,
    pub(in crate::gui_v2) generation: u64,
    pub(super) repair: super::repair::State,
    source_check: Option<SourceCheck>,
    next_source_check: Option<Instant>,
    export: Option<(Arc<Report>, Receiver<Result<String, String>>)>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl HealthPage {
    pub(in crate::gui_v2) fn take_repair_history_request(&mut self) -> bool {
        std::mem::take(&mut self.repair.open_history)
    }

    pub(in crate::gui_v2) fn show(&mut self, ui: &mut egui::Ui) {
        self.picker.poll();
        if self.repair.poll() {
            self.report = None;
            self.source_check = None;
        }
        self.poll();
        self.poll_export(ui.ctx());
        view::show(ui, self);
        if self.repair.busy()
            || self.job.is_some()
            || self.picker.loading
            || self.source_check.is_some()
            || self.export.is_some()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.report.is_some() {
            ui.ctx().request_repaint_after(Duration::from_secs(5));
        }
    }

    pub(in crate::gui_v2) fn invalidate_inputs(&mut self) {
        self.repair.confirmation.clear();
        self.report = None;
        self.source_check = None;
        self.next_source_check = None;
        self.export = None;
        self.selected_set = None;
        self.offset = 0;
        self.message = Some("Selection changed. Run a new read-only inspection.".into());
    }

    pub(in crate::gui_v2) fn start(&mut self) {
        if self.job.is_some() {
            return;
        }
        self.repair.confirmation.clear();
        self.report = None;
        self.source_check = None;
        self.next_source_check = None;
        self.export = None;
        self.message = None;
        self.progress = None;
        self.selected_set = None;
        self.offset = 0;
        let Some(reference) = self.selected.clone() else {
            self.message = Some("Choose a MAME arcade catalogue first.".into());
            return;
        };
        if !self
            .picker
            .is_usable(DatCatalogueWorkflow::MameHealth, &reference)
        {
            self.message = Some("This catalogue is not available for MAME arcade health. Refresh the catalogue list.".into());
            return;
        }
        let Some(snapshot) = self.picker.snapshot() else {
            self.message = Some("Catalogue inventory is not ready.".into());
            return;
        };
        let Some(path) = self
            .picker
            .summaries()
            .iter()
            .find(|s| s.reference == reference)
            .and_then(|s| s.technical_path.clone())
        else {
            self.message = Some("The selected catalogue has no readable source path.".into());
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        let request = Request {
            id: self.generation,
            reference,
            snapshot,
            catalogue_path: path,
            root: PathBuf::from(self.root.trim()),
            version_output: (!self.version_output.trim().is_empty())
                .then(|| self.version_output.trim().to_string()),
        };
        let (sender, receiver) = mpsc::sync_channel(16);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let work = request.clone();
        self.job = Some(Job {
            request,
            cancel,
            receiver,
        });
        std::thread::spawn(move || {
            let id = work.id;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker::run(work, &worker_cancel, &mut |p| {
                    let _ = sender.try_send(Message::Progress(id, p));
                })
            }))
            .unwrap_or_else(|_| {
                Err(
                    "The inspection worker stopped unexpectedly. No complete report is available."
                        .into(),
                )
            });
            let _ = sender.send(Message::Finished(id, result));
        });
    }

    pub(in crate::gui_v2) fn poll(&mut self) {
        loop {
            let Some(job) = &self.job else {
                break;
            };
            match job.receiver.try_recv() {
                Ok(Message::Progress(id, p)) => {
                    if id == job.request.id {
                        self.progress = Some(p);
                    }
                }
                Ok(Message::Finished(id, result)) => {
                    if id != job.request.id {
                        continue;
                    }
                    let selected_matches = self.selected.as_ref() == Some(&job.request.reference)
                        && PathBuf::from(self.root.trim()) == job.request.root
                        && (!self.version_output.trim().is_empty())
                            .then(|| self.version_output.trim().to_string())
                            == job.request.version_output;
                    let receipt_matches = result.as_ref().is_ok_and(|r| {
                        r.job_id == id
                            && r.catalogue.reference == job.request.reference.token()
                            && r.inspection.inventory.collection_root == job.request.root
                            && r.analysis.collection_root == job.request.root
                            && r.catalogue.source.path == job.request.catalogue_path
                    });
                    let cancelled = job.cancel.load(Ordering::Relaxed);
                    self.job = None;
                    match result {
                        Ok(_) if cancelled => self.message = Some("Inspection cancelled. The completed snapshot was discarded; no complete result is claimed.".into()),
                        Ok(report) if selected_matches && receipt_matches => {
                            self.message = Some(if report.inspection.inventory.inspected_completely { "Inspection finished. Results describe the snapshot taken during this check." } else { "Inspection is incomplete. No set is verified complete; review the reasons below." }.into());
                            self.report = Some(Arc::new(report));
                        }
                        Ok(_) => self.message = Some("The result does not match the current selection. Run a new inspection.".into()),
                        Err(e) => self.message = Some(e),
                    }
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.job = None;
                    self.message = Some(
                        "The inspection worker disconnected; no complete result is available."
                            .into(),
                    );
                    break;
                }
            }
        }
        if !self.repair.busy() {
            self.poll_sources();
        }
    }

    pub(in crate::gui_v2) fn checking_sources(&self) -> bool {
        self.source_check.is_some()
    }

    pub(in crate::gui_v2) fn copy_set(&mut self, name: String) {
        if self.export.is_some() {
            return;
        }
        let Some(report) = self.report.clone() else {
            return;
        };
        let work = report.clone();
        let (sender, receiver) = mpsc::channel();
        self.export = Some((report, receiver));
        std::thread::spawn(move || {
            let result = work.analysis.set_results.iter().find(|s| s.name == name)
                .ok_or_else(|| "The selected set is not in this report.".to_string())
                .and_then(|set| {
                    let value = serde_json::json!({"catalogue":work.catalogue,"inspection_complete":work.inspection.inventory.inspected_completely,"snapshot_unix_seconds":work.inspection.inspected_at_unix_seconds,"collection":work.inspection.inventory.collection_root,"set":set,"declared_context":work.context.get(&set.name),"runnability":"not_assessed"});
                    worker::bounded_json(&value, 32 * 1024 * 1024)
                });
            let _ = sender.send(result);
        });
    }

    fn poll_export(&mut self, context: &egui::Context) {
        if self.checking_sources() {
            return;
        }
        let Some((report, receiver)) = &self.export else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("Set report export stopped unexpectedly.".into())
            }
        };
        if self.report.as_ref().is_some_and(|r| Arc::ptr_eq(r, report)) {
            match result {
                Ok(json) => context.copy_text(json),
                Err(error) => self.message = Some(error),
            }
        }
        self.export = None;
    }

    fn poll_sources(&mut self) {
        if let Some(check) = &self.source_check {
            let result = match check.receiver.try_recv() {
                Ok(unchanged) => Some(unchanged),
                Err(TryRecvError::Disconnected) => Some(false),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(unchanged) = result {
                let matches = self
                    .report
                    .as_ref()
                    .is_some_and(|r| Arc::ptr_eq(r, &check.report));
                self.source_check = None;
                if matches && !unchanged {
                    self.report = None;
                    self.selected_set = None;
                    self.message = Some("Sources changed or cannot be checked since this snapshot. Report withdrawn; inspect again. No files were changed by Health.".into());
                }
                self.next_source_check = Some(Instant::now() + Duration::from_secs(5));
            }
        }
        if self.source_check.is_none()
            && self
                .next_source_check
                .is_none_or(|time| Instant::now() >= time)
            && let Some(report) = self.report.clone()
        {
            let (sender, receiver) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            let work = report.clone();
            self.source_check = Some(SourceCheck {
                report,
                cancel,
                receiver,
            });
            std::thread::spawn(move || {
                let unchanged = work.catalogue.source.still_matches()
                    && work.inspection.sources_unchanged(&worker_cancel);
                let _ = sender.send(unchanged);
            });
        }
    }
}

#[cfg(test)]
pub(in crate::gui_v2) mod tests;
