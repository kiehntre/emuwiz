//! The Library View History page.
//!
//! Read-only view of the durable, append-only Library View apply/remove
//! history that [`archivefs_core::library_view_history`] already records to
//! disk on every real apply/remove - see that module's own doc comment for
//! the write side. This page never writes, deletes, undoes, repairs, or
//! re-applies anything: every render here is a straight, honest read of
//! whatever is already on disk.
//!
//! # Not the in-memory recent-activity log
//!
//! This is deliberately a separate destination from History & Logs
//! (`MainView::HistoryLogs`), which shows `OperationHistory` - an
//! in-session, in-memory log of recent actions that is empty again after a
//! restart. Library View history is the opposite: it survives restarts
//! because it is read fresh from `library_view_history_dir` every time this
//! page loads or refreshes, never from anything held in memory across a
//! session boundary. Nothing in this module touches `OperationHistory`.

use std::path::PathBuf;

use archivefs_core::{
    FrontendProfileKind, LibraryViewHistoryEntry, LibraryViewHistoryOperation,
    LibraryViewHistoryRecord, default_library_view_history_dir, list_library_view_history_at,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// Matches the `100` already used by `library_views`' own tests over this
/// same listing API - a generous, bounded window rather than an arbitrary
/// new number.
const HISTORY_LIST_LIMIT: usize = 100;

/// The page's authoritative state.
pub(crate) struct LibraryViewHistoryPageState {
    /// Where history records live. Never a saved/loaded snapshot - every
    /// refresh re-reads this directory from disk, exactly like Repair
    /// History's own journal directory.
    history_dir: PathBuf,
    /// The history directory's entries, newest first, as
    /// `list_library_view_history_at` itself already orders them. Reloaded
    /// wholesale by [`Self::refresh`].
    pub(crate) entries: Vec<LibraryViewHistoryEntry>,
}

impl LibraryViewHistoryPageState {
    /// Loads history from the default Library View history directory (the
    /// same one `apply_library_view`/`remove_library_view_symlinks` already
    /// write through `record_library_view_operation`).
    pub(crate) fn load() -> Self {
        let history_dir = default_library_view_history_dir()
            .unwrap_or_else(|_| PathBuf::from("library-view-history"));
        Self::load_with_history_dir(history_dir)
    }

    /// Loads history from an explicit directory. Exists so tests never
    /// depend on resolving the real user data directory.
    pub(crate) fn load_with_history_dir(history_dir: PathBuf) -> Self {
        let mut state = Self {
            history_dir,
            entries: Vec::new(),
        };
        state.refresh();
        state
    }

    /// Re-reads every record from disk. A missing directory (nothing
    /// recorded yet) yields an empty list, not an error - see
    /// `list_library_view_history_at`'s own doc comment.
    pub(crate) fn refresh(&mut self) {
        self.entries = list_library_view_history_at(&self.history_dir, HISTORY_LIST_LIMIT);
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn operation_label(operation: LibraryViewHistoryOperation) -> &'static str {
    match operation {
        LibraryViewHistoryOperation::Apply => "Apply",
        LibraryViewHistoryOperation::Remove => "Remove",
    }
}

fn operation_tone(operation: LibraryViewHistoryOperation) -> widgets::StatusTone {
    match operation {
        LibraryViewHistoryOperation::Apply => widgets::StatusTone::Info,
        LibraryViewHistoryOperation::Remove => widgets::StatusTone::Pending,
    }
}

/// Matches the wording `administration_pages`' own profile-kind picker
/// already uses ("Generic"/"RomM"/"ES-DE"), so the same kind reads
/// identically wherever it appears in the GUI.
fn profile_kind_label(kind: FrontendProfileKind) -> &'static str {
    match kind {
        FrontendProfileKind::Generic => "Generic",
        FrontendProfileKind::Romm => "RomM",
        FrontendProfileKind::EsDe => "ES-DE",
    }
}

/// Draws the page.
pub(crate) fn show_library_view_history_page(
    ui: &mut egui::Ui,
    state: &mut LibraryViewHistoryPageState,
) {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::HISTORY,
        "Library View History",
        "Every Library View Apply/Remove operation EmuWiz has durably recorded to disk, newest \
         first. Read-only: nothing here can undo, repair, or re-apply a Library View.",
    );

    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                state.refresh();
            }
            ui.label(
                egui::RichText::new(format!("{} record(s)", state.entries.len()))
                    .color(theme::muted(ui)),
            );
        });
        ui.label(
            egui::RichText::new(
                "Always read fresh from the history directory on disk, never only from this \
                 session's memory - and never from the separate, in-memory History & Logs \
                 activity log, which does not survive a restart.",
            )
            .color(theme::muted(ui)),
        );
    });

    ui.add_space(8.0);

    if state.entries.is_empty() {
        widgets::empty_state(
            ui,
            "No Library View operations recorded yet",
            "Applying or removing a Library View will add a record here.",
            None,
        );
        return;
    }

    for entry in &state.entries {
        ui.add_space(6.0);
        match entry {
            LibraryViewHistoryEntry::Record { record, .. } => show_history_record(ui, record),
            LibraryViewHistoryEntry::Malformed { path, error } => {
                show_malformed_entry(ui, path, error)
            }
        }
    }
}

fn show_history_record(ui: &mut egui::Ui, record: &LibraryViewHistoryRecord) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                operation_label(record.operation),
                operation_tone(record.operation),
            );
            widgets::status_badge(
                ui,
                if record.success {
                    "Succeeded"
                } else {
                    "Failed"
                },
                if record.success {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Blocked
                },
            );
            ui.label(egui::RichText::new(&record.view_name).strong().size(15.0));
        });

        ui.label(
            egui::RichText::new(format!(
                "{} · {} profile",
                record.timestamp,
                profile_kind_label(record.profile_kind)
            ))
            .small()
            .color(theme::muted(ui)),
        );

        ui.label(
            egui::RichText::new(format!(
                "Created {} · Repaired {} · Removed {} · Unchanged {} · Failed {}{}",
                record.created,
                record.repaired,
                record.removed,
                record.unchanged,
                record.failed,
                record
                    .skipped_or_collision
                    .map(|count| format!(" · Skipped/collision {count}"))
                    .unwrap_or_default(),
            ))
            .small(),
        );

        if !record.warnings.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "{} warning(s) - see Technical details",
                    record.warnings.len()
                ))
                .small()
                .color(theme::muted(ui)),
            );
        }

        widgets::technical_details(ui, &record.manifest_path, |ui| {
            detail_label(ui, "Destination", &record.destination_root);
            detail_label(ui, "Manifest path", &record.manifest_path);
            detail_label(ui, "View id", &record.view_id);
            detail_label(ui, "Planned count", &record.planned_count.to_string());
            detail_label(ui, "Schema version", &record.schema_version.to_string());
            if !record.warnings.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Warnings:").strong().small());
                for warning in &record.warnings {
                    ui.label(egui::RichText::new(warning).small().color(theme::muted(ui)));
                }
            }
        });
    });
}

/// One corrupted/unreadable history file, shown as an honest warning rather
/// than hidden - it never hides the valid records around it, since the
/// caller loops over every entry regardless of which variant it is.
fn show_malformed_entry(ui: &mut egui::Ui, path: &std::path::Path, error: &str) {
    widgets::banner(
        ui,
        "A history record could not be read",
        &format!("{}: {error}", path.display()),
        widgets::StatusTone::Warning,
    );
}

fn detail_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_sized(
            [140.0, 0.0],
            egui::Label::new(egui::RichText::new(label).strong()),
        );
        ui.add(egui::Label::new(value).wrap());
    });
}

#[cfg(test)]
mod tests;
