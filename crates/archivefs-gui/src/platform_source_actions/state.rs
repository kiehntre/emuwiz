use crate::*;

/// One manual platform assignment change requested from the selected
/// archive's details panel - see `show_selected_archive`. Metadata-only:
/// unlike mount/unmount, this never depends on `latest_generation_actions_safe`
/// and is available for a cache-only/missing row exactly as for a live
/// one, since it only ever touches the library database, never the
/// filesystem or a mount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlatformAction {
    Set(String),
    Clear,
}

pub(crate) struct RunningPlatformAction {
    pub(crate) archive_path: PathBuf,
    pub(crate) receiver: Receiver<Result<PlatformAssignmentChange, String>>,
}

/// One bulk manual platform assignment change requested from the compact
/// "N archives selected" action bar - see `show_bulk_platform_action_bar`.
/// Metadata-only, exactly like `PlatformAction`: never depends on
/// `latest_generation_actions_safe`, never touches the filesystem or a
/// mount. Deliberately narrower than `PlatformAction`: no free-form
/// custom-text escape hatch (only `canonical_platform_names()`), matching
/// the bulk feature's "simple by default" scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BulkPlatformActionKind {
    Set(String),
    Clear,
}

/// The outcome of one bulk platform action applied at the GUI layer:
/// [`BulkPlatformAssignmentResult`] (archive-id-keyed, from the database
/// bulk API) plus how many of the *selected paths* never resolved to any
/// database archive id at all (a live-only/not-yet-scanned row, for
/// example) - a GUI-specific concern the database bulk API cannot see,
/// since it only ever receives ids. Kept as a separate, GUI-local
/// wrapper rather than adding a field to the shared core type, which the
/// CLI also uses and has no such "started from an exact PathBuf
/// selection" concept.
pub(crate) struct BulkPlatformActionOutcome {
    pub(crate) result: BulkPlatformAssignmentResult,
    pub(crate) unresolved_paths: usize,
}

pub(crate) struct RunningBulkPlatformAction {
    pub(crate) kind: BulkPlatformActionKind,
    pub(crate) requested_paths: usize,
    pub(crate) receiver: Receiver<Result<BulkPlatformActionOutcome, String>>,
}

/// Sentinel `platform_choice` value meaning "let the user type a custom
/// platform" - the GUI's escape hatch, mirroring the CLI's `--custom`
/// flag. Never itself sent as a platform value; `resolved_platform_choice`
/// substitutes the free-text field's contents instead.
pub(crate) const CUSTOM_PLATFORM_CHOICE: &str = "Custom...";

/// One custom-platform-alias database write requested from the "Custom
/// Platform Aliases" panel - see `show_platform_aliases_panel`.
/// Metadata-only, exactly like `PlatformAction`: never touches the
/// filesystem or a mount, and never triggers a rescan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AliasAction {
    Add { alias: String, platform: String },
    Remove { alias: String },
}

pub(crate) struct RunningAliasAction {
    pub(crate) action: AliasAction,
    pub(crate) receiver: Receiver<Result<(), String>>,
}

/// One Sources-page action requested from the background thread
/// `ArchiveFsApp::start_source_action` spawns - mirrors `AliasAction`
/// exactly. Every variant calls straight into the already-complete,
/// already-tested `archivefs_core` source-management functions (the same
/// ones the CLI's `source`/`sources` subcommands call - see
/// `run_source_action`); nothing here reimplements validation, scanning,
/// or persistence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SourceAction {
    Add(PathBuf),
    SetEnabled { path: PathBuf, enabled: bool },
    ScanOne(PathBuf),
    ScanAll,
    AssignPlatform { path: PathBuf, platform: String },
    Remove { path: PathBuf, keep_catalogue: bool },
}

/// What a completed [`SourceAction`] produced - just enough to let
/// `poll_source_action` build a truthful, specific feedback/Activity
/// message per variant, without re-deriving it from the refreshed
/// snapshot (which may already reflect *other* changes by the time it
/// reloads).
#[derive(Debug, Clone)]
pub(crate) enum SourceActionOutcome {
    Added(SourceFolderConfig),
    SetEnabled(SetSourceFolderEnabledOutcome),
    Scanned(ScanPersistSummary),
    PlatformAssigned {
        platform: String,
        scan: ScanPersistSummary,
    },
    Removed(RemoveSourceFolderOutcome),
}

pub(crate) struct RunningSourceAction {
    pub(crate) action: SourceAction,
    pub(crate) receiver: Receiver<Result<SourceActionOutcome, String>>,
    pub(crate) worker: Option<thread::JoinHandle<()>>,
}

/// Which source(s) a completed Sources-page scan covered - just enough to
/// show the result next to the right object (a single source's row, or a
/// page-level line for "all enabled sources"), never a claim about any
/// other source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourcesScanScope {
    One(PathBuf),
    AllEnabled,
}

/// The Sources page's own compact echo of its most recently completed
/// scan - rollup counts only (never a second copy of per-file skip
/// detail; that detail still lives solely in
/// `ScanPersistSummary::skipped_files`, reached the same way Database
/// Status already reaches it: via `show_skipped_files_window`). Set by
/// `poll_source_action` on a `SourceActionOutcome::Scanned` result so this
/// result is visible directly on the Sources page instead of only in the
/// separate Tools -> Database Status panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourcesLastScan {
    pub(crate) scope: SourcesScanScope,
    pub(crate) archives_found: i64,
    pub(crate) skipped_total: i64,
    /// The mixed-collection breakdown from
    /// `archivefs_core::ingestion::discover_source`, run alongside the
    /// archive scanner - see `ScanPersistSummary::ingestion_stats`.
    pub(crate) ingestion_stats: archivefs_core::ingestion::DiscoveryStats,
}

