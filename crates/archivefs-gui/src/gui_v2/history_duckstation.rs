//! GUI-v2 History: DuckStation cheat operations, one row each.
//!
//! A DuckStation cheat batch changes two files and is recorded as two shared
//! journals plus one adapter receipt. This view shows the OPERATION - its
//! cheats, the real apply time, the outcome and the undo status - never the
//! journals. It adds no undo engine: Undo runs in the Cheats workflow that owns
//! it (the adapter's own `undo_duckstation_native`, after a fresh safety check),
//! so this page offers a working deep link to that workflow instead.
//!
//! Everything is rebuilt from the receipts and journals on disk, so it is the
//! same after a restart. The slow part (reading journals) runs off the UI
//! thread and the result is refreshed when the page is opened, on request, and
//! after any change made by the workflow.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use archivefs_core::patch_manager::{
    GroupedHistoryRow, NativeGroupState, SharedHistoryReport, discover_duckstation_native_receipts,
    discover_shared_apply_history, group_shared_history,
};
use eframe::egui;

use super::history_view::{Outcome, UndoStatus, format_time};
use super::library::Library;
use super::routes::{Route, Section};

/// How stale the list may get while the page stays open.
const REFRESH_AFTER: Duration = Duration::from_secs(5);
const MAX_CHEATS_SHOWN: usize = 12;

/// One DuckStation operation as History presents it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Operation {
    pub operation_id: String,
    pub serial: String,
    pub cheats: Vec<String>,
    /// The real apply time recorded in the receipt (None if unrecorded).
    pub applied_at: Option<u64>,
    pub outcome: Outcome,
    pub undo: UndoStatus,
    /// The game the operation was made for, from the journals' own record.
    pub archive: Option<PathBuf>,
    pub files: Vec<String>,
    /// True when only a child journal exists: shown for review, never undone.
    pub review_only: bool,
}

/// Builds the operations from what is on disk. Pure given its inputs.
pub(super) fn build_operations(
    report: &SharedHistoryReport,
    rows: &[GroupedHistoryRow],
) -> Vec<Operation> {
    let archive_of = |index: usize| {
        report
            .journals
            .get(index)
            .and_then(|(_, journal)| journal.context.selected_archive.to_path_buf().ok())
    };
    let mut operations = Vec::new();
    for row in rows {
        match row {
            GroupedHistoryRow::DuckStationNative(group) => {
                let receipt = &group.receipt;
                let expected =
                    usize::from(receipt.cht.is_some()) + usize::from(receipt.game_ini.is_some());
                let complete = group.children.len() >= expected;
                let (outcome, undo) = match (&group.state, complete) {
                    (_, false) => (
                        Outcome::NeedsAttention,
                        UndoStatus::NeedsReview(
                            "Part of this change's record is missing, so EmuWiz will not offer to undo it. Check both DuckStation files by hand.".into(),
                        ),
                    ),
                    (NativeGroupState::UndoAvailable, true) => {
                        (Outcome::Completed, UndoStatus::Available)
                    }
                    (NativeGroupState::AlreadyUndone, true) => {
                        (Outcome::Undone, UndoStatus::AlreadyUndone)
                    }
                    (
                        NativeGroupState::PartiallyUndone {
                            restored,
                            remaining,
                            resumable,
                            reasons,
                        },
                        true,
                    ) => (
                        Outcome::NeedsAttention,
                        UndoStatus::NeedsReview(format!(
                            "An earlier undo stopped partway: {} restored, {} still changed. {}",
                            restored.join(", "),
                            remaining.join(", "),
                            if *resumable {
                                "It can be finished from the Cheats workflow.".to_string()
                            } else {
                                reasons.join(" ")
                            }
                        )),
                    ),
                    (NativeGroupState::Blocked(reasons), true) => (
                        Outcome::Completed,
                        UndoStatus::NeedsReview(reasons.join(" ")),
                    ),
                };
                operations.push(Operation {
                    operation_id: receipt.operation_id.clone(),
                    serial: receipt.serial.clone(),
                    cheats: receipt
                        .cheats_added
                        .iter()
                        .chain(&receipt.cheats_changed)
                        .cloned()
                        .collect(),
                    applied_at: (receipt.timestamp_unix_seconds != 0)
                        .then_some(receipt.timestamp_unix_seconds),
                    outcome,
                    undo,
                    archive: group.children.iter().find_map(|index| archive_of(*index)),
                    files: [receipt.cht.as_ref(), receipt.game_ini.as_ref()]
                        .into_iter()
                        .flatten()
                        .map(|side| {
                            format!("{}/{}", side.destination_root.display(), side.relative_path)
                        })
                        .collect(),
                    review_only: !complete,
                });
            }
            GroupedHistoryRow::OrphanNativeChild {
                journal_index,
                rolled_back,
            } => {
                let Some((_, journal)) = report.journals.get(*journal_index) else {
                    continue;
                };
                // A failed apply that rolled itself back leaves a rolled-back
                // child and no receipt: nothing changed and nothing to undo.
                let rolled_back = *rolled_back;
                operations.push(Operation {
                    operation_id: journal.operation_id.clone(),
                    serial: journal.context.verified_game_identity.clone(),
                    cheats: Vec::new(),
                    applied_at: (journal.timestamp_unix_seconds != 0)
                        .then_some(journal.timestamp_unix_seconds),
                    outcome: if rolled_back {
                        Outcome::Failed
                    } else {
                        Outcome::NeedsAttention
                    },
                    undo: if rolled_back {
                        UndoStatus::Unavailable(
                            "The change stopped and was rolled back; nothing remains to undo."
                                .into(),
                        )
                    } else {
                        UndoStatus::NeedsReview(
                            "Only one half of a two-file change has a record. EmuWiz will not undo half of it; check both DuckStation files by hand."
                                .into(),
                        )
                    },
                    archive: archive_of(*journal_index),
                    files: Vec::new(),
                    review_only: true,
                });
            }
            GroupedHistoryRow::Journal(_) => {}
        }
    }
    operations.sort_by(|a, b| {
        b.applied_at
            .cmp(&a.applied_at)
            .then_with(|| b.operation_id.cmp(&a.operation_id))
    });
    operations
}

/// Reads the history and groups it. Filesystem reads; called off the UI thread.
pub(super) fn load(history_root: &Path, backup_root: &Path) -> Vec<Operation> {
    let report = discover_shared_apply_history(history_root);
    let receipts = discover_duckstation_native_receipts(history_root);
    let rows = group_shared_history(&report, receipts, backup_root);
    build_operations(&report, &rows)
}

#[derive(Default)]
pub(super) struct State {
    operations: Vec<Operation>,
    worker: Option<Receiver<Vec<Operation>>>,
    loaded_at: Option<Instant>,
    unavailable: bool,
}

impl State {
    /// Applies a finished load.
    pub(super) fn poll(&mut self) {
        let Some(receiver) = &self.worker else {
            return;
        };
        match receiver.try_recv() {
            Ok(operations) => {
                self.operations = operations;
                self.worker = None;
                self.loaded_at = Some(Instant::now());
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.worker = None,
        }
    }

    /// Starts a (re)load when none is running and the data is old or missing.
    pub(super) fn ensure_fresh(&mut self, context: &egui::Context, force: bool) {
        if self.worker.is_some() {
            return;
        }
        let stale = self
            .loaded_at
            .is_none_or(|loaded| loaded.elapsed() > REFRESH_AFTER);
        if !force && !stale {
            return;
        }
        let Some((history_root, backup_root)) =
            crate::cheats_mods::duckstation_environment::shared_history_roots()
        else {
            self.unavailable = true;
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.worker = Some(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let _ = sender.send(load(&history_root, &backup_root));
            context.request_repaint();
        });
    }

    pub(super) fn is_loading(&self) -> bool {
        self.worker.is_some()
    }

    pub(super) fn operations(&self) -> &[Operation] {
        &self.operations
    }
}

/// The route to the Cheats workflow that owns this operation's Undo, if the
/// game is in the loaded library.
pub(super) fn owning_workflow_route(operation: &Operation, library: &Library) -> Option<Route> {
    let archive = operation.archive.as_ref()?;
    let game = library
        .games
        .iter()
        .find(|game| &game.archive.absolute_path == archive)?;
    Some(Route::Task {
        section: Section::Mods,
        game: game.archive.id,
    })
}

/// Paints the section. Returns a route when a deep link was clicked, and sets
/// `refresh` when the person asked for a refresh.
pub(super) fn show(
    ui: &mut egui::Ui,
    operations: &[Operation],
    library: &Library,
    loading: bool,
    refresh: &mut bool,
) -> Option<Route> {
    let mut destination = None;
    if operations.is_empty() && !loading {
        return None;
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.heading("DuckStation cheat changes");
        if ui
            .add_enabled(!loading, egui::Button::new("Refresh"))
            .clicked()
        {
            *refresh = true;
        }
        if loading {
            ui.spinner();
        }
    });
    ui.label("Each change below is ONE operation covering both DuckStation files. Undo restores all of it or none of it, and runs in the Cheats workflow for that game.");
    for operation in operations {
        ui.push_id(("duckstation-history", &operation.operation_id), |ui| {
            crate::ui::components::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(if operation.review_only {
                        "DuckStation cheat change (incomplete record)".to_string()
                    } else {
                        format!(
                            "DuckStation cheats · {} ({} cheat{})",
                            operation.serial,
                            operation.cheats.len(),
                            if operation.cheats.len() == 1 { "" } else { "s" }
                        )
                    });
                    ui.label(operation.undo.label());
                });
                ui.label(format!(
                    "{} · {}",
                    operation.outcome.label(),
                    operation
                        .applied_at
                        .map_or_else(|| "time not recorded".to_string(), format_time)
                ));
                for name in operation.cheats.iter().take(MAX_CHEATS_SHOWN) {
                    ui.label(format!("• {name}"));
                }
                if operation.cheats.len() > MAX_CHEATS_SHOWN {
                    ui.label(format!(
                        "… and {} more",
                        operation.cheats.len() - MAX_CHEATS_SHOWN
                    ));
                }
                match &operation.undo {
                    UndoStatus::Unavailable(reason) | UndoStatus::NeedsReview(reason) => {
                        ui.label(reason);
                    }
                    UndoStatus::Available | UndoStatus::AlreadyUndone => {}
                }
                ui.collapsing("Details", |ui| {
                    ui.label(format!("Operation {}", operation.operation_id));
                    for file in &operation.files {
                        ui.monospace(file);
                    }
                });
                if !operation.review_only
                    && matches!(
                        operation.undo,
                        UndoStatus::Available | UndoStatus::NeedsReview(_)
                    )
                {
                    match owning_workflow_route(operation, library) {
                        Some(route) => {
                            let label = if matches!(operation.undo, UndoStatus::Available) {
                                "Open in Cheats to undo"
                            } else {
                                "Open in Cheats to review"
                            };
                            if ui.button(label).clicked() {
                                destination = Some(route);
                            }
                        }
                        None => {
                            ui.label("Undo is in the Cheats workflow for this game, which is not in the loaded library right now.");
                        }
                    }
                }
            });
        });
    }
    destination
}

#[cfg(test)]
mod tests;
