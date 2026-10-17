//! Shared Blue presentation of the SQLite-owned import receipt and Undo.
//! Every database read, validation and transaction runs on a worker.
use archivefs_core::{
    Database, DiscoveryImportOperation, DiscoveryImportUndoPreview, DiscoveryImportUndoResult,
};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
// Each valid receipt may approach16MiB; one batch always fits the32MiB
// History read budget, so large valid receipts cannot block all navigation.
const PAGE: usize = 1;
enum Value {
    Loaded(PathBuf, Vec<DiscoveryImportOperation>),
    Preview(DiscoveryImportUndoPreview),
    Undone(DiscoveryImportUndoResult),
}
struct Job {
    target: Option<PathBuf>,
    receiver: Receiver<Result<Value, String>>,
}
#[derive(Default)]
pub(super) struct State {
    requested: Option<PathBuf>,
    database: Option<PathBuf>,
    operations: Vec<DiscoveryImportOperation>,
    job: Option<Job>,
    preview: Option<DiscoveryImportUndoPreview>,
    confirm: bool,
    message: String,
    loaded: Option<Instant>,
    page: usize,
    details_page: usize,
    changed: bool,
}
impl State {
    pub(super) fn poll_background(&mut self) -> bool {
        self.poll();
        std::mem::take(&mut self.changed)
    }
    pub(super) fn has_records(&self) -> bool {
        !self.operations.is_empty() || self.job.is_some()
    }

    fn start(
        &mut self,
        ctx: egui::Context,
        f: impl FnOnce() -> Result<Value, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job {
            target: self.requested.clone(),
            receiver: rx,
        });
        std::thread::spawn(move || {
            let r = f();
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }
    fn load(&mut self, ctx: egui::Context) {
        let requested = self.requested.clone();
        let offset = self.page * PAGE;
        self.start(ctx, move || {
            let path = match requested {
                Some(p) => p,
                None => archivefs_core::default_database_path().map_err(|e| e.to_string())?,
            };
            let db = Database::open_read_only(&path).map_err(|e| e.to_string())?;
            let ops = db.discovery_import_history(offset, PAGE)?;
            Ok(Value::Loaded(path, ops))
        });
    }
    fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let result=match job.receiver.try_recv(){Ok(r)=>r,Err(TryRecvError::Empty)=>return,Err(TryRecvError::Disconnected)=>Err("The import recovery worker stopped. Its commit outcome is unknown here. Reload durable History before retrying; never assume nothing changed.".into())};
        let same = job.target == self.requested;
        self.job = None;
        if !same {
            self.message="The selected database changed while a worker was active. Reload the originally confirmed database's History to inspect its durable outcome.".into();
            self.loaded = None;
            self.preview = None;
            self.confirm = false;
            return;
        }
        match result {
            Ok(Value::Loaded(path, ops)) => {
                self.database = Some(path);
                self.operations = ops;
                self.loaded = Some(Instant::now());
            }
            Ok(Value::Preview(p)) => {
                self.preview = Some(p);
                self.confirm = false;
                self.details_page = 0;
            }
            Ok(Value::Undone(r)) => {
                self.message = if r.already_undone {
                    format!(
                        "Import {} was already undone; no records changed.",
                        r.operation_id
                    )
                } else {
                    format!(
                        "Undo committed for {}: {} created archive records and {} recorded platform assignments removed. All media files and preexisting records preserved.",
                        r.operation_id, r.removed_records, r.removed_assignments
                    )
                };
                self.changed = !r.already_undone;
                self.preview = None;
                self.confirm = false;
                self.loaded = None;
            }
            Err(e) => {
                self.message = e;
                self.preview = None;
                self.confirm = false;
                self.loaded = Some(Instant::now());
            }
        }
    }
    /// Returns true once after a committed removal, for a read-only UI reload.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        database: Option<&Path>,
        busy: bool,
        game: Option<&Path>,
    ) -> bool {
        egui::ScrollArea::vertical()
            .id_salt("discovery_import_history_pane")
            .max_height(440.0)
            .show(ui, |ui| self.show_body(ui, database, busy, game))
            .inner
    }
    fn show_body(
        &mut self,
        ui: &mut egui::Ui,
        database: Option<&Path>,
        busy: bool,
        game: Option<&Path>,
    ) -> bool {
        let requested = database.map(Path::to_path_buf);
        if self.requested != requested {
            self.requested = requested;
            self.database = None;
            self.operations.clear();
            self.preview = None;
            self.confirm = false;
            self.loaded = None;
            self.page = 0;
        }
        self.poll();
        if self.job.is_none()
            && !busy
            && self.preview.is_none()
            && self
                .loaded
                .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            self.load(ui.ctx().clone());
        }
        ui.separator();
        ui.heading("Discovery imports · History & Undo");
        ui.small("Committed native bounded imports only. Preview is read-only. Confirm removes only this batch's unchanged created database records and recorded platform assignments, all or nothing. No ROMs, saves, artwork, paths or emulator settings are modified. Older imports without durable receipts cannot be selectively undone.");
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
        if let Some(path) = &self.database {
            ui.small(format!("Database: {}", path.display()));
        }
        let enabled = !busy && self.job.is_none();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(enabled, egui::Button::new("Reload import History"))
                .clicked()
            {
                self.preview = None;
                self.confirm = false;
                self.loaded = None;
            }
            if ui
                .add_enabled(
                    enabled && self.page > 0,
                    egui::Button::new("Newer import batches"),
                )
                .clicked()
            {
                self.page -= 1;
                self.preview = None;
                self.loaded = None;
            }
            if ui
                .add_enabled(
                    enabled && self.operations.len() == PAGE,
                    egui::Button::new("Older import batches"),
                )
                .clicked()
            {
                self.page += 1;
                self.preview = None;
                self.loaded = None;
            }
            ui.label(format!("Import history page {}", self.page + 1));
        });
        if self.job.is_some() {
            ui.spinner();
            ui.label("Reading or revalidating import recovery evidence…");
        }
        if self.operations.is_empty() && self.loaded.is_some() {
            ui.label("No committed import receipts on this page. No old imports have been assigned invented Undo evidence.");
        }
        let mut request = None;
        for op in &self.operations {
            if game.is_some_and(|p| !op.items.iter().any(|i| i.path() == p)) {
                continue;
            }
            let projection =
                archivefs_core::operation::discovery_import_operation(op, self.preview.as_ref());
            ui.push_id(("discovery-import", &op.operation_id), |ui| {
                ui.group(|ui| {
                    ui.strong(format!(
                        "Discovery import {} · {}",
                        op.operation_id,
                        projection.state.label()
                    ));
                    ui.label(format!(
                        "{} inserted | {} preexisting comparisons retained/skipped | {} refused during Apply | {} context exclusions | committed {}",
                        op.items.len(),
                        op.skipped,
                        op.refused,
                        op.excluded,
                        op.created_at
                    ));
                    if let Some(at) = &op.undone_at {
                        ui.label(format!("Undo committed {at}"));
                    }
                    ui.label(&projection.recovery.explanation);
                    if ui
                        .add_enabled(
                            enabled && !op.undone && !op.items.is_empty(),
                            egui::Button::new("Preview Undo"),
                        )
                        .clicked()
                    {
                        request = Some(op.operation_id.clone());
                    }
                    ui.collapsing("Import provenance and exclusion evidence", |ui| {
                        ui.label(format!("Root: {}", op.root.display()));
                        ui.small(format!("Allowlist SHA-256: {}", op.approval_sha256));
                        ui.small(format!("Exclusions SHA-256: {}", op.exclusions_sha256));
                        for basis in op.catalogue_basis.iter().take(20) {
                            ui.label(basis);
                        }
                        for e in op.exclusions.iter().take(50) {
                            ui.label(format!(
                                "{}: {}",
                                PathBuf::from(std::ffi::OsString::from_vec(e.path_bytes.clone()))
                                    .display(),
                                e.reason
                            ));
                        }
                        if op.exclusions.len() > 50 {
                            ui.label(format!(
                                "First 50 of {} retained exclusion reasons shown.",
                                op.exclusions.len()
                            ));
                        }
                    });
                });
            });
        }
        if let Some(id) = request
            && let Some(path) = self.database.clone()
        {
            self.preview = None;
            self.confirm = false;
            self.start(ui.ctx().clone(), move || {
                Database::open_read_only(path)
                    .map_err(|e| e.to_string())?
                    .preview_discovery_import_undo(&id)
                    .map(Value::Preview)
            });
        }
        if let Some(p) = &self.preview {
            ui.group(|ui|{
                ui.strong(format!("Undo preview: {}",p.operation.operation_id));
                ui.label(format!("Reviewed batch scope: {} archive rows + {} recorded platform assignments. Other dependent records deleted: 0. Preexisting library records deleted: 0.",p.affected_records(),p.affected_dependencies()));
                if p.expired(){ui.label("This Undo preview expired. Preview Undo again before confirming.");}
                if !p.blockers.is_empty(){ui.strong("Entire batch refused. No records will be removed.");for reason in p.blockers.iter().take(50){ui.label(reason);}if p.blockers.len()>50{ui.label(format!("{} more recorded refusals; none of the batch can be removed.",p.blockers.len()-50));}}
                ui.horizontal_wrapped(|ui|{if ui.add_enabled(self.details_page>0,egui::Button::new("Previous affected records")).clicked(){self.details_page-=1;}if ui.add_enabled((self.details_page+1)*50<p.operation.items.len(),egui::Button::new("Next affected records")).clicked(){self.details_page+=1;}});
                for item in p.operation.items.iter().skip(self.details_page*50).take(50){ui.label(format!("archives.id={} · {} · platform_assignments.id={:?}",item.archive_id,item.path().display(),item.platform_assignment_ids));}
                ui.small("Confirm repeats the full database validation under the writer lock. New edits, DAT evidence, relationships or unknown schema refuse the entire batch. Source media can be offline: Undo never opens it.");
                if ui.add_enabled(enabled&&p.can_undo(),egui::Button::new("Review Undo confirmation…")).clicked(){self.confirm=true;}
            });
        }
        if self.confirm
            && let Some(p) = self.preview.clone()
        {
            let mut apply = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("confirm_discovery_import_undo")).show(ui.ctx(),|ui|{
                ui.heading("Confirm Undo of this exact committed import");ui.label(format!("Import {}: remove {} created archive rows and {} recorded platform assignments.",p.operation.operation_id,p.affected_records(),p.affected_dependencies()));
                ui.label("No files or preexisting records will be removed. If any part is no longer safe, nothing is removed. An interruption is recovered through SQLite and the existing database recovery workflow; reload History to determine the committed outcome.");
                ui.horizontal_wrapped(|ui|{cancel=ui.button("Cancel Undo").clicked();apply=ui.add_enabled(enabled&&p.can_undo(),egui::Button::new("Confirm Undo")).clicked();});
            });
            if cancel {
                self.confirm = false;
            }
            if apply {
                self.confirm = false;
                self.start(ui.ctx().clone(), move || {
                    Database::undo_discovery_import(&p).map(Value::Undone)
                });
            }
        }
        std::mem::take(&mut self.changed)
    }
}
use std::os::unix::ffi::OsStringExt;

#[cfg(test)]
mod tests;
