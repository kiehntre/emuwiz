//! Saves page panels: memory card restore records (recovery, review, Undo)
//! and retained restore files (preview, then confirm, before retiring).

use super::{
    Paths, WorkDirs,
    recovery::{self, RecordKind, RecordsReport, ReviewPreview},
    storage::{self, FileKind, RetireResult, Status, StorageReport, StoredFile},
};
use crate::ui::theme;
use eframe::egui;
use std::path::PathBuf;

#[derive(Default)]
pub(in super::super) struct PanelState {
    pub(in super::super) records: Option<RecordsReport>,
    pub(in super::super) records_requested: bool,
    pub(in super::super) messages: Vec<String>,
    pub(in super::super) review: Option<ReviewPreview>,
    pub(in super::super) storage: Option<StorageReport>,
    /// Files shown in the retire confirmation, exactly as previewed.
    pub(in super::super) retire_preview: Option<Vec<StoredFile>>,
    pub(in super::super) storage_message: Option<String>,
}

pub(in super::super) enum PanelJob {
    Records {
        journal_dir: PathBuf,
    },
    Recover {
        journal_dir: PathBuf,
    },
    Review {
        journal: PathBuf,
    },
    MarkReviewed {
        preview: Box<ReviewPreview>,
        journal_dir: PathBuf,
    },
    Undo {
        journal: PathBuf,
        journal_dir: PathBuf,
    },
    Storage {
        dirs: WorkDirs,
        journal_dir: PathBuf,
        keep: Vec<PathBuf>,
    },
    Retire {
        files: Vec<StoredFile>,
        dirs: WorkDirs,
        journal_dir: PathBuf,
        keep: Vec<PathBuf>,
    },
}

pub(in super::super) enum PanelOutcome {
    Records(RecordsReport, Vec<String>),
    Review(Result<ReviewPreview, String>),
    Storage(StorageReport),
    Retired(RetireResult, StorageReport),
}

/// Backend worker. Uses the production PCSX2 closure observer.
pub(in super::super) fn run(job: PanelJob) -> PanelOutcome {
    let observer = archivefs_core::save_snapshots::history::default_closure_observer();
    let now = super::now();
    match job {
        PanelJob::Records { journal_dir } => {
            PanelOutcome::Records(recovery::load_records(&journal_dir), Vec::new())
        }
        PanelJob::Recover { journal_dir } => {
            let messages = recovery::recover(&journal_dir, &observer, now)
                .unwrap_or_else(|refusal| vec![refusal]);
            PanelOutcome::Records(recovery::load_records(&journal_dir), messages)
        }
        PanelJob::Review { journal } => PanelOutcome::Review(recovery::review_preview(&journal)),
        PanelJob::MarkReviewed {
            preview,
            journal_dir,
        } => {
            let message = match recovery::mark_reviewed(&preview, now) {
                Ok(()) => "Marked as reviewed. The record and its backup are kept.".to_string(),
                Err(error) => error,
            };
            PanelOutcome::Records(recovery::load_records(&journal_dir), vec![message])
        }
        PanelJob::Undo {
            journal,
            journal_dir,
        } => {
            let message = match super::undo_restore(&journal, &observer, now) {
                Ok(()) => {
                    "Undo finished: the memory card is back to how it was before that restore."
                        .to_string()
                }
                Err(error) => format!("Undo was not done. {error}"),
            };
            PanelOutcome::Records(recovery::load_records(&journal_dir), vec![message])
        }
        PanelJob::Storage {
            dirs,
            journal_dir,
            keep,
        } => PanelOutcome::Storage(storage::assess(&dirs, &journal_dir, &keep)),
        PanelJob::Retire {
            files,
            dirs,
            journal_dir,
            keep,
        } => {
            let result = storage::retire(&files, &dirs, &journal_dir, &keep);
            PanelOutcome::Retired(result, storage::assess(&dirs, &journal_dir, &keep))
        }
    }
}

pub(in super::super) fn receive(state: &mut PanelState, outcome: PanelOutcome) -> String {
    match outcome {
        PanelOutcome::Records(report, messages) => {
            state.records = Some(report);
            state.review = None;
            let summary = messages.first().cloned();
            state.messages = messages;
            summary.unwrap_or_else(|| "Restore records checked. Nothing was changed.".into())
        }
        PanelOutcome::Review(Ok(preview)) => {
            state.review = Some(preview);
            "Review ready. Nothing was changed.".into()
        }
        PanelOutcome::Review(Err(error)) => {
            state.review = None;
            state.messages = vec![error.clone()];
            error
        }
        PanelOutcome::Storage(report) => {
            state.storage = Some(report);
            state.retire_preview = None;
            "Restore storage checked. Nothing was changed.".into()
        }
        PanelOutcome::Retired(result, report) => {
            let mut message = format!(
                "Removed {} file(s), {} freed.",
                result.removed.len(),
                bytes(result.freed_bytes)
            );
            if !result.kept.is_empty() {
                message.push_str(&format!(
                    " {} file(s) were kept because they changed or became needed since the preview.",
                    result.kept.len()
                ));
            }
            state.storage = Some(report);
            state.retire_preview = None;
            state.storage_message = Some(message.clone());
            message
        }
    }
}

fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 {
        format!("{:.1} MB", value as f64 / (1024.0 * 1024.0))
    } else if value >= 1024 {
        format!("{:.1} KB", value as f64 / 1024.0)
    } else {
        format!("{value} bytes")
    }
}

fn kind_label(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Unfinished => "Interrupted",
        RecordKind::Unreadable => "Unreadable",
        RecordKind::NeedsReview => "Needs review",
        RecordKind::UndoAvailable => "Restored · Undo available",
        RecordKind::Finished => "Finished",
    }
}

fn open_preview_files(app: &super::super::App) -> Vec<PathBuf> {
    match &app.saves_states.card_restore.flow {
        super::Flow::Confirm { preview, .. } => vec![preview.plan.source_psu_path.clone()],
        _ => Vec::new(),
    }
}

/// Both panels, shown once on the all-saves view.
pub(in super::super) fn show(ui: &mut egui::Ui, app: &mut super::super::App) {
    let Some(paths) = app.saves_states.card_restore.paths() else {
        return;
    };
    if !app.saves_states.card_restore.panel.records_requested
        && app.saves_states.card_restore.job.is_none()
    {
        app.saves_states.card_restore.panel.records_requested = true;
        app.start_card_restore(
            "Checking memory card restore records",
            super::Job::Panel(PanelJob::Records {
                journal_dir: paths.journal_dir.clone(),
            }),
        );
    }
    let mut job: Option<(&'static str, PanelJob)> = None;
    records_panel(ui, app, &paths, &mut job);
    storage_panel(ui, app, &paths, &mut job);
    if let Some((label, job)) = job {
        app.start_card_restore(label, super::Job::Panel(job));
    }
}

fn records_panel(
    ui: &mut egui::Ui,
    app: &mut super::super::App,
    paths: &Paths,
    job: &mut Option<(&'static str, PanelJob)>,
) {
    let busy = app.saves_states.card_restore.job.is_some();
    let state = &app.saves_states.card_restore.panel;
    let attention = state.records.as_ref().map_or(0, |report| {
        report
            .rows
            .iter()
            .filter(|row| {
                matches!(row.kind, RecordKind::Unfinished | RecordKind::Unreadable)
                    || (row.kind == RecordKind::NeedsReview && row.review.is_none())
            })
            .count()
    });
    let title = if attention > 0 {
        format!("Memory card restore records · {attention} need attention")
    } else {
        "Memory card restore records".to_string()
    };
    let mut review_close = false;
    egui::CollapsingHeader::new(title)
        .id_salt("card_restore_records")
        .default_open(attention > 0)
        .show(ui, |ui| {
            ui.weak("Every PS2 memory card restore leaves a record. Records are kept as evidence; EmuWiz never deletes them.");
            for message in &state.messages {
                ui.label(message);
            }
            let Some(report) = &state.records else {
                ui.weak(if busy { "Checking records…" } else { "Records have not been checked yet." });
                return;
            };
            if !report.complete() {
                ui.colored_label(
                    theme::warning(ui),
                    "Not every restore record could be read, so restores stay paused until this is fixed. Nothing was removed.",
                );
                for problem in &report.problems {
                    ui.weak(problem);
                }
            }
            if report.rows.is_empty() {
                ui.weak("No restore records yet.");
            }
            for row in &report.rows {
                ui.push_id(&row.path, |ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(kind_label(row.kind));
                            if let Some(folder) = &row.folder {
                                ui.label(format!("· {folder}"));
                            }
                            if let Some(card) = &row.card_path {
                                ui.weak(format!("· {}", card.display()));
                            } else {
                                ui.weak(format!("· record {}", row.path.display()));
                            }
                        });
                        let explanation = match (row.kind, &row.review) {
                            (RecordKind::Unfinished, _) => Some("A restore or Undo was interrupted. Check interrupted restores works out what happened from the card; it never writes the card. Restores are paused until then."),
                            (RecordKind::Unreadable, _) => Some("This record cannot be read, so EmuWiz cannot tell which card it concerns or whether its restore finished. Restores to every memory card stay paused until the record can be read again; it needs manual investigation. Show evidence lists what to inspect. Nothing is deleted."),
                            (RecordKind::NeedsReview, None) => Some("EmuWiz could not prove what happened, so it stopped and kept everything. Restores to this card are paused until you review it."),
                            (_, Some(_)) => Some("Reviewed. The record is kept as evidence."),
                            _ => None,
                        };
                        if let Some(text) = explanation {
                            ui.label(text);
                        }
                        if let Some(detail) = &row.detail {
                            ui.weak(detail);
                        }
                        ui.horizontal_wrapped(|ui| {
                            let review_label = if row.kind == RecordKind::Unreadable {
                                "Show evidence…"
                            } else {
                                "Review…"
                            };
                            if (row.kind == RecordKind::Unreadable
                                || (row.kind == RecordKind::NeedsReview && row.review.is_none()))
                                && ui.add_enabled(!busy, egui::Button::new(review_label)).clicked()
                            {
                                *job = Some((
                                    "Reviewing a restore record",
                                    PanelJob::Review { journal: row.path.clone() },
                                ));
                            }
                            if row.kind == RecordKind::UndoAvailable
                                && ui
                                    .add_enabled(!busy, egui::Button::new("Undo this restore"))
                                    .on_hover_text("Puts the whole card back exactly as it was before this restore. Refused if the card changed since, or while PCSX2 is open.")
                                    .clicked()
                            {
                                *job = Some((
                                    "Undoing a memory card restore",
                                    PanelJob::Undo {
                                        journal: row.path.clone(),
                                        journal_dir: paths.journal_dir.clone(),
                                    },
                                ));
                            }
                        });
                    });
                });
            }
            if let Some(review) = &state.review {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Review restore record");
                    for fact in &review.facts {
                        ui.label(fact);
                    }
                    match &review.allowed {
                        Ok(text) => {
                            ui.label(text);
                        }
                        Err(text) => {
                            ui.colored_label(theme::warning(ui), text);
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        if review.allowed.is_ok()
                            && ui
                                .add_enabled(!busy, egui::Button::new("Mark as reviewed (keeps the record)"))
                                .clicked()
                        {
                            *job = Some((
                                "Recording a restore record review",
                                PanelJob::MarkReviewed {
                                    preview: Box::new(review.clone()),
                                    journal_dir: paths.journal_dir.clone(),
                                },
                            ));
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Close")).clicked() {
                            review_close = true;
                        }
                    });
                });
            }
            ui.horizontal_wrapped(|ui| {
                if report.rows.iter().any(|row| row.kind == RecordKind::Unfinished)
                    && ui
                        .add_enabled(!busy, egui::Button::new("Check interrupted restores"))
                        .on_hover_text("Needs PCSX2 closed. Reads the cards and updates the records; never writes a card.")
                        .clicked()
                {
                    *job = Some((
                        "Checking interrupted memory card restores",
                        PanelJob::Recover { journal_dir: paths.journal_dir.clone() },
                    ));
                }
                if ui.add_enabled(!busy, egui::Button::new("Refresh records")).clicked() {
                    *job = Some((
                        "Checking memory card restore records",
                        PanelJob::Records { journal_dir: paths.journal_dir.clone() },
                    ));
                }
            });
        });
    if review_close {
        app.saves_states.card_restore.panel.review = None;
    }
}

fn storage_panel(
    ui: &mut egui::Ui,
    app: &mut super::super::App,
    paths: &Paths,
    job: &mut Option<(&'static str, PanelJob)>,
) {
    let busy = app.saves_states.card_restore.job.is_some();
    let keep = open_preview_files(app);
    let state = &app.saves_states.card_restore.panel;
    let mut cancel = false;
    let mut confirm_preview: Option<Vec<StoredFile>> = None;
    egui::CollapsingHeader::new("Restore storage")
        .id_salt("card_restore_storage")
        .show(ui, |ui| {
            ui.weak("Exported save folders and card backups kept by memory card restores. Files a restore record still needs are never removed.");
            if let Some(message) = &state.storage_message {
                ui.label(message);
            }
            let Some(report) = &state.storage else {
                if ui.add_enabled(!busy, egui::Button::new("Check restore storage")).clicked() {
                    *job = Some((
                        "Checking restore storage",
                        PanelJob::Storage { dirs: paths.dirs.clone(), journal_dir: paths.journal_dir.clone(), keep: keep.clone() },
                    ));
                }
                return;
            };
            let retirable: Vec<StoredFile> = report.retirable().cloned().collect();
            let retirable_bytes: u64 = retirable.iter().map(|file| file.size_bytes).sum();
            ui.label(format!(
                "{} file(s), {} in total. {} file(s), {}, are no longer needed.",
                report.files.len(),
                bytes(report.total_bytes()),
                retirable.len(),
                bytes(retirable_bytes)
            ));
            for problem in &report.problems {
                ui.colored_label(theme::warning(ui), format!("{problem} Nothing can be removed until this is resolved."));
            }
            for file in &report.files {
                let kind = match file.kind {
                    FileKind::ExportedFolder => "Exported save folder",
                    FileKind::CardBackup => "Card backup",
                    FileKind::LeftoverCopy => "Leftover snapshot copy",
                    FileKind::Other => "Other file",
                };
                let status = match &file.status {
                    Status::Referenced(why) => format!("Kept: {why}"),
                    Status::Uncertain(why) => format!("Kept: {why}"),
                    Status::Unreferenced => "Not needed by any restore record".to_string(),
                };
                ui.weak(format!(
                    "{kind} · {} · {} · {status}",
                    file.path.file_name().unwrap_or_default().to_string_lossy(),
                    bytes(file.size_bytes)
                ));
            }
            match &state.retire_preview {
                None => {
                    ui.horizontal_wrapped(|ui| {
                        if !retirable.is_empty()
                            && ui.add_enabled(!busy, egui::Button::new("Preview removing unneeded files…")).clicked()
                        {
                            confirm_preview = Some(retirable.clone());
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Refresh storage")).clicked() {
                            *job = Some((
                                "Checking restore storage",
                                PanelJob::Storage { dirs: paths.dirs.clone(), journal_dir: paths.journal_dir.clone(), keep: keep.clone() },
                            ));
                        }
                    });
                }
                Some(files) => {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.heading("Remove unneeded restore files?");
                        ui.label(format!(
                            "These {} file(s), {}, are not named by any restore record and have not changed for at least an hour. Each one is checked again just before removal; anything that changed or became needed is kept.",
                            files.len(),
                            bytes(files.iter().map(|file| file.size_bytes).sum())
                        ));
                        for file in files {
                            ui.label(format!("  • {}", file.path.display()));
                        }
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(!busy, egui::Button::new("Remove these files")).clicked() {
                                *job = Some((
                                    "Removing unneeded restore files",
                                    PanelJob::Retire {
                                        files: files.clone(),
                                        dirs: paths.dirs.clone(),
                                        journal_dir: paths.journal_dir.clone(),
                                        keep: keep.clone(),
                                    },
                                ));
                            }
                            if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                                cancel = true;
                            }
                        });
                    });
                }
            }
        });
    let state = &mut app.saves_states.card_restore.panel;
    if cancel {
        state.retire_preview = None;
    }
    if let Some(files) = confirm_preview {
        state.retire_preview = Some(files);
        state.storage_message = None;
    }
}
