use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;

use crate::ClipboardBackend;
use crate::ui::components as widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivityAction {
    Refresh,
    Mount,
    MountAll,
    UnmountAll,
    Unmount,
    LazyUnmount,
    Remount,
    Cleanup,
    Diagnostics,
    /// A Doctor repair attempt (Stage 1B). Recorded whatever the result, so
    /// a refused or failed repair is as visible as a successful one.
    DoctorRepair,
    Setup,
    LibraryDatabase,
    PlatformAssignment,
    BulkPlatformAssignment,
    PlatformAliasManagement,
    CatalogueCleanup,
    SourceAdded,
    SourceEnabled,
    SourceDisabled,
    SourceScan,
    SourceRemoved,
    LibraryViewAdded,
    LibraryViewEdited,
    LibraryViewEnabled,
    LibraryViewDisabled,
    LibraryViewPreview,
    LibraryViewApply,
    LibraryViewRepair,
    LibraryViewRemoved,
    /// Exporting the visible History & Logs entries to a file - recorded
    /// in the history itself so the export's own outcome is auditable.
    LogExport,
    /// Background RetroArch profile discovery for cheat setup (the
    /// Settings page's "Discovered Profiles" section).
    RetroArchProfileScan,
    /// Trusted cheat-source catalogue retrieval (network fetch or
    /// offline cached-snapshot reuse) from the cheat workflow.
    CheatSourceRetrieval,
    /// Read-only discovery of local PCSX2 configuration profiles.
    Pcsx2ProfileScan,
    /// Bounded read-only inventory of one PCSX2 profile's PNACH files.
    Pcsx2PnachInspection,
    /// Read-only discovery of local Dolphin user profiles.
    DolphinProfileScan,
    /// Bounded read-only inventory of Dolphin GameSettings INI files.
    DolphinGameIniInspection,
    /// Matching a verified GameCube game ID against a Dolphin profile's own
    /// GameSettings files for an exact Gecko cheat candidate - distinct
    /// from `DolphinGameIniInspection`, which only inventories files.
    DolphinGeckoCandidateMatch,
    /// Read-only discovery of explicitly supplied Xenia Canary directories.
    XeniaProfileScan,
    /// Retrieving, and matching Title ID/Media ID/module-hash compatibility
    /// against, the Xenia Canary game-patches upstream provider.
    XeniaPatchCandidateMatch,
    /// Shared bounded source-to-destination preview and conflict detection.
    CheatPreview,
    /// The confirmed, file-writing shared apply for a reviewed cheat
    /// installation - distinct from `CheatPreview`, which covers every
    /// read-only preview/inspection step leading up to it, so History &
    /// Logs can tell "previewed" apart from "actually installed".
    CheatInstall,
    /// Dolphin cheat catalogue download/update/rebuild/removal - distinct
    /// from `DolphinGeckoCandidateMatch`, which covers per-game provider
    /// lookups (local or network), not the catalogue itself.
    DolphinCatalogueRetrieval,
    /// A RomM identity-source operation: connection test, enable/disable,
    /// sample or full import, or clearing cached cover thumbnails. Recorded
    /// whatever the outcome, so a refused or cancelled import is as visible
    /// as a successful one.
    RommSource,
    /// A gated DAT rename apply: the user-reviewed, confirmed application of
    /// approved rename proposals. Distinct from every read-only preview.
    DatRenameApply,
    /// A rollback of a DAT rename transaction.
    DatRenameRollback,
}

/// Every `ActivityAction`, for the History & Logs "Operation" filter.
/// Must list each variant exactly once (checked by
/// `activity_filter_lists_cover_every_variant`).
pub(crate) const ALL_ACTIVITY_ACTIONS: [ActivityAction; 44] = [
    ActivityAction::Refresh,
    ActivityAction::Mount,
    ActivityAction::MountAll,
    ActivityAction::UnmountAll,
    ActivityAction::Unmount,
    ActivityAction::LazyUnmount,
    ActivityAction::Remount,
    ActivityAction::Cleanup,
    ActivityAction::Diagnostics,
    ActivityAction::Setup,
    ActivityAction::LibraryDatabase,
    ActivityAction::PlatformAssignment,
    ActivityAction::BulkPlatformAssignment,
    ActivityAction::PlatformAliasManagement,
    ActivityAction::CatalogueCleanup,
    ActivityAction::SourceAdded,
    ActivityAction::SourceEnabled,
    ActivityAction::SourceDisabled,
    ActivityAction::SourceScan,
    ActivityAction::SourceRemoved,
    ActivityAction::LibraryViewAdded,
    ActivityAction::LibraryViewEdited,
    ActivityAction::LibraryViewEnabled,
    ActivityAction::LibraryViewDisabled,
    ActivityAction::LibraryViewPreview,
    ActivityAction::LibraryViewApply,
    ActivityAction::LibraryViewRepair,
    ActivityAction::LibraryViewRemoved,
    ActivityAction::LogExport,
    ActivityAction::RetroArchProfileScan,
    ActivityAction::CheatSourceRetrieval,
    ActivityAction::Pcsx2ProfileScan,
    ActivityAction::Pcsx2PnachInspection,
    ActivityAction::DolphinProfileScan,
    ActivityAction::DolphinGameIniInspection,
    ActivityAction::DolphinGeckoCandidateMatch,
    ActivityAction::XeniaProfileScan,
    ActivityAction::XeniaPatchCandidateMatch,
    ActivityAction::CheatPreview,
    ActivityAction::CheatInstall,
    ActivityAction::DolphinCatalogueRetrieval,
    ActivityAction::RommSource,
    ActivityAction::DatRenameApply,
    ActivityAction::DatRenameRollback,
];

/// Every `ActivityOutcome`, for the History & Logs "Result" filter.
pub(crate) const ALL_ACTIVITY_OUTCOMES: [ActivityOutcome; 10] = [
    ActivityOutcome::Started,
    ActivityOutcome::Offered,
    ActivityOutcome::Retried,
    ActivityOutcome::Confirmed,
    ActivityOutcome::Cancelled,
    ActivityOutcome::Skipped,
    ActivityOutcome::Completed,
    ActivityOutcome::Failed,
    ActivityOutcome::Rejected,
    ActivityOutcome::OfflineUsable,
];

/// The History & Logs page's filter/sort state. `None` filters mean
/// "show everything" (the design's "All Operations"/"All Results").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HistoryLogFilters {
    pub(crate) action: Option<ActivityAction>,
    pub(crate) outcome: Option<ActivityOutcome>,
    pub(crate) oldest_first: bool,
    /// Free-text search over the entry's message, action label, and outcome
    /// label. Empty (the default) matches everything. Matched
    /// case-insensitively; never mutates or reorders the underlying
    /// history.
    pub(crate) text_query: String,
}

/// Whether one history entry passes the History & Logs filters - pure,
/// so filtering can never mutate or reorder the history itself.
pub(crate) fn history_entry_visible(entry: &HistoryEntry, filters: &HistoryLogFilters) -> bool {
    filters.action.is_none_or(|action| entry.action == action)
        && filters
            .outcome
            .is_none_or(|outcome| entry.outcome == outcome)
        && history_entry_matches_text(entry, &filters.text_query)
}

/// Whether `query` (matched case-insensitively, empty meaning "match
/// everything") appears in the entry's message, action label, or outcome
/// label.
pub(crate) fn history_entry_matches_text(entry: &HistoryEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query_lower = query.to_lowercase();
    entry.message.to_lowercase().contains(&query_lower)
        || entry
            .action
            .to_string()
            .to_lowercase()
            .contains(&query_lower)
        || entry
            .outcome
            .to_string()
            .to_lowercase()
            .contains(&query_lower)
}

/// The filtered, ordered entries the History & Logs page shows.
/// `OperationHistory::entries` iterates newest-first; `oldest_first`
/// reverses the *filtered* list without touching the underlying order.
pub(crate) fn visible_history_entries<'a>(
    history: &'a OperationHistory,
    filters: &HistoryLogFilters,
) -> Vec<&'a HistoryEntry> {
    let mut entries: Vec<&HistoryEntry> = history
        .entries()
        .filter(|entry| history_entry_visible(entry, filters))
        .collect();
    if filters.oldest_first {
        entries.reverse();
    }
    entries
}

impl std::fmt::Display for ActivityAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Refresh => "Refresh",
            Self::Mount => "Mount",
            Self::MountAll => "Mount All",
            Self::UnmountAll => "Unmount All",
            Self::Unmount => "Unmount",
            Self::LazyUnmount => "Lazy unmount",
            Self::Remount => "Remount",
            Self::Cleanup => "Cleanup",
            Self::Diagnostics => "Diagnostics",
            Self::DoctorRepair => "Doctor repair",
            Self::Setup => "Setup",
            Self::LibraryDatabase => "Library database",
            Self::PlatformAssignment => "Platform assignment",
            Self::BulkPlatformAssignment => "Bulk platform assignment",
            Self::PlatformAliasManagement => "Platform alias management",
            Self::CatalogueCleanup => "Catalogue cleanup",
            Self::SourceAdded => "Source added",
            Self::SourceEnabled => "Source enabled",
            Self::SourceDisabled => "Source disabled",
            Self::SourceScan => "Source scan",
            Self::SourceRemoved => "Source removed",
            Self::LibraryViewAdded => "Library View added",
            Self::LibraryViewEdited => "Library View edited",
            Self::LibraryViewEnabled => "Library View enabled",
            Self::LibraryViewDisabled => "Library View disabled",
            Self::LibraryViewPreview => "Library View preview",
            Self::LibraryViewApply => "Library View apply",
            Self::LibraryViewRepair => "Library View repair",
            Self::LibraryViewRemoved => "Library View removed",
            Self::LogExport => "Log export",
            Self::RetroArchProfileScan => "RetroArch profile scan",
            Self::CheatSourceRetrieval => "Cheat source retrieval",
            Self::Pcsx2ProfileScan => "PCSX2 profile scan",
            Self::Pcsx2PnachInspection => "PCSX2 PNACH inspection",
            Self::DolphinProfileScan => "Dolphin profile scan",
            Self::DolphinGameIniInspection => "Dolphin Game INI inspection",
            Self::DolphinGeckoCandidateMatch => "Dolphin Gecko candidate match",
            Self::XeniaProfileScan => "Xenia profile scan",
            Self::XeniaPatchCandidateMatch => "Xenia patch candidate match",
            Self::CheatPreview => "Cheats & Mods preview",
            Self::CheatInstall => "Cheats & Mods install",
            Self::DolphinCatalogueRetrieval => "Dolphin cheat catalogue retrieval",
            Self::RommSource => "RomM identity source",
            Self::DatRenameApply => "DAT rename apply",
            Self::DatRenameRollback => "DAT rename rollback",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivityOutcome {
    Started,
    Offered,
    Retried,
    Confirmed,
    Cancelled,
    Skipped,
    Completed,
    Failed,
    Rejected,
    /// A failed connection attempt that is not a failure in practice: the
    /// offline copy is still being served, so this reads as informational
    /// rather than as a scary global "Failed". The technical reason is kept
    /// in the entry's message.
    OfflineUsable,
}

impl std::fmt::Display for ActivityOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Started => "Started",
            Self::Offered => "Offered",
            Self::Retried => "Retried",
            Self::Confirmed => "Confirmed",
            Self::Cancelled => "Cancelled",
            Self::Skipped => "Skipped",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Rejected => "Rejected",
            Self::OfflineUsable => "Offline",
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HistoryEntry {
    pub(crate) timestamp: SystemTime,
    pub(crate) action: ActivityAction,
    pub(crate) archive_path: Option<PathBuf>,
    pub(crate) outcome: ActivityOutcome,
    pub(crate) message: String,
}

impl HistoryEntry {
    pub(crate) fn new(
        action: ActivityAction,
        archive_path: Option<PathBuf>,
        outcome: ActivityOutcome,
        message: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: SystemTime::now(),
            action,
            archive_path,
            outcome,
            message: message.into(),
        }
    }
}

#[derive(Default)]
pub(crate) struct OperationHistory {
    entries: VecDeque<HistoryEntry>,
}

impl OperationHistory {
    pub(crate) fn record(&mut self, entry: HistoryEntry) {
        self.entries.push_front(entry);
        self.entries.truncate(HISTORY_LIMIT);
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn remove(&mut self, index: usize) {
        self.entries.remove(index);
    }
}

pub(crate) const HISTORY_LIMIT: usize = 50;

pub(crate) const ACTIVITY_EXPANDED_BY_DEFAULT: bool = false;

/// Matches the collapsed activity panel's real content: one row of
/// buttons/badges plus its frame margin. Only used as the very first
/// frame's guess for the "activity_collapsed" panel id - actual content
/// height takes over immediately after and is what gets persisted.
pub(crate) const ACTIVITY_PANEL_COLLAPSED_DEFAULT_HEIGHT: f32 = 44.0;

/// Matches the expanded activity panel's real content: the button row,
/// separator, and the history list's own `max_height(220.0)` scroll area.
/// Only used as the very first frame's guess for the "activity_expanded"
/// panel id, for the same reason as the collapsed default above.
pub(crate) const ACTIVITY_PANEL_EXPANDED_DEFAULT_HEIGHT: f32 = 220.0;

pub(crate) enum ActivityPanelAction {
    ShowRelatedArchive(PathBuf),
}

pub(crate) fn activity_outcome_tone(outcome: ActivityOutcome) -> widgets::StatusTone {
    match outcome {
        ActivityOutcome::Completed => widgets::StatusTone::Success,
        ActivityOutcome::Failed | ActivityOutcome::Rejected => widgets::StatusTone::Blocked,
        ActivityOutcome::OfflineUsable => widgets::StatusTone::Info,
        ActivityOutcome::Started | ActivityOutcome::Retried | ActivityOutcome::Confirmed => {
            widgets::StatusTone::Active
        }
        ActivityOutcome::Offered | ActivityOutcome::Skipped | ActivityOutcome::Cancelled => {
            widgets::StatusTone::Pending
        }
    }
}

pub(crate) fn activity_summary_entry(history: &OperationHistory) -> Option<&HistoryEntry> {
    history
        .entries()
        .find(|entry| {
            matches!(
                entry.outcome,
                ActivityOutcome::Failed | ActivityOutcome::Rejected
            )
        })
        .or_else(|| history.entries().next())
}

pub(crate) fn show_activity_panel(
    context: &egui::Context,
    history: &mut OperationHistory,
    expanded: &mut bool,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<ActivityPanelAction> {
    let mut action = None;
    // Root cause of the bottom-clipping bug: `TopBottomPanel::bottom` picks
    // this frame's panel height by loading `PanelState` persisted under
    // its *own id* from the previous frame (egui's `panel.rs`), and only
    // falls back to a fresh default the very first time that id is ever
    // shown. Collapsed and expanded here render wildly different content
    // heights (one status row vs. a history list up to ~220px tall plus a
    // button row), but previously both used the *same* id ("activity") -
    // so the frame right after toggling from collapsed to expanded loaded
    // the collapsed height, squeezed the expanded content into it (that
    // content's own clip rect is the panel rect: see egui's `panel.rs`,
    // "If we overflow, don't do so visibly"), and only corrected itself
    // one frame later. A user's screenshot taken in that window - or
    // rendered while the app is between reactive repaints - shows exactly
    // "one line of content" jammed near the screen edge. Giving each
    // visual state its own id keeps their persisted heights from ever
    // contaminating each other, so there is no longer a wrong state to
    // render even transiently.
    let (panel_id, default_height) = if *expanded {
        ("activity_expanded", ACTIVITY_PANEL_EXPANDED_DEFAULT_HEIGHT)
    } else {
        (
            "activity_collapsed",
            ACTIVITY_PANEL_COLLAPSED_DEFAULT_HEIGHT,
        )
    };
    let maximum_height = if *expanded {
        (context.input(|input| input.screen_rect().height()) * 0.28)
            .clamp(120.0, ACTIVITY_PANEL_EXPANDED_DEFAULT_HEIGHT)
    } else {
        ACTIVITY_PANEL_COLLAPSED_DEFAULT_HEIGHT
    };
    egui::TopBottomPanel::bottom(panel_id)
        .resizable(*expanded)
        .default_height(default_height)
        .height_range(ACTIVITY_PANEL_COLLAPSED_DEFAULT_HEIGHT..=maximum_height)
        .show(context, |ui| {
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    if *expanded {
                        "Hide activity"
                    } else {
                        "Show activity"
                    },
                    widgets::ActionStyle::Quiet,
                    true,
                )
                .clicked()
                {
                    *expanded = !*expanded;
                }
                widgets::status_badge(
                    ui,
                    format!("{} events", history.len()),
                    widgets::StatusTone::Info,
                );
                if !*expanded && let Some(entry) = activity_summary_entry(history) {
                    widgets::status_badge(
                        ui,
                        entry.outcome.to_string(),
                        activity_outcome_tone(entry.outcome),
                    );
                    ui.add(egui::Label::new(&entry.message).truncate())
                        .on_hover_text(&entry.message);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if *expanded
                        && widgets::action_button(
                            ui,
                            "Clear activity history",
                            widgets::ActionStyle::Destructive,
                            history.entries().next().is_some(),
                        )
                        .clicked()
                    {
                        history.clear();
                    }
                });
            });
            if !*expanded {
                return;
            }
            ui.separator();

            if history.entries().next().is_none() {
                ui.weak("No recent activity.");
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("activity_history")
                .max_height(220.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Collected as owned data *before* the loop, rather
                    // than iterating `history.entries()` directly, so a
                    // menu item can freely call `history.clear()`/
                    // `history.remove()` without fighting the borrow
                    // checker over a `history` still being iterated.
                    let rows: Vec<(
                        usize,
                        ActivityAction,
                        ActivityOutcome,
                        String,
                        Option<PathBuf>,
                    )> = history
                        .entries()
                        .enumerate()
                        .map(|(index, entry)| {
                            (
                                index,
                                entry.action,
                                entry.outcome,
                                entry.message.clone(),
                                entry.archive_path.clone(),
                            )
                        })
                        .collect();
                    let mut remove_index = None;
                    for (index, activity, outcome, text, archive_path) in &rows {
                        let response = widgets::card(ui, |ui| {
                            widgets::activity_row_header(
                                ui,
                                outcome.to_string(),
                                activity_outcome_tone(*outcome),
                                activity.to_string(),
                                None,
                                |_ui| {},
                            );
                            ui.add(
                                egui::Label::new(text)
                                    .selectable(true)
                                    .wrap()
                                    .sense(egui::Sense::click()),
                            )
                        });
                        ui.add_space(6.0);
                        response.context_menu(|ui| {
                            if ui.button("Copy message").clicked() {
                                let _ = clipboard.set_text(text.clone());
                                ui.close();
                            }
                            if let Some(archive_path) = archive_path {
                                if ui.button("Copy related path").clicked() {
                                    let _ = clipboard.set_text(archive_path.display().to_string());
                                    ui.close();
                                }
                                if ui.button("Show related archive").clicked() {
                                    action = Some(ActivityPanelAction::ShowRelatedArchive(
                                        archive_path.clone(),
                                    ));
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if ui.button("Remove this entry").clicked() {
                                remove_index = Some(*index);
                                ui.close();
                            }
                            if ui.button("Clear activity history").clicked() {
                                history.clear();
                                ui.close();
                            }
                        });
                    }
                    // Deferred to after the loop: removing mid-iteration
                    // would shift every later index out from under `rows`.
                    if let Some(index) = remove_index {
                        history.remove(index);
                    }
                });
        });
    action
}
