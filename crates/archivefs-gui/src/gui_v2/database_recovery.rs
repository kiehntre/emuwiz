//! Blue's History section for library-database restore receipts: the state
//! of every restore and rollback, reconciliation of interrupted ones, and
//! rollback of completed ones.
//!
//! Core owns every check (`archivefs_core::database::restore`): the record
//! list is the same `OperationRegistry` projection the full History window
//! shows (metadata-only preview, never opens the database), and every action
//! calls the same executor the full window calls, with the same confirmation
//! phrase and the same refusals. Actions run on a worker thread so the
//! overlay stays responsive; nothing here hashes or opens a database on the
//! UI thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use archivefs_core::operation::{
    ActionAvailability, OperationKind, OperationRegistry, OperationState,
};
use archivefs_core::{DatabaseRestoreReceipt, InterruptedRestoreAssessment};
use eframe::egui;

const CONFIRMATION: &str = "RESTORE DATABASE";
const RELOAD_EVERY: Duration = Duration::from_secs(5);
const MAX_ROWS: usize = 50;

/// One restore or rollback receipt as History presents it.
#[derive(Clone, Debug)]
pub(super) struct Row {
    pub receipt_path: PathBuf,
    pub operation_id: String,
    pub state: OperationState,
    pub completed_at_unix: Option<u64>,
    /// Core's summary: the receipt message, or for an interrupted receipt
    /// the on-disk assessment and its next step.
    pub summary: String,
    pub explanation: String,
    pub classification: String,
    pub emergency_backup: Option<String>,
    pub rollback_offered: bool,
    pub reconcile_offered: bool,
}

impl Row {
    pub(super) fn interrupted(&self) -> bool {
        self.state == OperationState::Interrupted
    }

    /// Interrupted, but core could not classify it well enough to finalise
    /// automatically: the person must review the database directory.
    pub(super) fn needs_manual_review(&self) -> bool {
        self.interrupted() && !self.reconcile_offered
    }

    pub(super) fn headline(&self) -> String {
        match self.state {
            OperationState::Completed => "Database restore completed".into(),
            OperationState::RolledBack => "Database restore rolled back".into(),
            OperationState::Failed => "Database restore failed".into(),
            OperationState::Interrupted if self.reconcile_offered => {
                "Database restore interrupted: reconciliation required".into()
            }
            OperationState::Interrupted => {
                "Database restore interrupted: manual review required".into()
            }
            other => format!("Database restore {}", other.label().to_lowercase()),
        }
    }
}

enum Kind {
    Load,
    RollbackReview,
    Rollback,
    Reconcile,
}

enum Value {
    Loaded(Vec<Row>),
    RollbackReviewed(DatabaseRestoreReceipt),
    RolledBack(DatabaseRestoreReceipt),
    Reconciled(InterruptedRestoreAssessment),
}

struct Job {
    kind: Kind,
    receiver: Receiver<Result<Value, String>>,
}

#[derive(Default)]
pub(super) struct State {
    database: Option<PathBuf>,
    rows: Vec<Row>,
    job: Option<Job>,
    /// A completed restore whose rollback inputs passed core's read-only
    /// review; the rollback button acts on exactly this receipt.
    rollback_review: Option<DatabaseRestoreReceipt>,
    confirmation: String,
    message: String,
    loaded: Option<Instant>,
    /// True once after the live database file was replaced (a rollback), so
    /// the caller reloads the library it has in memory.
    changed: bool,
    /// Tests point the section at a disposable database; production uses
    /// the configured library database.
    #[cfg(test)]
    pub(super) pinned_database: Option<PathBuf>,
}

fn rows_from_registry(database: &Path) -> Vec<Row> {
    let mut registry = OperationRegistry::default();
    registry.append_database_restore_history(database);
    let mut rows: Vec<Row> = registry
        .records
        .into_iter()
        .filter(|record| record.kind == OperationKind::DatabaseRecovery)
        .filter_map(|record| {
            let receipt_path = PathBuf::from(record.output.journal_reference.as_ref()?);
            Some(Row {
                receipt_path,
                operation_id: record.operation_id,
                state: record.state,
                completed_at_unix: record.completed_at_unix,
                summary: record.output.summary,
                explanation: record.recovery.explanation,
                classification: record.recovery.classification.label().to_string(),
                emergency_backup: record.output.outputs.first().map(|o| o.path.clone()),
                rollback_offered: record.recovery.actions.rollback == ActionAvailability::Available,
                reconcile_offered: record.state == OperationState::Interrupted
                    && record.recovery.actions.review == ActionAvailability::Available,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.completed_at_unix
            .cmp(&a.completed_at_unix)
            .then_with(|| b.operation_id.cmp(&a.operation_id))
    });
    rows.truncate(MAX_ROWS);
    rows
}

impl State {
    pub(super) fn has_records(&self) -> bool {
        !self.rows.is_empty() || self.job.is_some()
    }

    /// Any receipt that never reached a terminal state.
    pub(super) fn needs_attention(&self) -> bool {
        self.rows.iter().any(Row::interrupted)
    }

    #[cfg(test)]
    pub(super) fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[cfg(test)]
    pub(super) fn message(&self) -> &str {
        &self.message
    }

    #[cfg(test)]
    pub(super) fn rollback_review(&self) -> Option<&DatabaseRestoreReceipt> {
        self.rollback_review.as_ref()
    }

    fn database_path(&self) -> Option<PathBuf> {
        #[cfg(test)]
        if let Some(pinned) = &self.pinned_database {
            return Some(pinned.clone());
        }
        archivefs_core::default_database_path().ok()
    }

    /// A worker is reading receipts or running an action.
    pub(super) fn working(&self) -> bool {
        self.job.is_some()
    }

    /// A section bound to a disposable database, for tests.
    #[cfg(test)]
    pub(super) fn pinned(database: PathBuf) -> Self {
        Self {
            pinned_database: Some(database),
            ..Self::default()
        }
    }

    /// Whether an action may start now: nothing of ours is running and the
    /// window has no catalogue load or scan in flight. Core refuses again on
    /// its own (locks, freshness, identity), so this gate is only the polite
    /// first refusal.
    pub(super) fn actions_enabled(&self, busy: bool) -> bool {
        !busy && !self.working()
    }

    pub(super) fn rollback_enabled(&self, busy: bool) -> bool {
        self.actions_enabled(busy)
            && self.rollback_review.is_some()
            && self.confirmation == CONFIRMATION
    }

    fn start(
        &mut self,
        ctx: egui::Context,
        kind: Kind,
        f: impl FnOnce() -> Result<Value, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job { kind, receiver: rx });
        std::thread::spawn(move || {
            let result = f();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn load(&mut self, ctx: egui::Context) {
        let Some(database) = self.database_path() else {
            self.message = "EmuWiz could not locate the library database, so restore receipts cannot be listed.".into();
            self.loaded = Some(Instant::now());
            return;
        };
        self.database = Some(database.clone());
        self.start(ctx, Kind::Load, move || {
            Ok(Value::Loaded(rows_from_registry(&database)))
        });
    }

    /// Core's read-only review of a completed restore's rollback inputs.
    pub(super) fn request_rollback_review(
        &mut self,
        ctx: egui::Context,
        receipt_path: PathBuf,
        busy: bool,
    ) {
        if !self.actions_enabled(busy) {
            self.message = refused_busy("Rollback review");
            return;
        }
        self.rollback_review = None;
        self.confirmation.clear();
        self.start(ctx, Kind::RollbackReview, move || {
            archivefs_core::review_database_rollback(&receipt_path)
                .map(Value::RollbackReviewed)
                .map_err(|error| error.to_string())
        });
    }

    pub(super) fn cancel_rollback_review(&mut self) {
        self.rollback_review = None;
        self.confirmation.clear();
    }

    /// Core's rollback executor, with the typed confirmation phrase.
    pub(super) fn request_rollback(&mut self, ctx: egui::Context, busy: bool) {
        if !self.actions_enabled(busy) {
            self.message = refused_busy("Rollback");
            return;
        }
        if !self.rollback_enabled(busy) {
            self.message =
                "Rollback needs a passed review and the exact phrase RESTORE DATABASE.".into();
            return;
        }
        let Some(receipt) = self.rollback_review.clone() else {
            self.message = "Review the rollback first.".into();
            return;
        };
        let confirmation = self.confirmation.clone();
        self.start(ctx, Kind::Rollback, move || {
            archivefs_core::rollback_database_restore(&receipt, &confirmation)
                .map(Value::RolledBack)
                .map_err(|error| error.to_string())
        });
    }

    /// Core's reconciliation of an interrupted receipt: it rewrites only the
    /// receipt, never a database file, and refuses when it cannot be sure.
    pub(super) fn request_reconcile(
        &mut self,
        ctx: egui::Context,
        receipt_path: PathBuf,
        busy: bool,
    ) {
        if !self.actions_enabled(busy) {
            self.message = refused_busy("Reconciliation");
            return;
        }
        self.start(ctx, Kind::Reconcile, move || {
            archivefs_core::finalise_interrupted_restore(&receipt_path, CONFIRMATION)
                .map(Value::Reconciled)
                .map_err(|error| error.to_string())
        });
    }

    #[cfg(test)]
    pub(super) fn set_confirmation(&mut self, text: &str) {
        self.confirmation = text.to_string();
    }

    pub(super) fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(
                "The database recovery worker stopped before reporting. Its outcome is unknown here; reload History to see the durable receipt before retrying."
                    .into(),
            ),
        };
        let kind = match self.job.take() {
            Some(job) => job.kind,
            None => Kind::Load,
        };
        match result {
            Ok(Value::Loaded(rows)) => {
                self.rows = rows;
                self.loaded = Some(Instant::now());
            }
            Ok(Value::RollbackReviewed(receipt)) => {
                self.rollback_review = Some(receipt);
                self.confirmation.clear();
                self.message.clear();
            }
            Ok(Value::RolledBack(receipt)) => {
                self.message = receipt.message.clone();
                self.rollback_review = None;
                self.confirmation.clear();
                self.changed = true;
                self.loaded = None;
            }
            Ok(Value::Reconciled(assessment)) => {
                self.message = format!("Reconciled: {}", assessment.receipt.message);
                // Reconciliation changes no database file, but a restore or
                // rollback found applied means the live file differs from
                // what this window may have loaded before the interruption;
                // a read-only reload is cheap and never wrong.
                self.changed = matches!(
                    assessment.receipt.state,
                    archivefs_core::DatabaseRestoreState::Completed
                        | archivefs_core::DatabaseRestoreState::RolledBack
                );
                self.loaded = None;
            }
            Err(error) => {
                self.message = match kind {
                    Kind::Load => format!("Restore receipts could not be listed: {error}"),
                    Kind::RollbackReview => format!("Rollback review refused: {error}"),
                    Kind::Rollback => format!("Rollback refused: {error}"),
                    Kind::Reconcile => format!("Reconciliation refused: {error}"),
                };
                if matches!(kind, Kind::Rollback) {
                    // Core finalised its receipt; the review is stale.
                    self.rollback_review = None;
                    self.confirmation.clear();
                }
                // A failed listing keeps its cadence (no respawn per frame);
                // a failed action re-reads the receipts straight away.
                self.loaded = if matches!(kind, Kind::Load) {
                    Some(Instant::now())
                } else {
                    None
                };
            }
        }
    }

    /// Returns true once after the live database file was replaced, for a
    /// read-only reload of the library in memory.
    pub(super) fn show(&mut self, ui: &mut egui::Ui, busy: bool) -> bool {
        self.poll();
        if self.job.is_none()
            && self.rollback_review.is_none()
            && self.loaded.is_none_or(|at| at.elapsed() > RELOAD_EVERY)
        {
            self.load(ui.ctx().clone());
        }
        if self.rows.is_empty() && self.job.is_none() && self.message.is_empty() {
            return std::mem::take(&mut self.changed);
        }
        ui.separator();
        ui.heading("Database restore & recovery");
        ui.small("Receipts of library-database restores. Rollback puts the pre-restore database back from its verified emergency backup; reconciliation records what an interrupted restore left on disk. Both refuse while anything is writing the catalogue, and both need the same confirmation as a restore.");
        if self.needs_attention() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "A database restore did not finish. Reconcile it below before restoring again.",
            );
        }
        if busy {
            ui.label("Database actions wait until the current load, scan or background job has finished.");
        }
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(match job.kind {
                    Kind::Load => "Reading restore receipts…",
                    Kind::RollbackReview => "Checking the emergency backup and the live database…",
                    Kind::Rollback => "Rolling back the restore…",
                    Kind::Reconcile => "Reconciling the interrupted restore…",
                });
            });
        }
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
        let enabled = self.actions_enabled(busy);
        let ctx = ui.ctx().clone();
        let mut review_request: Option<PathBuf> = None;
        let mut reconcile_request: Option<PathBuf> = None;
        let mut rollback_request = false;
        let mut cancel = false;
        if let Some(receipt) = &self.rollback_review {
            ui.group(|ui| {
                ui.strong("Review rollback");
                ui.label(format!("Restore: {}", receipt.operation_id));
                if let Some(path) = &receipt.emergency_backup_path {
                    ui.small(format!("Emergency backup: {}", path.display()));
                }
                ui.label("The pre-restore database will be put back from this verified backup. Changes made since the restore would be lost; the rollback refuses if the restored database has changed.");
                ui.add(egui::TextEdit::singleline(&mut self.confirmation).hint_text("Type RESTORE DATABASE"));
                ui.horizontal(|ui| {
                                                            if ui
                        .add_enabled(
                            enabled && self.confirmation == CONFIRMATION,
                            egui::Button::new("Roll back restore"),
                        )
                        .clicked()
                    {
                        rollback_request = true;
                    }
                    if ui.button("Cancel review").clicked() {
                        cancel = true;
                    }
                });
            });
        }
        if ui
            .add_enabled(enabled, egui::Button::new("Reload restore receipts"))
            .clicked()
        {
            self.loaded = None;
        }
        for row in &self.rows {
            ui.push_id(&row.operation_id, |ui| {
                ui.group(|ui| {
                    ui.strong(row.headline());
                    ui.label(&row.summary);
                    ui.small(format!("{} · {}", row.classification, row.explanation));
                    if let Some(backup) = &row.emergency_backup {
                        ui.small(format!("Emergency backup: {backup}"));
                    }
                    ui.horizontal_wrapped(|ui| {
                        if row.rollback_offered
                            && ui
                                .add_enabled(enabled, egui::Button::new("Review rollback"))
                                .clicked()
                        {
                            review_request = Some(row.receipt_path.clone());
                        }
                        if row.reconcile_offered
                            && ui
                                .add_enabled(enabled, egui::Button::new("Reconcile interrupted restore"))
                                .clicked()
                        {
                            reconcile_request = Some(row.receipt_path.clone());
                        }
                    });
                    if row.needs_manual_review() {
                        ui.label("Automatic reconciliation is not safe for this receipt; review the database directory by hand before opening the full History window.");
                    }
                    ui.collapsing("Receipt", |ui| {
                        ui.monospace(&row.operation_id);
                        ui.monospace(row.receipt_path.display().to_string());
                    });
                });
            });
        }
        if cancel {
            self.cancel_rollback_review();
        }
        if let Some(path) = review_request {
            self.request_rollback_review(ctx.clone(), path, busy);
        }
        if let Some(path) = reconcile_request {
            self.request_reconcile(ctx.clone(), path, busy);
        }
        if rollback_request {
            self.request_rollback(ctx, busy);
        }
        std::mem::take(&mut self.changed)
    }
}

fn refused_busy(operation: &str) -> String {
    format!(
        "{operation} refused: the catalogue is loading, scanning or already running a database recovery action. Try again when it is idle. Nothing was changed."
    )
}
