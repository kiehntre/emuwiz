use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::SystemTime;

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
