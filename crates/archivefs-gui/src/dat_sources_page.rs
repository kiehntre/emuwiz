//! The DAT Sources page: register local DAT catalogues, check them, audit
//! against them.
//!
//! # Why this page exists
//!
//! DAT parsing, indexing, and the read-only audit have shipped for a while,
//! reachable only through `archivefs-cli dat …`, and only ever for a path typed
//! afresh each time. There was nowhere to *keep* a DAT. This page is that
//! place, and everything it does calls the same core the CLI does.
//!
//! # The shape, following Cheat Sources
//!
//! Authoritative state becomes a [`DatSourcesPageView`] through a pure
//! function, and the drawing code only draws. The properties worth testing
//! here (that a disabled source is still listed, that removing one does not
//! delete a file, that an audit reports only verdicts the core produced) are
//! data questions, answerable without a frame buffer.
//!
//! [`DatSourcesPageState`] holds a `saved` registry and a `draft` one. Edits
//! touch the draft; the file is written only on Save. The difference between
//! the two *is* the unsaved-change state, so "is this dirty?" cannot drift from
//! "would saving change anything?".
//!
//! # One background job at a time
//!
//! Validating a 200 MB catalogue and auditing a library are both long enough to
//! freeze a window, so both run on a worker thread with a cancellation flag and
//! a bounded progress channel. One job runs at a time: a second concurrent
//! parse of the same source would race for no benefit, and the design this
//! follows calls for at most one source operation in flight.
//!
//! # Nothing here writes to a ROM
//!
//! The page writes only its own durable DAT configuration: the local registry,
//! typed managed-source configuration, and persisted TOSEC pack selections.
//! Validation reads DAT files; an audit reads DAT files and ROMs. Removing a
//! source removes a registry entry. There is no rename, move, delete, archive
//! rewrite, or symlink change anywhere on this page, and none is deferred
//! behind a flag - the capability is simply not present.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Instant;

use archivefs_core::dat::classification::{
    ContentSelectionPolicy, DatContentClassification, DatContentSummary,
};
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::managed_sources::{
    ManagedDatSources, default_managed_dat_sources_config_path, load_managed_dat_sources_from,
    resolve_managed_dat_sources, resolve_redump_bios_sources, resolve_redump_games_sources,
    save_managed_dat_sources_to,
};
use archivefs_core::dat::model::DatFormat;
use archivefs_core::dat::parser::DiagnosticSeverity;
use archivefs_core::dat::parsers::parse_dat_file;
use archivefs_core::dat::policy::{
    ClonePolicy, DatPolicyConfig, EffectiveDatPolicy, LanguageId, LanguagePreference, PolicyField,
    RegionId, RevisionPolicy, participating_sources, resolve, validate_policy_config,
};
use archivefs_core::dat::rename_apply::{
    ApplyError, ApplyExecution, ApplyOutcome, EntryState, HardConflictMode, RollbackOutcome,
    RollbackResult, TransactionEntry, TransactionState, apply_transaction, rollback_transaction,
};
use archivefs_core::dat::rename_plan::{
    ProposalState, RenamePlan, RenamePlanContext, ReviewDecision, build_rename_plan,
};
use archivefs_core::dat::sources::audit_run::{
    CombinedDatAuditRequest, CombinedDatAuditSource, DatAuditOutcome, DatAuditProgress,
    DatAuditRequest, run_combined_dat_audit, run_dat_audit,
};
use archivefs_core::dat::sources::{
    DatFileOutcome, DatHealthState, DatSourceEntry, DatSourceHealth, DatSourceKind,
    DatSourceRegistry, DatValidationReport, UnresolvedDatSetting, load_dat_sources_config_from,
    save_dat_sources_config_to, suggest_display_name, validate_dat_source,
};
use archivefs_core::dat::tosec_release_pack::{
    PackAvailability, PersistedTosecPack, TosecPackDat, TosecSelectionKey,
    apply_selection_to_registry, default_tosec_packs_path, inventory_release_pack,
    load_tosec_packs, save_tosec_packs,
};
use archivefs_core::dat::updates::{
    HttpsManagedDatTransport, ManagedDatProvider, ManagedDatReadOnlySource,
    ManagedDatSourceDescriptor, ManagedDatSourceId, ManagedDatState, ManagedDatUpdateFailureKind,
    ManagedDatUpdateOptions, ManagedDatUpdateOutcome, ManagedDatUpdatePolicy, RedumpBiosSystem,
    RedumpGameSystem, check_managed_dat_update, managed_dat_root, update_managed_dat,
};
use archivefs_core::identity_source::no_intro::{
    NO_INTRO_DATOMATIC_DOWNLOAD_PAGE, NoIntroPackClassification, NoIntroPackImportStatus,
    NoIntroPackInspection, import_no_intro_pack, inspect_no_intro_pack,
    load_current_no_intro_pack_summary,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// Said once on the page, because a DAT audit is the one place a user might
/// reasonably expect a "fix it" button and there is deliberately not one.
pub(crate) const READ_ONLY_PROMISE: &str = "Your files won't be renamed unless you approve it. A DAT audit only reads files and \
     reports what it found; it does not move, delete or rewrite them.";
/// The short, simple promise shown on the page. Longer detail stays available
/// as a tooltip (`READ_ONLY_PROMISE`) but never front and centre.
pub(crate) const SAFE_PROMISE: &str = "Your files won't be renamed unless you approve it.";
pub(crate) const GAMES_ONLY_EXPLANATION: &str = "Games only hides entries confidently identified as things like magazines, music, demos, manuals, or other non-game material. Unknown entries are kept for review.";
/// The prominent, repeated statement the rename-planning section must show.
pub(crate) const PLAN_ONLY_PROMISE: &str = "Planning only — EmuWiz will not rename any files. This section derives suggested names \
     from verified DAT matches and explains them; nothing here changes, moves, deletes or rewrites \
     a file.";

/// Batches larger than this require a typed confirmation phrase such as
/// "RENAME 42 FILES" before any rename happens.
pub(crate) const TYPED_CONFIRMATION_THRESHOLD: usize = 8;

/// The exact phrase a user must type to confirm a large batch.
pub(crate) fn typed_confirmation_phrase(count: usize) -> String {
    format!("RENAME {count} FILES")
}

/// What Stage 1 supports, stated rather than implied by what happens to work.
pub(crate) const SUPPORTED_FORMATS: &str = "Logiqx XML (No-Intro, Redump) and ClrMamePro text (TOSEC, generic). Other formats are not \
     supported and are not silently accepted.";

/// How many progress messages may be queued before older ones are dropped.
///
/// A run over 25,000 files produces a message per file; if the window is busy,
/// an unbounded queue would grow until the run finished. Dropping progress is
/// free - the next message supersedes it - so the send is non-blocking and a
/// full channel simply means the display is a little behind.
const PROGRESS_QUEUE_DEPTH: usize = 64;

/// Files that must be processed before an ETA can be trusted at all.
const ETA_MIN_FILES: u64 = 100;

/// Seconds that must have elapsed before an ETA can be trusted at all.
const ETA_MIN_SECONDS: f64 = 5.0;

/// Blend for the exponential moving average of throughput: a single frame's
/// speed moves the estimate by this fraction of the way, so one fast sample
/// cannot make the ETA jump.
const ETA_SMOOTHING_ALPHA: f64 = 0.2;

// ---------------------------------------------------------------------------
// View model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DatSaveState {
    Idle,
    Saved,
    Failed(String),
}

/// One source's row, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatSourceRowView {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) path: String,
    pub(crate) kind_label: &'static str,
    pub(crate) enabled: bool,
    /// The assigned platform's display name, or `None` when unassigned.
    pub(crate) platform_display: Option<String>,
    /// The raw assigned ID, needed to keep the picker's exclusion list right.
    pub(crate) platform_id: Option<String>,
    /// The assignment names a platform this build does not know.
    pub(crate) platform_unresolved: bool,
    /// Formats the last validation actually saw. Empty until validated - this
    /// is never guessed from the filename.
    pub(crate) formats: Vec<String>,
    pub(crate) health_state: DatHealthState,
    pub(crate) health_detail: Option<String>,
    pub(crate) last_validated: Option<String>,
    /// The DAT changed since the stored verdict was taken, so the verdict
    /// describes a file that is no longer there.
    pub(crate) health_stale: bool,
    pub(crate) entry_count: Option<u64>,
    pub(crate) rom_count: Option<u64>,
    /// This row differs from what is on disk.
    pub(crate) changed: bool,
    /// A background job is running for this source.
    pub(crate) busy: bool,
    /// The last validation run's per-file breakdown, if one has been run.
    pub(crate) detail: Option<InspectView>,
    /// The last validation's diagnostics, grouped by (severity, code, message)
    /// and sorted by severity then code then message. Deterministic, bounded,
    /// and never reparsed to build.
    pub(crate) groups: Vec<DiagnosticGroupView>,
    /// The bounded safety limit stopped the last validation part-way through a
    /// folder, so the verdict covers only part of it. Must never be presented
    /// as "everything was checked".
    pub(crate) incomplete_load: bool,
    /// How many DAT files the last (incomplete) validation actually read.
    pub(crate) dat_files_read: Option<u64>,
    /// How many DAT files the folder holds, when genuinely known.
    pub(crate) dat_files_total: Option<u64>,
    /// Whether the full warning details are recorded in History & Logs. The
    /// card only ever points there when this is true; today the details are
    /// kept inline instead, so this stays false.
    pub(crate) history_link_available: bool,
}

impl DatSourceRowView {
    /// The line describing an incomplete catalogue load, or `None` when the
    /// load was complete.
    ///
    /// "512 of 2,024 DAT files read" is shown only when both numbers are
    /// genuinely known; otherwise the safety limit is named without inventing
    /// a total.
    pub(crate) fn incomplete_load_line(&self) -> Option<String> {
        if !self.incomplete_load {
            return None;
        }
        match (self.dat_files_read, self.dat_files_total) {
            (Some(read), Some(total)) => Some(format!("{read} of {total} DAT files read")),
            _ => Some("Processing stopped at the configured safety limit".to_string()),
        }
    }

    /// The groups of one severity, in deterministic order.
    pub(crate) fn groups_of(&self, severity: DiagnosticSeverity) -> Vec<&DiagnosticGroupView> {
        self.groups
            .iter()
            .filter(|group| group.severity == severity)
            .collect()
    }

    /// How many distinct diagnostic types (groups) have this severity.
    pub(crate) fn diagnostic_types(&self, severity: DiagnosticSeverity) -> usize {
        self.groups_of(severity).len()
    }

    /// How many total occurrences of this severity were found.
    pub(crate) fn diagnostic_occurrences(&self, severity: DiagnosticSeverity) -> usize {
        self.groups_of(severity)
            .iter()
            .map(|group| group.occurrence_count)
            .sum()
    }
}

/// The policy fields of one scope, borrowed for editing.
///
/// This is what lets a policy edit act on "global or this platform's
/// override" through one code path; the field shapes of
/// [`DatPolicyConfig`] and its per-platform override differ, but the four
/// editable fields are the same.
struct PolicyTargets<'a> {
    region: &'a mut Option<Vec<String>>,
    language: &'a mut Option<Vec<String>>,
    revision: &'a mut Option<String>,
    clone: &'a mut Option<String>,
    content: &'a mut Option<String>,
}

impl<'a> PolicyTargets<'a> {
    fn new(
        region: &'a mut Option<Vec<String>>,
        language: &'a mut Option<Vec<String>>,
        revision: &'a mut Option<String>,
        clone: &'a mut Option<String>,
        content: &'a mut Option<String>,
    ) -> Self {
        Self {
            region,
            language,
            revision,
            clone,
            content,
        }
    }

    /// The region list, creating it when absent.
    fn region_list(&mut self) -> &mut Vec<String> {
        self.region.get_or_insert_with(Vec::new)
    }

    /// The language list, creating it when absent.
    fn language_list(&mut self) -> &mut Vec<String> {
        self.language.get_or_insert_with(Vec::new)
    }

    /// Turns a list that has become empty back into "absent".
    ///
    /// Removing the last preference should mean "no preference, inherit the
    /// parent scope" - the `None` state - not "this scope explicitly prefers
    /// nothing", which is a distinction only a hand-edited file needs.
    fn normalise_empty_lists(&mut self) {
        if self.region.as_ref().is_some_and(Vec::is_empty) {
            *self.region = None;
        }
        if self.language.as_ref().is_some_and(Vec::is_empty) {
            *self.language = None;
        }
    }
}

/// Moves `list[index]` to `index + delta` (`-1` up, `+1` down), refusing to
/// move out of range. This is a *move*, not a swap: an entry moved up two
/// places passes over both neighbours rather than exchanging places with the
/// first one.
fn move_index(list: &mut Vec<String>, index: usize, delta: i32) {
    let target = match index.checked_add_signed(delta as isize) {
        Some(target) if target < list.len() => target,
        _ => return,
    };
    if target == index {
        return;
    }
    let value = list.remove(index);
    list.insert(target, value);
}

/// One DAT file inside a source, as the Inspect panel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InspectFileView {
    pub(crate) file_name: String,
    pub(crate) status: &'static str,
    /// The format and counts, or the parser's error.
    pub(crate) detail: String,
    pub(crate) errors: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) notes: Vec<String>,
}

/// How many occurrence rows a diagnostic group will show before truncating.
/// The drill-down is bounded so a folder of hundreds of affected files cannot
/// produce an unbounded list.
pub(crate) const MAX_DIAGNOSTIC_OCCURRENCES_SHOWN: usize = 50;

/// One occurrence of a diagnostic, with the safe DAT filename and the location
/// the parser recorded (when it records one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticOccurrenceView {
    /// The DAT file's name only - never an absolute path.
    pub(crate) file_name: String,
    pub(crate) line: Option<usize>,
    pub(crate) column: Option<usize>,
}

/// Repeated diagnostics from a validation run, grouped into one row per
/// distinct (severity, code, message) so 512 identical DOCTYPE notes render as
/// one group with an occurrence count instead of 512 lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticGroupView {
    pub(crate) severity: DiagnosticSeverity,
    /// The stable parser code, e.g. "trusted_dtd_unavailable".
    pub(crate) code: &'static str,
    /// The diagnostic message, verbatim, deduplicated by the group key.
    pub(crate) message: String,
    /// Total occurrences across every DAT file in the source.
    pub(crate) occurrence_count: usize,
    /// Distinct DAT files that produced at least one occurrence.
    pub(crate) affected_file_count: usize,
    /// A bounded, deterministic list of occurrences (safe filename + location).
    pub(crate) occurrences: Vec<DiagnosticOccurrenceView>,
    /// More occurrences exist than are listed in `occurrences`.
    pub(crate) occurrences_truncated: bool,
    /// A stable id for the disclosure toggle.
    pub(crate) id: String,
}

/// What the last validation run found, in detail.
///
/// Present only after a source has actually been validated this session: the
/// persisted health carries a summary, but the per-file breakdown is not
/// written to the registry, because it can be several hundred lines and is
/// reproducible in a second by checking again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InspectView {
    pub(crate) files: Vec<InspectFileView>,
    /// Catalogue identities claimed by more than one file in a folder source.
    pub(crate) duplicate_identities: Vec<String>,
    /// Files in a folder source that were looked at and not taken, with why.
    pub(crate) skipped: Vec<String>,
    pub(crate) truncated: bool,
}

/// A setting kept but not understood, shown read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnresolvedDatRowView {
    pub(crate) explanation: String,
}

/// What a running job is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningJobView {
    pub(crate) source_id: String,
    pub(crate) what: &'static str,
    pub(crate) detail: String,
    pub(crate) cancellable: bool,
    /// True from the moment Cancel is pressed until the worker confirms it is
    /// gone. The card reads "Stopping…" while this is set, and the job stays
    /// busy until then - a stale progress line cannot restore an active look.
    pub(crate) cancellation_requested: bool,
    /// Structured audit progress, when the running job is an audit.
    pub(crate) progress: Option<AuditProgressView>,
    /// The source's assigned platform, shown only when it is authoritative
    /// (assigned and recognised by this build). Never guessed from the path.
    pub(crate) platform_display: Option<String>,
}

impl RunningJobView {
    /// The heading: "Auditing 'collection'" normally, "Stopping 'collection'…"
    /// the moment Cancel has been pressed. An empty `source_id` (only ever
    /// `ValidateAll`, which has no single source to name) omits the quoted
    /// subject entirely rather than rendering empty quotes.
    pub(crate) fn heading(&self) -> String {
        let verb = if self.cancellation_requested {
            "Stopping"
        } else {
            self.what
        };
        if self.source_id.is_empty() {
            verb.to_string()
        } else {
            format!("{verb} '{}'", self.source_id)
        }
    }
}

/// The ETA, in the only three states a running card can honestly show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EtaView {
    /// No estimate is possible yet - no samples, no total, or nothing left to
    /// estimate. Draw nothing.
    None,
    /// The run is progressing but has not gone far or long enough to trust a
    /// number. Draw "Estimating time remaining…".
    Estimating,
    /// A concrete estimate, in whole seconds remaining.
    About { seconds_remaining: u64 },
}

/// Structured progress for a running audit, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuditProgressView {
    pub(crate) phase: &'static str,
    pub(crate) files_checked: u64,
    pub(crate) total_files: Option<u64>,
    /// The current folder or file, shortened for display. The full private
    /// path is never turned into a display string.
    pub(crate) current_path: Option<String>,
    pub(crate) elapsed_seconds: u64,
    pub(crate) percent: Option<u8>,
    pub(crate) eta: EtaView,
}

impl AuditProgressView {
    /// The position: "42 of 100" when the total is known, "42 files so far"
    /// when it is not. Never invents a count the run has not produced.
    pub(crate) fn position(&self) -> String {
        match self.total_files {
            Some(total) => format!("{} of {total}", self.files_checked),
            None => format!("{} files so far", self.files_checked),
        }
    }

    /// One line describing where the run is and how long it has taken.
    pub(crate) fn line(&self) -> String {
        let percentage = self
            .percent
            .map(|percent| format!(" ({percent}%)"))
            .unwrap_or_default();
        format!(
            "{} · {}{percentage} · {} elapsed",
            self.phase,
            self.position(),
            format_elapsed(self.elapsed_seconds)
        )
    }

    /// The ETA line, or `None` when nothing should be drawn.
    pub(crate) fn eta_line(&self) -> Option<String> {
        match &self.eta {
            EtaView::None => None,
            EtaView::Estimating => Some("Estimating time remaining…".to_string()),
            EtaView::About { seconds_remaining } => Some(format_eta_remaining(*seconds_remaining)),
        }
    }
}

/// Everything the page draws.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DatSourcesPageView {
    pub(crate) rows: Vec<DatSourceRowView>,
    /// Separately configured, app-managed DATs. These are never inferred from
    /// a local source's name, origin, or path.
    pub(crate) managed_rows: Vec<ManagedDatSourceRowView>,
    /// Fixed Redump BIOS datasets are separate from ordinary game/disc DATs
    /// even though both use the same typed managed-DAT updater.
    pub(crate) redump_bios_rows: Vec<ManagedDatSourceRowView>,
    /// The deliberately closed set of ordinary Redump game/disc datasets.
    /// Unconfigured systems remain visible so configuration is explicit.
    pub(crate) redump_game_rows: Vec<ManagedDatSourceRowView>,
    pub(crate) managed_load_error: Option<String>,
    pub(crate) managed_action_error: Option<String>,
    pub(crate) tosec_packs: Vec<TosecPackView>,
    pub(crate) tosec_load_error: Option<String>,
    pub(crate) tosec_action_error: Option<String>,
    pub(crate) tosec_last_apply: Option<TosecApplyView>,
    pub(crate) no_intro_selected_pack: Option<(String, u64)>,
    pub(crate) no_intro_inspection: Option<NoIntroPackInspection>,
    pub(crate) no_intro_installed: Option<NoIntroPackInspection>,
    pub(crate) no_intro_action_error: Option<String>,
    pub(crate) no_intro_import_status: Option<NoIntroPackImportStatus>,
    pub(crate) unresolved: Vec<UnresolvedDatRowView>,
    /// Problems found while reading the file that this build could not act on
    /// (an unusable ID, a second entry claiming one ID).
    pub(crate) load_problems: Vec<String>,
    pub(crate) dirty: bool,
    pub(crate) config_path: PathBuf,
    pub(crate) save_state: DatSaveState,
    pub(crate) load_error: Option<String>,
    /// The last add/remove attempt that was refused, with its reason.
    pub(crate) action_error: Option<String>,
    pub(crate) pending_consequences: Vec<String>,
    pub(crate) running: Option<RunningJobView>,
    /// Any page-owned worker is running, including an explicit managed-DAT
    /// check/update which intentionally has no generic cancellation button.
    pub(crate) background_busy: bool,
    /// The final tally of the last completed (or cancelled) "Validate all"
    /// run, this session.
    pub(crate) last_validate_all_summary: Option<ValidateAllSummary>,
    /// The folders offered as audit targets: the configured library source
    /// folders, in configuration order.
    pub(crate) library_folders: Vec<PathBuf>,
    pub(crate) audit: Option<Box<AuditResultView>>,
    pub(crate) audit_error: Option<String>,
    pub(crate) identity_enrichment: Option<Box<archivefs_core::PlatformIdentityEnrichmentSummary>>,
    /// The DAT Matching Policy section, including the Effective Policy
    /// Summary resolved for the scope the user is inspecting.
    pub(crate) policy: DatPolicyView,
    /// The read-only rename-planning section, present when the latest audit
    /// produced a plan.
    pub(crate) rename_plan: Option<RenamePlanView>,
    /// The gated apply flow: review, confirmation, results and crash recovery.
    pub(crate) rename_apply: RenameApplyView,
}

impl DatSourcesPageView {
    /// Whether the page has nothing registered yet.
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// One explicitly configured, typed managed MAME software-list source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedDatSourceRowView {
    pub(crate) source_id: ManagedDatSourceId,
    pub(crate) provider: ManagedDatProvider,
    pub(crate) source_label: String,
    pub(crate) authoritative_name: String,
    pub(crate) configured: bool,
    pub(crate) update_policy: ManagedDatUpdatePolicy,
    pub(crate) installed: bool,
    pub(crate) current_revision: Option<String>,
    pub(crate) last_checked: Option<String>,
    pub(crate) status: ManagedDatStatusView,
    pub(crate) update_enabled: bool,
    pub(crate) busy: bool,
    pub(crate) technical: ManagedDatTechnicalView,
}

/// A bounded, read-only projection of one persisted TOSEC release pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TosecPackView {
    pub(crate) pack_id: String,
    pub(crate) root_path: PathBuf,
    pub(crate) availability: PackAvailability,
    pub(crate) imported_at: String,
    pub(crate) dat_count: usize,
    pub(crate) selected_dat_count: usize,
    pub(crate) groups: Vec<TosecSelectionGroupView>,
    pub(crate) deferred_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TosecSelectionGroupView {
    pub(crate) key: TosecSelectionKey,
    pub(crate) dat_count: usize,
    pub(crate) selected: bool,
    pub(crate) deferred_count: usize,
    pub(crate) raw_categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TosecApplyView {
    pub(crate) pack_id: String,
    pub(crate) registered: usize,
    pub(crate) already_registered: usize,
    pub(crate) removed: usize,
    pub(crate) deferred: usize,
    pub(crate) conflicts: usize,
    pub(crate) failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedDatTechnicalView {
    pub(crate) sha256: Option<String>,
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
    pub(crate) current_path: Option<String>,
    pub(crate) previous_snapshot: Option<String>,
    pub(crate) previous_path: Option<String>,
}

/// The human-facing result of an explicit managed-DAT operation. It is kept
/// separate from the persisted state: no status text is updater authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagedDatStatusView {
    NotInstalled,
    Idle,
    Checking,
    UpdateAvailable { upstream_revision: String },
    UpToDate,
    Updating,
    Updated,
    Offline,
    RateLimited { retry_after_seconds: Option<u64> },
    Disabled,
    Failed { detail: String },
}

fn managed_dat_status_from_outcome(outcome: ManagedDatUpdateOutcome) -> ManagedDatStatusView {
    match outcome {
        ManagedDatUpdateOutcome::Disabled => ManagedDatStatusView::Disabled,
        ManagedDatUpdateOutcome::UpToDate { .. } => ManagedDatStatusView::UpToDate,
        ManagedDatUpdateOutcome::UpdateAvailable { upstream_revision } => {
            ManagedDatStatusView::UpdateAvailable { upstream_revision }
        }
        ManagedDatUpdateOutcome::Updated { .. } => ManagedDatStatusView::Updated,
        ManagedDatUpdateOutcome::Offline => ManagedDatStatusView::Offline,
        ManagedDatUpdateOutcome::RateLimited {
            retry_after_seconds,
        } => ManagedDatStatusView::RateLimited {
            retry_after_seconds,
        },
        ManagedDatUpdateOutcome::Failed { kind, detail } => ManagedDatStatusView::Failed {
            detail: managed_dat_failure_message(kind, &detail),
        },
    }
}

fn managed_dat_failure_message(kind: ManagedDatUpdateFailureKind, detail: &str) -> String {
    let message = match kind {
        ManagedDatUpdateFailureKind::Forbidden | ManagedDatUpdateFailureKind::NotFound => {
            "Source unavailable"
        }
        ManagedDatUpdateFailureKind::Parser
        | ManagedDatUpdateFailureKind::WrongEcosystem
        | ManagedDatUpdateFailureKind::WrongAuthoritativeName
        | ManagedDatUpdateFailureKind::EmptyCatalogue
        | ManagedDatUpdateFailureKind::DownloadTooLarge
        | ManagedDatUpdateFailureKind::EmptyDownload
        | ManagedDatUpdateFailureKind::TruncatedDownload => {
            "Downloaded DAT failed validation; current copy kept"
        }
        ManagedDatUpdateFailureKind::Storage => "Could not store update; current copy kept",
        ManagedDatUpdateFailureKind::Network
        | ManagedDatUpdateFailureKind::Timeout
        | ManagedDatUpdateFailureKind::Tls => "Update check failed; existing DAT remains available",
        ManagedDatUpdateFailureKind::HttpStatus | ManagedDatUpdateFailureKind::InvalidResponse => {
            "Update check failed; current copy kept"
        }
    };
    if detail.is_empty() {
        message.to_string()
    } else {
        format!("{message}: {detail}")
    }
}

// ---------------------------------------------------------------------------
// Rename planning view model
// ---------------------------------------------------------------------------

/// How the plan rows are filtered for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RenamePlanFilter {
    #[default]
    All,
    Suggested,
    AlreadyCanonical,
    Ambiguous,
    Conflicts,
    Unsupported,
    Blocked,
}

impl RenamePlanFilter {
    pub(crate) const ALL: [RenamePlanFilter; 7] = [
        RenamePlanFilter::All,
        RenamePlanFilter::Suggested,
        RenamePlanFilter::AlreadyCanonical,
        RenamePlanFilter::Ambiguous,
        RenamePlanFilter::Conflicts,
        RenamePlanFilter::Unsupported,
        RenamePlanFilter::Blocked,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Suggested => "Suggested",
            Self::AlreadyCanonical => "Already canonical",
            Self::Ambiguous => "Ambiguous",
            Self::Conflicts => "Conflicts",
            Self::Unsupported => "Unsupported",
            Self::Blocked => "Blocked",
        }
    }

    fn matches(self, row: &RenamePlanRowView) -> bool {
        match self {
            Self::All => true,
            Self::Suggested => row.state == ProposalState::Suggested,
            Self::AlreadyCanonical => row.state == ProposalState::AlreadyCanonical,
            Self::Ambiguous => row.state == ProposalState::Ambiguous,
            Self::Conflicts => row.state == ProposalState::Conflict,
            Self::Unsupported => row.state == ProposalState::Unsupported,
            Self::Blocked => matches!(
                row.state,
                ProposalState::Blocked
                    | ProposalState::ExcludedByContentPolicy
                    | ProposalState::UnclassifiedContent
            ),
        }
    }
}

/// The rename-planning section: the read-only plan derived from the latest
/// audit, with its counts and every row (the active filter lives in
/// [`DatSourcesPageUi`] and selects which rows are drawn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenamePlanView {
    pub(crate) generation: u64,
    /// The full, unshortened folder this plan audited - used only for exact
    /// current-library comparisons (see `transaction_targets_root`), never
    /// displayed; `scan_root_short` is what the UI shows.
    pub(crate) scan_root: String,
    pub(crate) scan_root_short: String,
    pub(crate) platform_display: Option<String>,
    pub(crate) source_display_name: String,
    pub(crate) counts: archivefs_core::dat::rename_plan::RenamePlanCounts,
    pub(crate) audited_total: usize,
    pub(crate) verified_total: usize,
    pub(crate) truncated: bool,
    pub(crate) rows: Vec<RenamePlanRowView>,
    pub(crate) error: Option<String>,
}

/// One plan row, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenamePlanRowView {
    /// The source file's path. The row renders the basename and a shortened
    /// parent; the full path is kept only for identity and future apply work.
    pub(crate) source_path: PathBuf,
    pub(crate) current_basename: String,
    pub(crate) proposed_basename: Option<String>,
    pub(crate) platform_display: Option<String>,
    pub(crate) source_display_name: String,
    pub(crate) game_name: Option<String>,
    pub(crate) rom_name: Option<String>,
    pub(crate) verdict_label: String,
    pub(crate) content: ContentTechnicalView,
    pub(crate) state: ProposalState,
    pub(crate) object_kind_label: &'static str,
    pub(crate) explanations: Vec<String>,
    pub(crate) ambiguity_reason: Option<String>,
    pub(crate) collision_detail: Option<String>,
    pub(crate) blockers: Vec<String>,
    pub(crate) extension_preserved: bool,
    pub(crate) sanitisation_notes: Vec<String>,
    pub(crate) decision: Option<ReviewDecision>,
}

// ---------------------------------------------------------------------------
// Rename apply view model
// ---------------------------------------------------------------------------

/// The apply/recovery section of the rename-planning flow.
///
/// Everything the user sees here is derived by the core executor; the GUI only
/// renders the built transaction, records confirmations, and runs the core
/// apply/rollback functions on a worker thread. It never calls `fs::rename`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameApplyView {
    /// The read-only review of the transaction the user is about to confirm.
    pub(crate) review: Option<ApplyReviewView>,
    /// The last apply outcome, with per-file results.
    pub(crate) outcome: Option<ApplyOutcomeView>,
    pub(crate) apply_error: Option<String>,
    /// Whether the safe-subset option is offered after an AbortAll conflict.
    pub(crate) subset_available: bool,
    pub(crate) rollback_result: Option<RollbackResultView>,
    pub(crate) rollback_error: Option<String>,
    pub(crate) apply_running: bool,
    pub(crate) rollback_running: bool,
    /// Interrupted transactions found on disk, offered for review.
    pub(crate) recovery: Vec<RecoveryTransactionView>,
    /// The journal directory, shown for transparency.
    pub(crate) journal_dir: String,
    /// The error from the last failed attempt to persist a "Leave
    /// untouched" resolution, if any.
    pub(crate) recovery_resolution_error: Option<String>,
}

/// The review a user must confirm before any rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplyReviewView {
    /// The transaction built at review time (identity snapshots included).
    pub(crate) transaction: archivefs_core::dat::rename_apply::RenameTransaction,
    /// The exact old → new pairs, in plan order.
    pub(crate) rows: Vec<ApplyReviewRow>,
    /// The trusted root the renames must stay inside, when configured.
    pub(crate) trusted_root: Option<String>,
    /// The exact phrase that must be typed to confirm, when required
    /// (`None` for small batches where a single confirm click suffices).
    pub(crate) required_phrase: Option<String>,
}

/// One old → new pair in the review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplyReviewRow {
    pub(crate) current_basename: String,
    pub(crate) proposed_basename: String,
}

/// The per-file result of an apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplyOutcomeView {
    pub(crate) transaction_id: String,
    pub(crate) state: TransactionState,
    pub(crate) requested: usize,
    pub(crate) applied: usize,
    pub(crate) skipped: usize,
    pub(crate) failed: usize,
    pub(crate) rows: Vec<ApplyRowView>,
}

/// One per-file apply result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplyRowView {
    pub(crate) current_basename: String,
    pub(crate) proposed_basename: String,
    pub(crate) state: EntryState,
    pub(crate) failure_reason: Option<String>,
}

/// The outcome of a rollback pass, rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RollbackResultView {
    pub(crate) label: &'static str,
    pub(crate) detail: String,
}

/// One persisted transaction offered for rollback or crash recovery: either a
/// settled `Applied` batch (roll back the whole transaction) or an interrupted
/// batch (roll back completed steps or leave untouched).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveryTransactionView {
    pub(crate) transaction_id: String,
    pub(crate) state: TransactionState,
    pub(crate) applied_count: usize,
    pub(crate) total_count: usize,
    /// A human-facing summary of what this transaction renamed, e.g.
    /// `Renamed "old.zip" -> "new.zip"` (single entry) or `Renamed 12
    /// files` (many) - computed once here, at view-build time, from the
    /// same `TransactionEntry::original_basename`/`proposed_basename`
    /// already recorded in the journal (never a new persisted field).
    pub(crate) human_summary: String,
    /// The folder this transaction's plan audited
    /// ([`archivefs_core::dat::rename_apply::RenameTransaction::source_scan_root`],
    /// recorded verbatim, for provenance only). Lets a consumer (Quick
    /// Rename) tell "this transaction concerns the folder I'm working in
    /// right now" apart from "this is some other, unrelated library" -
    /// see `transaction_targets_root`.
    pub(crate) source_scan_root: String,
    /// The user's persisted decision about this transaction's
    /// crash-recovery prompt, if any - see
    /// [`archivefs_core::dat::rename_apply::RecoveryResolution`]. `None`
    /// means it still genuinely needs a decision (or was never interrupted
    /// in the first place, for a settled `Applied` transaction).
    pub(crate) resolution: Option<archivefs_core::dat::rename_apply::RecoveryResolution>,
}

// ---------------------------------------------------------------------------
// DAT matching policy view model
// ---------------------------------------------------------------------------

/// The DAT Matching Policy section: what the user has set, at the scope they
/// are editing, plus the resolved Effective Policy Summary for that scope.
///
/// The GUI never implements policy logic: every resolved value in here is the
/// output of the core resolver, and every edit action maps onto the persisted
/// [`DatPolicyConfig`] through the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatPolicyView {
    /// The scope being edited: `None` = global, otherwise a canonical
    /// platform id.
    pub(crate) scope: Option<String>,
    /// The scope's display label ("All platforms" or the platform's name).
    pub(crate) scope_label: String,
    /// The platforms a user can choose as a scope, derived from the platforms
    /// the registered sources cover.
    pub(crate) scopes_available: Vec<PolicyScopeOption>,
    pub(crate) region_preferences: Vec<PolicyPreferenceRowView>,
    pub(crate) language_preferences: Vec<PolicyPreferenceRowView>,
    pub(crate) revision_policy: RevisionPolicy,
    pub(crate) clone_policy: ClonePolicy,
    pub(crate) content_selection: ContentSelectionPolicy,
    pub(crate) effective: EffectivePolicySummaryView,
    /// Validation problems in the persisted policy (unknown values, bad
    /// platform keys). Never blocks a save; the values are preserved.
    pub(crate) problems: Vec<String>,
    /// False when the registry could not be read, so nothing can be edited.
    pub(crate) editable: bool,
}

/// One entry in an ordered preference list, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyPreferenceRowView {
    /// The stable value (`europe`, `en`, `multi`, `original`, …).
    pub(crate) value: String,
    /// What a person sees.
    pub(crate) label: String,
    /// 1-based position in the list.
    pub(crate) position: usize,
}

/// One selectable scope for the policy section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyScopeOption {
    /// `None` = "All platforms" (the global scope).
    pub(crate) id: Option<String>,
    pub(crate) label: String,
}

/// The Effective Policy Summary: the resolved policy for the current scope,
/// and where each value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectivePolicySummaryView {
    /// The resolved platform ("All platforms" or the platform's display name).
    pub(crate) platform: String,
    /// The sources consulted for this scope, in order.
    pub(crate) source_ordering: Vec<SourceOrderRow>,
    pub(crate) region: String,
    pub(crate) language: String,
    pub(crate) revision: String,
    pub(crate) clone: String,
    pub(crate) content: String,
    /// "field — where it came from" rows.
    pub(crate) source_of: Vec<(String, String)>,
}

/// One source in the summary's consultation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceOrderRow {
    pub(crate) display_name: String,
    pub(crate) priority: u32,
    /// 1-based: "consulted 1st, 2nd, …".
    pub(crate) consulted_position: usize,
}

/// One verdict category, as a countable row.
///
/// The categories are exactly the ones
/// [`archivefs_core::dat::audit::AuditSummary`] carries. None is invented and
/// none is merged: "Probable (multiple)" is not folded into "Exact (multiple)",
/// because a CRC32 agreeing is not the same evidence as a SHA-1 agreeing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuditCategoryView {
    pub(crate) label: &'static str,
    pub(crate) count: usize,
    pub(crate) meaning: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AuditResultView {
    pub(crate) source_display_name: String,
    pub(crate) source_id: String,
    pub(crate) dat_path: String,
    pub(crate) scan_root: String,
    /// The scan folder shortened for display, so a long private path does not
    /// take over the summary.
    pub(crate) scan_root_short: String,
    pub(crate) catalogue_names: Vec<String>,
    pub(crate) catalogue_entries: usize,
    pub(crate) headline: String,
    /// How long the run took, when it completed.
    pub(crate) elapsed_seconds: Option<u64>,
    pub(crate) categories: Vec<AuditCategoryView>,
    /// Per-file lines, capped for display.
    pub(crate) entries: Vec<AuditEntryView>,
    pub(crate) entries_truncated: usize,
    /// Archive-member evidence is visually separate from physical loose files.
    /// A complete single-member ZIP/7z exact match, or a package-level LHA
    /// slave match, may safely produce an outer-container proposal; member
    /// paths are never renamed directly.
    pub(crate) archives: Vec<ArchiveAuditView>,
    pub(crate) unhashed: Vec<(String, String)>,
    pub(crate) unreadable_catalogues: Vec<String>,
    pub(crate) truncated: bool,
    pub(crate) files_scanned: usize,
    pub(crate) content_selection: ContentSelectionPolicy,
    pub(crate) content_summary: DatContentSummary,
    /// The Effective Policy Summary annotation, when the audit carried a
    /// policy. Never changes a verdict; it shows the preferred candidate
    /// order for the files whose hash matched several catalogue entries.
    pub(crate) policy: Option<AuditPolicyView>,
    /// How much of this exact DAT/source snapshot the audit found locally,
    /// or `None` when a completion claim would not have a trustworthy basis
    /// (see [`dat_completion_view`]'s doc).
    pub(crate) completion: Option<DatCompletionView>,
}

/// How much of one selected DAT/source's catalogue the last audit verified
/// locally, and what that means in plain language.
///
/// "Verified" and "total" are never re-derived from a rescan: both come
/// straight from the already-loaded [`DatAuditOutcome`] a completed audit
/// produced - `total` from the catalogue's own declared entry count, and
/// `verified` from counting the *distinct* catalogue games this audit's
/// already-computed verdicts named with a confident, unambiguous
/// cryptographic match. Building this view is one pass over data the audit
/// already holds in memory; nothing here reads a file or a DAT again.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DatCompletionView {
    /// Distinct catalogue games this audit found unambiguous evidence for.
    pub(crate) verified: usize,
    /// The catalogue's own declared entry (game) count.
    pub(crate) total: usize,
    /// `verified / total`, as a percentage, rounded to two decimal places.
    /// `None` exactly when `state` is `Unknown` - a fabricated percentage
    /// over an untrustworthy total is worse than no percentage at all.
    pub(crate) percent: Option<f64>,
    pub(crate) state: DatCompletionState,
    /// `total - verified`, or `None` when `state` is `Unknown` and there is
    /// nothing safe to subtract from. Never derived from unmatched *local*
    /// files - see `extra_local_files`.
    pub(crate) missing: Option<usize>,
    /// Local files this audit compared that matched no catalogue entry at
    /// all (`AuditVerdict::NotInDat`). A distinct concept from `missing`:
    /// a library can have both catalogue entries with no local file
    /// (missing) and local files with no catalogue entry (extra) at once,
    /// and neither is derived from the other.
    pub(crate) extra_local_files: usize,
    /// The catalogue title(s) this completion is measured against - the
    /// same names already shown as "Catalogue: ...".
    pub(crate) source_title: String,
    /// The provider name, when the DAT header carries one EmuWiz recognises
    /// (its `<author>` or `<homepage>` text) - never guessed from the
    /// source's display name or file path.
    pub(crate) provider: Option<String>,
    /// The DAT header's own `<version>` text - the closest thing most
    /// publishers have to a revision/snapshot identifier. `None` when the
    /// header did not carry one, or (for a combined multi-source audit)
    /// when there is no single header to report.
    pub(crate) revision: Option<String>,
    /// True only when `state` is `Complete` and the provider looks like
    /// No-Intro - see this view's constructor for why this is the one
    /// provider-specific claim made here.
    pub(crate) no_intro_complete_badge: bool,
    /// Set when something about this run means the numbers above might
    /// understate (never overstate) how complete the collection actually
    /// is - a capped scan, or a catalogue file that failed to parse.
    pub(crate) caveat: Option<String>,
}

/// Deterministic completion states, ordered from best to worst. `Complete`
/// is the least reachable, matching `dat::set::SetState`'s own "Complete is
/// the least reachable" convention for the same reason: it is the one claim
/// that must never be produced by rounding or by a partial view of the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatCompletionState {
    /// `verified == total`, `total > 0`.
    Complete,
    /// `>= 95% and < 100%`.
    NearlyComplete,
    /// `> 0% and < 95%`.
    Incomplete,
    /// `verified == 0`, `total > 0`.
    NoneVerified,
    /// `total` is unavailable or not trustworthy as a denominator (for
    /// example a combined multi-source audit, or a catalogue that reported
    /// zero entries).
    Unknown,
}

impl DatCompletionState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Complete => "Complete",
            Self::NearlyComplete => "Nearly complete",
            Self::Incomplete => "Incomplete",
            Self::NoneVerified => "None verified",
            Self::Unknown => "Not enough information",
        }
    }

    pub(crate) fn tone(self) -> widgets::StatusTone {
        match self {
            Self::Complete => widgets::StatusTone::Success,
            Self::NearlyComplete => widgets::StatusTone::Warning,
            Self::Incomplete | Self::NoneVerified => widgets::StatusTone::Blocked,
            Self::Unknown => widgets::StatusTone::Pending,
        }
    }
}

/// Whether an outcome is the multi-source "audit everything enabled at once"
/// pass rather than one selected DAT/source. Mirrors the literal id
/// `run_combined_dat_audit_with_cache` constructs
/// (`crates/archivefs-core/src/dat/sources/audit_run.rs`); a combined pass
/// has no single catalogue snapshot, so it never gets a completion claim -
/// see [`dat_completion_view`].
fn is_combined_audit(outcome: &DatAuditOutcome) -> bool {
    outcome.source_id == "combined-enabled-dat-sources"
}

/// Builds the completion view for one audit outcome, or `None` when a
/// completion claim would not have a trustworthy basis: a combined
/// multi-source audit (no single selected DAT), or a catalogue that
/// declared zero entries (nothing to measure against).
///
/// # Why `verified` counts distinct games, not matched files
///
/// A library can hold more than one copy of the same ROM; counting matched
/// *files* would let duplicates inflate completion past what the catalogue
/// actually has. Counting distinct `game_name`s from
/// [`AuditVerdict::Exact`] verdicts avoids that, and matches this DAT
/// model's existing unit of "one catalogue set is one `<game>`" (see
/// `dat::set`'s module doc).
///
/// `AuditVerdict::ExactMultipleCandidates` is deliberately excluded from
/// this count even though [`AuditVerdict::is_confident`] treats it as
/// cryptographic evidence: it names several *candidate* games, and crediting
/// all of them (or guessing one) would overclaim which specific catalogue
/// entry is actually verified. A file audited this way still counts toward
/// `files_scanned`/the category breakdown elsewhere on the page; it is just
/// never presented as proof that one particular game is present.
///
/// # Why `verified` cannot exceed `total`
///
/// Ordinarily distinct-games-matched cannot exceed the catalogue's own
/// entry count, but a folder source can merge several DAT files whose game
/// names collide, or a DAT can declare an entry count that undercounts its
/// own `<game>` elements. Either way this clamps `verified` to `total`
/// rather than ever showing a percentage above 100%, and remembers that it
/// did so as a caveat rather than silently hiding the discrepancy.
fn dat_completion_view(outcome: &DatAuditOutcome) -> Option<DatCompletionView> {
    // A combined multi-source pass has no single selected DAT/snapshot to
    // measure "100%" against at all - not even an "Unknown" claim about one
    // - so this is the one case with no completion view whatsoever.
    if is_combined_audit(outcome) {
        return None;
    }

    let total = outcome.catalogue_entries;
    let extra_local_files = outcome.report.summary.not_in_dat;

    // A catalogue that declared zero entries has no trustworthy denominator:
    // showing a percentage or a missing count here would be fabricated,
    // never derived. This is the "Unknown/Not checked" state, not absence
    // of the widget - the page still names the source and says plainly that
    // there is not enough information.
    if total == 0 {
        return Some(DatCompletionView {
            verified: 0,
            total: 0,
            percent: None,
            state: DatCompletionState::Unknown,
            missing: None,
            extra_local_files,
            source_title: outcome.catalogue_names.join(", "),
            provider: provider_from_outcome(outcome),
            revision: revision_from_outcome(outcome),
            no_intro_complete_badge: false,
            caveat: Some(
                "This catalogue declared no entries, so completion cannot be measured against \
                 it."
                .to_string(),
            ),
        });
    }

    let mut confident_games: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for entry in &outcome.report.entries {
        if let archivefs_core::dat::audit::AuditVerdict::Exact { game_name, .. } = &entry.verdict {
            confident_games.insert(game_name.as_str());
        }
    }
    let raw_verified = confident_games.len();
    let verified = raw_verified.min(total);
    let clamped = raw_verified > total;

    let missing = total.saturating_sub(verified);
    let percent = (verified as f64 / total as f64) * 100.0;
    let rounded_percent = (percent * 100.0).round() / 100.0;

    let state = if verified == total {
        DatCompletionState::Complete
    } else if rounded_percent >= 95.0 {
        DatCompletionState::NearlyComplete
    } else if verified > 0 {
        DatCompletionState::Incomplete
    } else {
        DatCompletionState::NoneVerified
    };

    let provider = provider_from_outcome(outcome);
    let revision = revision_from_outcome(outcome);
    let no_intro_complete_badge =
        looks_like_no_intro(outcome) && state == DatCompletionState::Complete;

    let mut caveat_parts = Vec::new();
    if outcome.truncated {
        caveat_parts.push(
            "The local scan stopped at a safety limit, so more matching files may exist than \
             this run saw; completion may understate the true total, never overstate it."
                .to_string(),
        );
    }
    if !outcome.unreadable_catalogues.is_empty() {
        caveat_parts.push(
            "Part of this source's catalogue could not be read, so the total entry count may be \
             lower than the source's real catalogue."
                .to_string(),
        );
    }
    if clamped {
        caveat_parts.push(format!(
            "{raw_verified} distinct catalogue games were matched, more than the catalogue's own \
             {total}-entry count; the percentage below is capped at 100%."
        ));
    }
    let caveat = if caveat_parts.is_empty() {
        None
    } else {
        Some(caveat_parts.join(" "))
    };

    Some(DatCompletionView {
        verified,
        total,
        percent: Some(rounded_percent.min(100.0)),
        state,
        missing: Some(missing),
        extra_local_files,
        source_title: outcome.catalogue_names.join(", "),
        provider,
        revision,
        no_intro_complete_badge,
        caveat,
    })
}

/// The provider name.
///
/// Prefers the already-detected [`archivefs_core::dat::model::DatEcosystem`]
/// - parse-time classification this crate already trusts, not a new text
/// heuristic - when it names something specific. The `Generic*` variants
/// mean the parser could not confirm a specific ecosystem, so those fall
/// through to the header's own `<author>`, then `<homepage>` text (several
/// No-Intro DATs put the provider name itself there rather than a URL).
/// `None` when nothing above is available - never guessed from the source's
/// display name or file path.
fn provider_from_outcome(outcome: &DatAuditOutcome) -> Option<String> {
    use archivefs_core::dat::model::DatEcosystem;
    match outcome.catalogue_ecosystem {
        Some(DatEcosystem::GenericLogiqx | DatEcosystem::GenericClrMamePro) | None => {
            provider_from_header_text(outcome)
        }
        Some(ecosystem) => Some(ecosystem.label().to_string()),
    }
}

fn provider_from_header_text(outcome: &DatAuditOutcome) -> Option<String> {
    outcome
        .catalogue_author
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(outcome
            .catalogue_homepage
            .as_deref()
            .filter(|value| !value.trim().is_empty()))
        .map(str::to_string)
}

/// The DAT header's own `<version>` text, when non-empty.
fn revision_from_outcome(outcome: &DatAuditOutcome) -> Option<String> {
    outcome
        .catalogue_version
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

/// Whether this outcome's detected ecosystem is specifically No-Intro - the
/// one provider this view is allowed to badge specifically (see
/// [`DatCompletionView::no_intro_complete_badge`]'s doc for why). Reads the
/// same already-classified [`archivefs_core::dat::model::DatEcosystem`]
/// `provider_from_outcome` does, never a text guess.
fn looks_like_no_intro(outcome: &DatAuditOutcome) -> bool {
    matches!(
        outcome.catalogue_ecosystem,
        Some(archivefs_core::dat::model::DatEcosystem::NoIntro)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveAuditView {
    pub(crate) archive_name: String,
    pub(crate) completion: String,
    pub(crate) members: Vec<ArchiveMemberAuditView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveMemberAuditView {
    pub(crate) index: usize,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) verdict: Option<String>,
    pub(crate) detail: String,
    pub(crate) evidence_sources: Vec<String>,
}

/// The policy annotation of one audit result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuditPolicyView {
    /// The sources consulted for this platform, in order.
    pub(crate) source_ordering: Vec<String>,
    /// One row per multi-candidate file that was ranked.
    pub(crate) notes: Vec<AuditPolicyNoteView>,
    /// `None` when no file needed a ranking.
    pub(crate) notes_truncated: Option<usize>,
}

/// One ranked multi-candidate file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuditPolicyNoteView {
    pub(crate) file_name: String,
    pub(crate) verdict_label: String,
    /// The ranked candidates, in the deterministic display order.
    pub(crate) ranked: Vec<String>,
    /// The explanations, most decisive first.
    pub(crate) explanations: Vec<String>,
    /// Whether the policy picked a single winner, and its label.
    pub(crate) decided: bool,
    pub(crate) winner: Option<String>,
    /// Whether the policy could not decide, and why.
    pub(crate) ambiguous: bool,
    pub(crate) ambiguity_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuditEntryView {
    pub(crate) file_name: String,
    pub(crate) verdict: &'static str,
    pub(crate) detail: String,
    /// Every enabled catalogue that supplied the agreeing exact match. This
    /// stays empty for ordinary one-source audits and non-actionable rows.
    pub(crate) evidence_sources: Vec<String>,
    pub(crate) content: Vec<ContentTechnicalView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContentTechnicalView {
    pub(crate) classification: String,
    pub(crate) confidence: String,
    pub(crate) evidence: Vec<String>,
    pub(crate) original_metadata: Vec<(String, String)>,
    pub(crate) classifier_version: String,
}

/// How many audited files are listed individually.
///
/// The summary counts every file; the list is what a person reads, and 500
/// lines is already past the point of reading. The view says how many were
/// left out rather than implying the list is complete.
pub(crate) const MAX_AUDIT_ENTRIES_SHOWN: usize = 500;

/// How many rename-plan rows are drawn on one page. A verified library the
/// size of the proven Game Boy run (1839 actionable entries) must stay
/// scrollable and responsive; rendering all of them into one `ScrollArea`
/// at once is what this bounds against. Selection is unaffected by paging:
/// it is keyed by source path on [`DatSourcesPageState`], not by what is on
/// screen.
pub(crate) const RENAME_PLAN_PAGE_SIZE: usize = 150;

/// The `[start, end)` slice bounds and total page count for page `page`
/// (0-based, clamped into range) over `total` rows at
/// [`RENAME_PLAN_PAGE_SIZE`] rows per page. Pulled out of the drawing
/// function so pagination math is unit-testable without an egui context -
/// an egui `ScrollArea` only *paints* the rows that fit in its visible
/// height regardless of how many are handed to it, so a rendered-text
/// assertion cannot see far-down rows even when they are correctly
/// included in the page.
pub(crate) fn rename_plan_page_bounds(total: usize, page: usize) -> (usize, usize, usize) {
    let page_count = total.div_ceil(RENAME_PLAN_PAGE_SIZE).max(1);
    let page = page.min(page_count - 1);
    let start = page * RENAME_PLAN_PAGE_SIZE;
    let end = (start + RENAME_PLAN_PAGE_SIZE).min(total);
    (start, end, page_count)
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// One thing the page can ask for. Only `Save` writes the registry; only
/// `Validate` and `Audit` read anything else.
///
/// The policy actions all carry the scope they apply to (`None` = global,
/// otherwise a canonical platform id), so an edit made while inspecting one
/// platform's effective policy lands on that platform's override, never on
/// the global preferences by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DatSourcesPageAction {
    AddFile {
        path: PathBuf,
    },
    /// A deliberately narrow local import for the public Retroplay-derived
    /// WHDLoad catalogue.  It remains a user-selected local DAT: no Retroplay
    /// package infrastructure is downloaded or scraped by EmuWiz.
    AddWHDLoadDat {
        path: PathBuf,
    },
    OpenNoIntroDownloadPage,
    ChooseNoIntroPack {
        path: PathBuf,
    },
    InspectNoIntroPack,
    ImportNoIntroPack,
    AddFolder {
        path: PathBuf,
    },
    SetEnabled {
        id: String,
        enabled: bool,
    },
    SetPlatform {
        id: String,
        platform: Option<String>,
    },
    Remove {
        id: String,
    },
    Validate {
        id: String,
    },
    /// Validates every currently configured source (enabled or not - the
    /// per-source Validate button already does not require a source to be
    /// enabled, so this does not either), sequentially, on the same
    /// [`validate_dat_source`] path a single Validate uses. A no-op while a
    /// job is already running.
    ValidateAll,
    Audit {
        id: String,
        scan_root: PathBuf,
    },
    /// Read-only all-evidence audit for the normal Identify & Rename flow.
    /// The state gathers only enabled local DATs and validated installed game
    /// snapshots; BIOS metadata is never included.
    AuditAllEnabled {
        scan_root: PathBuf,
    },
    OpenDatSources,
    OpenAdvancedIdentifyRename,
    /// Adds one explicitly typed MAME software-list source. The authoritative
    /// name is validated by core; no remote endpoint is configurable here.
    AddManagedMameSoftwareList {
        authoritative_name: String,
    },
    /// Removes only the managed-source configuration. Downloaded state and
    /// immutable objects are deliberately retained by core for later cleanup.
    RemoveManagedMameSoftwareList {
        authoritative_name: String,
    },
    AddManagedRedumpBios {
        system: RedumpBiosSystem,
    },
    RemoveManagedRedumpBios {
        system: RedumpBiosSystem,
    },
    AddManagedRedumpGames {
        system: RedumpGameSystem,
    },
    RemoveManagedRedumpGames {
        system: RedumpGameSystem,
    },
    CheckManagedDat {
        source_id: ManagedDatSourceId,
    },
    UpdateManagedDat {
        source_id: ManagedDatSourceId,
    },
    ImportTosecReleasePack {
        root: PathBuf,
    },
    RemoveTosecReleasePack {
        pack_id: String,
    },
    SetTosecSelection {
        pack_id: String,
        key: TosecSelectionKey,
        enabled: bool,
    },
    ApplyTosecSelection {
        pack_id: String,
    },
    CancelJob,
    Save,
    Revert,
    SelectPolicyScope {
        scope: Option<String>,
    },
    MoveRegion {
        scope: Option<String>,
        index: usize,
        delta: i32,
    },
    AddRegion {
        scope: Option<String>,
        region: RegionId,
    },
    RemoveRegion {
        scope: Option<String>,
        index: usize,
    },
    MoveLanguage {
        scope: Option<String>,
        index: usize,
        delta: i32,
    },
    AddLanguage {
        scope: Option<String>,
        preference: LanguagePreference,
    },
    RemoveLanguage {
        scope: Option<String>,
        index: usize,
    },
    ClearRegion {
        scope: Option<String>,
    },
    ClearLanguage {
        scope: Option<String>,
    },
    SetRevisionPolicy {
        scope: Option<String>,
        policy: RevisionPolicy,
    },
    SetClonePolicy {
        scope: Option<String>,
        policy: ClonePolicy,
    },
    SetContentSelection {
        scope: Option<String>,
        policy: ContentSelectionPolicy,
    },
    /// Records or clears a review decision on one proposal. Decisions are
    /// session-only and never touch a file.
    SetReviewDecision {
        path: String,
        decision: Option<ReviewDecision>,
    },
    /// Clears every review decision for the current plan.
    ClearReviewDecisions,
    /// Accepts every actionable (`Suggested`) proposal in the current plan
    /// for review, regardless of the active display filter or which page a
    /// row is currently rendered on. Never touches a non-actionable row
    /// (unmatched, ambiguous, unsupported, conflicting, or blocked stay
    /// unselected) - session-only, like every other review decision.
    SelectAllActionable,
    /// Build the transaction for the approved, applicable proposals and show
    /// the read-only review. No mutation.
    BeginApplyReview,
    /// Quick Rename's one-click "prepare to rename": [`Self::SelectAllActionable`]
    /// immediately followed by [`Self::BeginApplyReview`]. Exists so the
    /// simple workflow never makes a user click Select, wait for a
    /// re-render, then click Rename - it is exactly those two existing
    /// steps, not a new one.
    QuickRenamePrepareApply,
    /// Confirm the review and run the apply in AbortAll mode. `typed` is the
    /// user's typed confirmation phrase, validated before any mutation.
    ConfirmApply {
        typed: String,
    },
    /// Re-run the apply applying only the independently safe subset, after an
    /// AbortAll conflict was shown and the user explicitly chose it.
    ConfirmApplySafeSubset {
        typed: String,
    },
    CancelApplyReview,
    /// Roll back the last applied (or recovered) transaction.
    RollbackTransaction {
        id: String,
    },
    /// A crash-recovery choice for an interrupted transaction.
    RecoveryChoice {
        id: String,
        choice: RecoveryChoice,
    },
    /// Forget the apply outcome display.
    ClearApplyOutcome,
    /// Quick Rename's "Rename another library" / "Rename N another
    /// library" reset: clears this session's chosen folder, scan result,
    /// plan, and apply outcome so the page returns to its initial "Choose
    /// library or folder…" state. Never touches DAT source configuration,
    /// the transaction journal directory, or `recovery_transactions`/
    /// `dismissed_recovery_ids` - those are library-wide state, not part of
    /// "this Quick Rename session".
    ResetQuickRenameSession,
    /// Session-only dismissal of every currently-settled (`Applied`)
    /// recovery transaction from Quick Rename's collapsed history list.
    /// Never deletes a journal - see `dismissed_recovery_ids`'s doc for why
    /// that would not be honest for an unresolved transaction, though a
    /// settled one *is* already safely removable via Repair History's
    /// existing "Clear completed history" (`remove_journal`), which this
    /// deliberately does not duplicate.
    HideSettledRecoveryHistory,
}

/// A crash-recovery choice, mirroring the journal recovery options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryChoice {
    RollBack,
    LeaveUntouched,
}

// ---------------------------------------------------------------------------
// Background work
// ---------------------------------------------------------------------------

enum JobMessage {
    Progress(String),
    /// Structured audit progress, kept structured so the page can compute
    /// percentages and an ETA instead of only echoing text.
    AuditProgress(DatAuditProgress),
    /// One source's validation report - a single Validate job's only result,
    /// or one of many in a `ValidateAll` job's sequential run. Which it is is
    /// decided entirely by `RunningJob::kind`; the report and the update it
    /// drives are identical either way; see `validate_dat_source`, the one
    /// path both use.
    Validated(Box<DatValidationReport>),
    /// `ValidateAll` only: sent immediately before each source starts, so the
    /// page can show which source is currently being read and mark its row
    /// busy - the same way `RunningJob::source_id` already does for a single
    /// Validate/Audit job.
    ValidatingNext {
        id: String,
        display_name: String,
    },
    /// `ValidateAll` only: every source has either been validated or (only
    /// when cancelled) skipped. The final tally already lives on
    /// `RunningJob::bulk`; this just marks the run over.
    ValidateAllFinished,
    Audited {
        /// The audit generation this result belongs to. A result from a stale
        /// generation is discarded so an older plan can never replace a newer
        /// one.
        generation: u64,
        outcome: Box<DatAuditOutcome>,
        enrichment: Option<Box<archivefs_core::PlatformIdentityEnrichmentSummary>>,
        /// The read-only rename plan derived from the audit, when one could be
        /// built (and was not cancelled).
        plan: Option<Box<RenamePlan>>,
    },
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Validate,
    ValidateAll,
    Audit,
}

/// `ValidateAll`-only running tally, carried on the job so the final summary
/// is exact even if the page never sees `ValidateAllFinished` (a panicked
/// worker still leaves whatever was already committed intact and visible).
#[derive(Debug, Clone, Copy)]
struct BulkValidationProgress {
    total: usize,
    completed: usize,
    valid: usize,
    changed: usize,
    failed: usize,
}

impl BulkValidationProgress {
    fn summary(self) -> ValidateAllSummary {
        ValidateAllSummary {
            total: self.total,
            valid: self.valid,
            changed: self.changed,
            failed: self.failed,
            skipped: self.total.saturating_sub(self.completed),
        }
    }
}

/// The final tally a "Validate all" run leaves for display, until the next
/// run replaces it or a Revert discards it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ValidateAllSummary {
    pub(crate) total: usize,
    pub(crate) valid: usize,
    pub(crate) changed: usize,
    pub(crate) failed: usize,
    /// Never validated because the run was cancelled before reaching them.
    /// Always `0` for a run that finished on its own.
    pub(crate) skipped: usize,
}

/// One source's outcome inside a "Validate all" run, classified from the
/// exact [`DatValidationReport`]/[`DatSourceHealth`] a single Validate
/// already produces and stores - no separate judgment call invented for the
/// bulk case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidateAllOutcome {
    /// Parsed cleanly (or with warnings) and the recorded health did not
    /// change from what was already on file.
    Valid,
    /// Parsed cleanly (or with warnings), but the recorded health differs
    /// from what was on file before this run - including a source's very
    /// first validation, where "on file" was empty.
    Changed,
    /// The path is unreadable, or at least one registered DAT failed to
    /// parse.
    Failed,
}

fn classify_validate_all_outcome(
    state: DatHealthState,
    previous_health: Option<&DatSourceHealth>,
    new_health: &DatSourceHealth,
) -> ValidateAllOutcome {
    if matches!(state, DatHealthState::Invalid | DatHealthState::Unreadable) {
        return ValidateAllOutcome::Failed;
    }
    // `last_validated_unix_seconds` always differs (this run just set it), so
    // it is excluded from the comparison - otherwise every run would report
    // "changed" regardless of whether anything the user cares about moved.
    let mut new_without_timestamp = new_health.clone();
    new_without_timestamp.last_validated_unix_seconds = None;
    let mut previous_without_timestamp = previous_health.cloned().unwrap_or_default();
    previous_without_timestamp.last_validated_unix_seconds = None;
    if previous_without_timestamp == new_without_timestamp {
        ValidateAllOutcome::Valid
    } else {
        ValidateAllOutcome::Changed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedDatOperation {
    Check,
    Update,
}

fn managed_dat_action(
    source_id: ManagedDatSourceId,
    operation: ManagedDatOperation,
) -> DatSourcesPageAction {
    match operation {
        ManagedDatOperation::Check => DatSourcesPageAction::CheckManagedDat { source_id },
        ManagedDatOperation::Update => DatSourcesPageAction::UpdateManagedDat { source_id },
    }
}

struct ManagedDatJobMessage {
    source_id: ManagedDatSourceId,
    result: archivefs_core::Result<ManagedDatUpdateOutcome>,
}

/// A separate serial worker for explicit managed-DAT network operations.
/// Unlike local validation/audit jobs it is not cancellable: the core updater
/// has no cancellation contract, so presenting a cancel button would lie.
struct RunningManagedDatJob {
    source_id: ManagedDatSourceId,
    messages: Receiver<ManagedDatJobMessage>,
}

/// A rename apply or rollback worker result.
enum ApplyJobMessage {
    Applied(Box<ApplyOutcome>),
    RolledBack(Box<RollbackOutcome>),
    Failed(String),
    /// An AbortAll apply found hard conflicts; nothing was mutated and the
    /// safe-subset option is now offered.
    HardConflicts(String),
    Cancelled,
}

/// The dedicated worker for apply and rollback mutations. The GUI never calls
/// `std::fs::rename`; it sends the core executor's request here and renders the
/// result.
struct ApplyJob {
    cancel: Arc<AtomicBool>,
    messages: Receiver<ApplyJobMessage>,
}

/// A History & Logs record produced by an apply or rollback outcome. It carries
/// counts and the transaction id but never private paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameHistoryRecord {
    pub(crate) action: RenameHistoryAction,
    pub(crate) transaction_id: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenameHistoryAction {
    Apply,
    Rollback,
}

struct RunningJob {
    kind: JobKind,
    source_id: String,
    cancel: Arc<AtomicBool>,
    /// Set by [`DatSourcesPageAction::CancelJob`]. The visible card switches
    /// to "Stopping…" immediately; the job itself keeps running until the
    /// worker sends a terminal message, and anything that arrives afterwards
    /// is ignored rather than allowed to restore an active-looking state.
    cancel_requested: bool,
    messages: Receiver<JobMessage>,
    latest: String,
    /// When the job started, for elapsed time.
    started_at: Instant,
    /// Structured progress for audit jobs. `None` for validation.
    audit_progress: Option<AuditProgressTracker>,
    /// The source's resolved platform at job start, shown on the running card
    /// only when it is authoritative (assigned and recognised).
    platform_display: Option<String>,
    /// `Some` only for `JobKind::ValidateAll`: the running tally across the
    /// whole batch. `None` for every other kind.
    bulk: Option<BulkValidationProgress>,
}

/// Sends without blocking, dropping the message when the queue is full.
///
/// The worker must never wait on the UI: a stalled window would otherwise stall
/// the audit, and the only thing lost by dropping is a progress line the next
/// one replaces.
fn send_progress(sender: &SyncSender<JobMessage>, message: JobMessage) {
    match sender.try_send(message) {
        Ok(()) | Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {}
    }
}

/// The percentage, as a whole number, or `None` when the total is unknown or
/// zero. A total that is not known is never replaced by a guessed one.
fn format_percentage(checked: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    let percent = ((checked as f64 / total as f64) * 100.0).round() as i64;
    Some(percent.clamp(0, 100) as u8)
}

/// Seconds as a person would read an elapsed time: "42s", "3m 12s", "1h 5m".
fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// Remaining seconds as an approximate ETA: "About 12 minutes remaining".
fn format_eta_remaining(seconds: u64) -> String {
    if seconds < 60 {
        format!("About {} seconds remaining", seconds.max(1))
    } else if seconds < 3600 {
        let minutes = ((seconds + 30) / 60).max(1);
        format!(
            "About {minutes} {} remaining",
            if minutes == 1 { "minute" } else { "minutes" }
        )
    } else {
        let hours = ((seconds + 1800) / 3600).max(1);
        format!(
            "About {hours} {} remaining",
            if hours == 1 { "hour" } else { "hours" }
        )
    }
}

/// A path shortened for display: the last two components, with the private
/// leading part elided.
///
/// The user picked the folder, so showing part of it on the card is fine; the
/// point is that a long absolute path never takes over the running card, and
/// that a full private path is never turned into a display string. Short paths
/// are returned as they are.
fn shorten_path(path: &str) -> String {
    let mut components: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    if components.len() <= 2 {
        return path.to_string();
    }
    let kept: Vec<&str> = components.split_off(components.len() - 2);
    format!("…/{}", kept.join("/"))
}

/// The source's platform, shown on the running card only when it is
/// authoritative: assigned by the user and recognised by this build. An
/// unassigned source has no platform to claim, and an unresolved one is not
/// presented as if it were real.
fn authoritative_platform(entry: &DatSourceEntry) -> Option<String> {
    entry
        .platform
        .as_ref()
        .filter(|_| entry.platform_is_resolved())
        .and_then(|_| entry.platform_display())
}

/// The region preference list as *authored* at `scope`: the global list, or a
/// platform override's own list (empty when that platform has no override).
fn authored_region_list(config: &DatPolicyConfig, scope: &Option<String>) -> Vec<String> {
    match scope {
        None => config.region_preferences.clone().unwrap_or_default(),
        Some(platform) => config
            .platforms
            .as_ref()
            .and_then(|overrides| overrides.get(platform))
            .and_then(|entry| entry.region_preferences.clone())
            .unwrap_or_default(),
    }
}

/// The language preference list as *authored* at `scope`.
fn authored_language_list(config: &DatPolicyConfig, scope: &Option<String>) -> Vec<String> {
    match scope {
        None => config.language_preferences.clone().unwrap_or_default(),
        Some(platform) => config
            .platforms
            .as_ref()
            .and_then(|overrides| overrides.get(platform))
            .and_then(|entry| entry.language_preferences.clone())
            .unwrap_or_default(),
    }
}

/// A preference list rendered as one line for the summary, or "Any" when the
/// list is empty (an empty preference list means no preference - all regions
/// or languages are treated as equal).
fn render_preference_list(values: Vec<String>) -> String {
    if values.is_empty() {
        "Any".to_string()
    } else {
        values.join(", ")
    }
}

/// The Effective Policy Summary for one scope, derived entirely from the core
/// resolver's output - the GUI renders, it never re-resolves.
fn effective_summary_view(
    effective: &EffectiveDatPolicy,
    scope: &Option<String>,
) -> EffectivePolicySummaryView {
    let platform = match scope {
        None => "All platforms".to_string(),
        Some(id) => archivefs_core::platform::display_name_for(id).to_string(),
    };
    let source_ordering = effective
        .source_ordering
        .iter()
        .enumerate()
        .map(|(index, source)| SourceOrderRow {
            display_name: source.display_name.clone(),
            priority: source.priority,
            consulted_position: index + 1,
        })
        .collect();
    let region = render_preference_list(
        effective
            .region_preferences
            .iter()
            .map(|region| region.label().to_string())
            .collect(),
    );
    let language = render_preference_list(
        effective
            .language_preferences
            .iter()
            .map(|preference| preference.label().to_string())
            .collect(),
    );
    let source_of = vec![
        (
            "Region preference".to_string(),
            effective.scope_of[&PolicyField::Region].label().to_string(),
        ),
        (
            "Language preference".to_string(),
            effective.scope_of[&PolicyField::Language]
                .label()
                .to_string(),
        ),
        (
            "Revision policy".to_string(),
            effective.scope_of[&PolicyField::Revision]
                .label()
                .to_string(),
        ),
        (
            "Clone policy".to_string(),
            effective.scope_of[&PolicyField::Clone].label().to_string(),
        ),
        (
            "Content selection".to_string(),
            effective.scope_of[&PolicyField::Content]
                .label()
                .to_string(),
        ),
    ];
    EffectivePolicySummaryView {
        platform,
        source_ordering,
        region,
        language,
        revision: effective.revision_policy.label().to_string(),
        clone: effective.clone_policy.label().to_string(),
        content: effective.content_selection.label().to_string(),
        source_of,
    }
}

/// Groups the last validation's diagnostics by (severity, code, message),
/// accumulating per-group occurrence and affected-file counts.
///
/// The result is sorted by severity (Error, Warning, Note) then code then
/// message, so two runs over the same folder produce identical output. Occurrence
/// rows are kept bounded; the message string is cloned once per distinct type,
/// never once per occurrence. `source_id` scopes each group's stable id so the
/// open-group state can never bleed across sources.
fn group_diagnostics(source_id: &str, report: &DatValidationReport) -> Vec<DiagnosticGroupView> {
    use std::collections::{BTreeMap, BTreeSet};

    fn severity_rank(severity: DiagnosticSeverity) -> u8 {
        match severity {
            DiagnosticSeverity::Error => 0,
            DiagnosticSeverity::Warning => 1,
            DiagnosticSeverity::Note => 2,
        }
    }

    struct GroupAcc {
        severity: DiagnosticSeverity,
        occurrence_count: usize,
        affected_files: BTreeSet<String>,
        occurrences: Vec<DiagnosticOccurrenceView>,
        occurrences_truncated: bool,
    }

    // The key is ordered so the BTreeMap yields severity- then code- then
    // message-sorted groups.
    let mut groups: BTreeMap<(u8, &'static str, String), GroupAcc> = BTreeMap::new();

    for file in &report.files {
        let DatFileOutcome::Parsed { diagnostics, .. } = &file.outcome else {
            continue;
        };
        for diagnostic in diagnostics {
            let key = (
                severity_rank(diagnostic.severity),
                diagnostic.code,
                diagnostic.message.clone(),
            );
            let acc = groups.entry(key).or_insert_with(|| GroupAcc {
                severity: diagnostic.severity,
                occurrence_count: 0,
                affected_files: BTreeSet::new(),
                occurrences: Vec::new(),
                occurrences_truncated: false,
            });
            acc.occurrence_count += 1;
            acc.affected_files.insert(file.file_name.clone());
            if acc.occurrences.len() < MAX_DIAGNOSTIC_OCCURRENCES_SHOWN {
                acc.occurrences.push(DiagnosticOccurrenceView {
                    file_name: file.file_name.clone(),
                    line: diagnostic.line,
                    column: diagnostic.column,
                });
            } else {
                acc.occurrences_truncated = true;
            }
        }
    }

    groups
        .into_iter()
        .map(|((_, code, message), acc)| DiagnosticGroupView {
            severity: acc.severity,
            code,
            message: message.clone(),
            occurrence_count: acc.occurrence_count,
            affected_file_count: acc.affected_files.len(),
            occurrences: acc.occurrences,
            occurrences_truncated: acc.occurrences_truncated,
            // The id is scoped by source and severity so expanding a group on
            // one source can never leave another source's same-typed group
            // open, and a code reused at two severities cannot collide.
            id: format!(
                "{source_id}:{}:{code}:{message}",
                severity_rank(acc.severity)
            ),
        })
        .collect()
}

/// How far a running audit has got, structurally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditPhase {
    ReadingCatalogue,
    Scanning,
    Hashing,
    Comparing,
}

impl AuditPhase {
    fn label(self) -> &'static str {
        match self {
            Self::ReadingCatalogue => "Reading catalogue",
            Self::Scanning => "Scanning",
            Self::Hashing => "Checking files",
            Self::Comparing => "Comparing",
        }
    }
}

/// The GUI-side record of a running audit's progress.
///
/// Kept off the view so the drawing function only draws. The ETA's smoothing
/// lives here because it is state across frames; the pure formatting helpers
/// stay free functions so tests can drive them without a clock. The ETA view
/// is computed at update time and cached, so a frozen run (a stall, or a run
/// being cancelled) shows the estimate it had at its last update rather than
/// one that keeps drifting as the wall clock moves.
#[derive(Debug)]
struct AuditProgressTracker {
    phase: AuditPhase,
    files_checked: u64,
    total_files: Option<u64>,
    current_path: Option<String>,
    eta: EtaEstimator,
    eta_view: EtaView,
}

impl AuditProgressTracker {
    fn new() -> Self {
        Self {
            phase: AuditPhase::ReadingCatalogue,
            files_checked: 0,
            total_files: None,
            current_path: None,
            eta: EtaEstimator::new(),
            eta_view: EtaView::None,
        }
    }

    /// Feeds one progress event. `elapsed_seconds` is the time since the run
    /// started, read by the caller so tests can supply a controlled clock.
    fn update(&mut self, event: &DatAuditProgress, elapsed_seconds: f64) {
        match event {
            DatAuditProgress::ReadingCatalogue { .. } => {
                self.phase = AuditPhase::ReadingCatalogue;
                self.files_checked = 0;
                self.total_files = None;
                self.current_path = None;
                self.eta_view = EtaView::None;
            }
            DatAuditProgress::CatalogueReady { .. } => {
                // Between phases; nothing new about files.
            }
            DatAuditProgress::Scanning {
                files_found,
                current_dir,
            } => {
                self.phase = AuditPhase::Scanning;
                self.files_checked = *files_found as u64;
                // The discovery phase does not know the total yet; an ETA is
                // impossible and none is invented.
                self.total_files = None;
                self.current_path = current_dir.clone();
                self.eta = EtaEstimator::new();
                self.eta_view = EtaView::None;
            }
            DatAuditProgress::Hashing {
                index,
                total,
                file_name,
            } => {
                self.phase = AuditPhase::Hashing;
                self.files_checked = *index as u64;
                self.total_files = Some(*total as u64);
                self.current_path = Some(file_name.clone());
                self.eta.update(*index as u64, elapsed_seconds);
                self.eta_view = self.eta.eta(*index as u64, *total as u64, elapsed_seconds);
            }
            DatAuditProgress::Comparing { files } => {
                self.phase = AuditPhase::Comparing;
                self.files_checked = *files as u64;
                self.total_files = Some(*files as u64);
                self.eta_view = EtaView::None;
            }
        }
    }

    /// The view for one frame. `elapsed_seconds` is supplied by the caller so
    /// tests do not depend on a real clock; it only feeds the elapsed label.
    fn view(&self, elapsed_seconds: u64) -> AuditProgressView {
        let percent = match self.total_files {
            Some(total) if total > 0 => format_percentage(self.files_checked, total),
            _ => None,
        };
        AuditProgressView {
            phase: self.phase.label(),
            files_checked: self.files_checked,
            total_files: self.total_files,
            current_path: self.current_path.as_deref().map(shorten_path),
            elapsed_seconds,
            percent,
            eta: self.eta_view.clone(),
        }
    }
}

/// Exponential-moving-average throughput, so the ETA does not jump from one
/// frame's speed.
#[derive(Debug, Clone, PartialEq)]
struct EtaEstimator {
    smoothed_files_per_second: Option<f64>,
    last: Option<(u64, f64)>,
}

impl EtaEstimator {
    fn new() -> Self {
        Self {
            smoothed_files_per_second: None,
            last: None,
        }
    }

    /// Feeds one sample. `elapsed_seconds` is the time since the run started.
    ///
    /// A stall (no new files between two samples) or a non-advancing clock
    /// leaves the smoothed rate untouched: the estimate freezes rather than
    /// decaying, which is what "if progress stalls, stop updating the ETA"
    /// means.
    fn update(&mut self, checked: u64, elapsed_seconds: f64) {
        if let Some((last_checked, last_elapsed)) = self.last {
            let delta_seconds = elapsed_seconds - last_elapsed;
            let delta_files = checked.saturating_sub(last_checked) as f64;
            if delta_seconds > 0.0 && delta_files > 0.0 {
                let rate = delta_files / delta_seconds;
                self.smoothed_files_per_second = Some(match self.smoothed_files_per_second {
                    Some(previous) => {
                        ETA_SMOOTHING_ALPHA * rate + (1.0 - ETA_SMOOTHING_ALPHA) * previous
                    }
                    None => rate,
                });
            }
        }
        self.last = Some((checked, elapsed_seconds));
    }

    /// The ETA for the current position, applying the confidence gates.
    fn eta(&self, checked: u64, total: u64, elapsed_seconds: f64) -> EtaView {
        let Some(rate) = self.smoothed_files_per_second else {
            return EtaView::None;
        };
        if checked < ETA_MIN_FILES || elapsed_seconds < ETA_MIN_SECONDS {
            return EtaView::Estimating;
        }
        if rate <= 0.0 || total <= checked {
            // Nothing left to estimate, or no forward movement.
            return EtaView::None;
        }
        let seconds_remaining = ((total - checked) as f64 / rate).ceil() as u64;
        EtaView::About { seconds_remaining }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub(crate) struct DatSourcesPageState {
    config_path: PathBuf,
    /// Dedicated configuration and storage for explicitly managed DATs. This
    /// remains wholly separate from the user-local DAT registry above.
    managed_config_path: PathBuf,
    managed_root: PathBuf,
    managed_sources: ManagedDatSources,
    managed_load_error: Option<String>,
    managed_action_error: Option<String>,
    managed_statuses: BTreeMap<String, ManagedDatStatusView>,
    managed_job: Option<RunningManagedDatJob>,
    /// Persisted, user-selected local TOSEC release packs. This is wholly
    /// separate from both the local DAT registry and managed remote sources.
    tosec_packs_path: PathBuf,
    tosec_packs: Vec<PersistedTosecPack>,
    tosec_load_error: Option<String>,
    tosec_action_error: Option<String>,
    tosec_last_apply: Option<TosecApplyView>,
    no_intro_selected_pack: Option<PathBuf>,
    no_intro_inspection: Option<NoIntroPackInspection>,
    no_intro_installed: Option<NoIntroPackInspection>,
    no_intro_action_error: Option<String>,
    no_intro_import_status: Option<NoIntroPackImportStatus>,
    /// Existing EmuWiz catalogue to enrich after a completed audit. Absent
    /// in injected tests and when no catalogue exists.
    database_path: Option<PathBuf>,
    /// What is on disk, as last read or last written.
    saved: DatSourceRegistry,
    /// What the user has edited but not yet saved.
    draft: DatSourceRegistry,
    load_error: Option<String>,
    load_problems: Vec<String>,
    save_state: DatSaveState,
    action_error: Option<String>,
    /// The last validation report for each source, this session.
    validations: BTreeMap<String, DatValidationReport>,
    /// The grouped diagnostics for each validated source, derived once when the
    /// report lands and cached so the per-frame view rebuild does not re-group
    /// thousands of diagnostics. Kept in lockstep with `validations`.
    diagnostic_groups: BTreeMap<String, Vec<DiagnosticGroupView>>,
    /// The final tally of the last completed (or cancelled) "Validate all"
    /// run, this session. `None` until one has run at least once, and reset
    /// to `None` at the start of the next run and on Revert.
    last_validate_all_summary: Option<ValidateAllSummary>,
    audit: Option<Box<DatAuditOutcome>>,
    audit_error: Option<String>,
    /// How long the most recent completed audit took, read from the job's start
    /// instant when its result arrived. `None` until an audit completes.
    audit_elapsed_seconds: Option<u64>,
    job: Option<RunningJob>,
    /// Decides whether a symlinked ROM may be followed while hashing, exactly
    /// as it does everywhere else in the build.
    trusted: TrustedRoots,
    library_folders: Vec<PathBuf>,
    limits: DatLimits,
    /// The DAT matching policy scope being edited and inspected. `None` is the
    /// global scope; a value is a canonical platform id. Persisted nowhere - it
    /// only decides which preferences the policy section reads and writes.
    policy_scope: Option<String>,
    /// The read-only rename plan derived from the latest audit.
    rename_plan: Option<RenamePlan>,
    /// Why the plan could not be produced, when it could not.
    rename_plan_error: Option<String>,
    /// Monotonically increasing audit generation. Each audit start bumps it;
    /// a result carrying an older generation is a stale plan and is dropped.
    audit_generation: u64,
    /// Set when a current audit has had a chance to enrich the catalogue; the
    /// shell consumes it to reload Library metadata once.
    identity_enrichment_completed: bool,
    identity_enrichment: Option<Box<archivefs_core::PlatformIdentityEnrichmentSummary>>,
    /// Session-only review decisions, keyed by source path. Recording one
    /// never touches a file; nothing here persists them (deferral documented).
    review_decisions: BTreeMap<String, ReviewDecision>,
    /// The durable transaction journal directory.
    transaction_dir: PathBuf,
    /// The transaction built at review time, awaiting confirmation.
    apply_review: Option<archivefs_core::dat::rename_apply::RenameTransaction>,
    /// The approved source paths the review was built from.
    apply_approved: std::collections::BTreeSet<String>,
    /// The last apply outcome (worker result).
    apply_outcome: Option<ApplyOutcome>,
    apply_error: Option<String>,
    /// Whether the safe-subset option is offered after an AbortAll conflict.
    subset_available: bool,
    apply_running: bool,
    rollback_running: bool,
    rollback_result: Option<RollbackResult>,
    rollback_error: Option<String>,
    /// The apply/rollback worker.
    apply_job: Option<ApplyJob>,
    /// Transactions found on disk that are still actionable: settled `Applied`
    /// batches (eligible for rollback after a restart) and interrupted batches
    /// (offered for crash recovery). `RolledBack` journals are neither.
    recovery_transactions: Vec<archivefs_core::dat::rename_apply::RenameTransaction>,
    /// The error from the last failed attempt to persist a "Leave
    /// untouched" resolution to a journal (disk I/O, an unreadable
    /// journal, …). `None` on success or before any attempt.
    recovery_resolution_error: Option<String>,
    /// Transaction ids dismissed from Quick Rename's own "View
    /// recovery/history" list via "Hide settled history". Deliberately
    /// session-only and deliberately *not* used for "Leave untouched"
    /// (that is now a durable, per-transaction `recovery_resolution` on the
    /// journal itself - see `handle_recovery_choice` and
    /// `RenameTransaction::needs_attention`): "Hide settled history" is a
    /// temporary display preference about already-settled, already-safe
    /// history, not a decision about an unresolved transaction, and the two
    /// must not be conflated. Filtered out of `recovery_transactions` on
    /// every reload so the hide sticks for the rest of this session
    /// (`refresh_recovery` reloads from disk on every poll and would
    /// otherwise re-surface it next frame).
    dismissed_recovery_ids: std::collections::BTreeSet<String>,
    /// History & Logs records produced by apply/rollback outcomes, drained by
    /// the shell. No private paths are included.
    history_records: Vec<RenameHistoryRecord>,
}

impl DatSourcesPageState {
    /// Loads the registry, falling back to an empty one when the file is absent.
    ///
    /// A parse failure is surfaced rather than swallowed, and saving is refused
    /// while it stands: writing an empty registry over a file that failed to
    /// parse would destroy content the user may still want to fix by hand.
    pub(crate) fn load(
        config_path: PathBuf,
        library_folders: Vec<PathBuf>,
        trusted: TrustedRoots,
    ) -> Self {
        let transaction_dir = archivefs_core::dat::rename_apply::default_rename_transaction_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("archivefs-rename-transactions"));
        let managed_config_path = default_managed_dat_sources_config_path()
            .unwrap_or_else(|_| config_path.with_file_name("managed_dat_sources.toml"));
        let managed_root =
            managed_dat_root().unwrap_or_else(|_| config_path.with_file_name("managed-dats"));
        let mut state = Self::load_with_transaction_dir_and_managed_paths(
            config_path,
            library_folders,
            trusted,
            transaction_dir,
            managed_config_path,
            managed_root,
        );
        state.no_intro_installed = load_current_no_intro_pack_summary().ok().flatten();
        state
    }

    /// [`Self::load`] with the rename-transaction journal directory injected,
    /// so tests never read or write the real home directory.
    pub(crate) fn load_with_transaction_dir(
        config_path: PathBuf,
        library_folders: Vec<PathBuf>,
        trusted: TrustedRoots,
        transaction_dir: PathBuf,
    ) -> Self {
        // Tests and injected callers stay entirely inside their supplied
        // registry location rather than reading the real app configuration.
        let managed_config_path = config_path.with_file_name("managed_dat_sources.toml");
        let managed_root = config_path.with_file_name("managed-dats");
        Self::load_with_transaction_dir_and_managed_paths(
            config_path,
            library_folders,
            trusted,
            transaction_dir,
            managed_config_path,
            managed_root,
        )
    }

    /// Testable/injected variant with dedicated managed config and object
    /// roots. It performs local reads only; it never starts a network action.
    pub(crate) fn load_with_transaction_dir_and_managed_paths(
        config_path: PathBuf,
        library_folders: Vec<PathBuf>,
        trusted: TrustedRoots,
        transaction_dir: PathBuf,
        managed_config_path: PathBuf,
        managed_root: PathBuf,
    ) -> Self {
        let mut load_error = None;
        let mut load_problems = Vec::new();
        let saved = match load_dat_sources_config_from(&config_path) {
            Ok(config) => {
                let (registry, problems) = DatSourceRegistry::from_config(&config);
                load_problems = problems;
                registry
            }
            Err(error) => {
                load_error = Some(error.to_string());
                DatSourceRegistry::new()
            }
        };
        let draft = saved.clone();
        let (managed_sources, managed_load_error) =
            match load_managed_dat_sources_from(&managed_config_path) {
                Ok(sources) => (sources, None),
                Err(error) => (ManagedDatSources::new(), Some(error.to_string())),
            };
        // Keep injected/test pages inside the supplied configuration tree;
        // production callers pass the normal app-owned default path above.
        let tosec_packs_path = if managed_config_path
            == default_managed_dat_sources_config_path()
                .unwrap_or_else(|_| config_path.with_file_name("managed_dat_sources.toml"))
        {
            default_tosec_packs_path()
                .unwrap_or_else(|_| config_path.with_file_name("tosec_release_packs.json"))
        } else {
            config_path.with_file_name("tosec_release_packs.json")
        };
        let (tosec_packs, tosec_load_error) = match load_tosec_packs(&tosec_packs_path) {
            Ok(packs) => (packs, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        // The durable journal directory and any transactions found in it that
        // are still actionable: settled `Applied` batches offered for rollback
        // and interrupted batches offered for explicit crash recovery. Uses
        // the exact same reconciliation `refresh_recovery` applies to an
        // already-running page, so a transaction rediscovered after a
        // restart can never disagree with one the page had open the whole
        // time - see `load_reconciled_recovery_transactions`.
        let recovery_transactions = Self::load_reconciled_recovery_transactions(&transaction_dir);
        Self {
            config_path,
            managed_config_path,
            managed_root,
            managed_sources,
            managed_load_error,
            managed_action_error: None,
            managed_statuses: BTreeMap::new(),
            managed_job: None,
            tosec_packs_path,
            tosec_packs,
            tosec_load_error,
            tosec_action_error: None,
            tosec_last_apply: None,
            no_intro_selected_pack: None,
            no_intro_inspection: None,
            no_intro_installed: None,
            no_intro_action_error: None,
            no_intro_import_status: None,
            database_path: None,
            saved,
            draft,
            load_error,
            load_problems,
            save_state: DatSaveState::Idle,
            action_error: None,
            validations: BTreeMap::new(),
            diagnostic_groups: BTreeMap::new(),
            last_validate_all_summary: None,
            audit: None,
            audit_error: None,
            audit_elapsed_seconds: None,
            job: None,
            trusted,
            library_folders,
            limits: DatLimits::default(),
            policy_scope: None,
            rename_plan: None,
            rename_plan_error: None,
            audit_generation: 0,
            identity_enrichment_completed: false,
            identity_enrichment: None,
            review_decisions: BTreeMap::new(),
            transaction_dir,
            apply_review: None,
            apply_approved: std::collections::BTreeSet::new(),
            apply_outcome: None,
            apply_error: None,
            subset_available: false,
            apply_running: false,
            rollback_running: false,
            rollback_result: None,
            rollback_error: None,
            apply_job: None,
            recovery_transactions,
            recovery_resolution_error: None,
            dismissed_recovery_ids: std::collections::BTreeSet::new(),
            history_records: Vec::new(),
        }
    }

    pub(crate) fn with_database_path(mut self, database_path: Option<PathBuf>) -> Self {
        self.database_path = database_path.filter(|path| path.is_file());
        self
    }

    pub(crate) fn take_identity_enrichment_completed(&mut self) -> bool {
        std::mem::take(&mut self.identity_enrichment_completed)
    }

    /// How many sources are registered on disk, right now - what Home
    /// shows once this page has been visited this session. Reads `saved`,
    /// not `draft`: an unsaved edit should not change what Home reports.
    pub(crate) fn registered_source_count(&self) -> usize {
        self.saved.len()
    }

    /// Whether the draft differs from what is on disk.
    ///
    /// Compared as serialised configuration, because that is exactly what a
    /// save would write: an edit that round-trips to the same document is
    /// genuinely not a change.
    pub(crate) fn is_dirty(&self) -> bool {
        self.draft.to_config() != self.saved.to_config()
    }

    /// Whether a background job is running.
    pub(crate) fn is_busy(&self) -> bool {
        self.job.is_some() || self.apply_job.is_some() || self.managed_job.is_some()
    }

    /// Signals cancellation and immediately forgets the running job, whatever
    /// it targets.
    ///
    /// Dropping `self.job` drops the channel's receiving end, so any message
    /// the worker later sends - including one already in flight - fails
    /// silently on the sending side (every send in this module is `let _ =
    /// sender.send(...)`) rather than being read by a future `poll()`. That is
    /// what makes this safe to call even though the worker thread itself is
    /// not joined: it keeps running to whatever its own bound is (a parse
    /// bounded by `DatLimits`, or an audit that now observes `cancel`), but
    /// nothing it produces can reach page state again.
    fn abandon_running_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// [`Self::abandon_running_job`], but only when the running job targets
    /// `id` - so removing one source does not cancel an audit or validation
    /// legitimately running against a different one.
    fn abandon_job_for(&mut self, id: &str) {
        if self.job.as_ref().is_some_and(|job| job.source_id == id) {
            self.abandon_running_job();
        }
    }

    /// Drains whatever the worker has sent since the last frame.
    ///
    /// Called before [`Self::view`] so the view stays a pure function of state.
    /// Returns true when something arrived, so the caller can request a repaint
    /// only when there is a reason to.
    /// Drains the rename apply/rollback worker, if one is running.
    fn poll_apply(&mut self) -> bool {
        let Some(job) = self.apply_job.as_mut() else {
            return false;
        };
        let mut changed = false;
        let mut finished = false;
        loop {
            match job.messages.try_recv() {
                Ok(ApplyJobMessage::Applied(outcome)) => {
                    let summary = outcome.summary.clone();
                    let transaction_id = outcome.transaction.transaction_id.clone();
                    self.history_records.push(RenameHistoryRecord {
                        action: RenameHistoryAction::Apply,
                        transaction_id,
                        message: format!(
                            "requested {}, applied {}, skipped {}, failed {}",
                            summary.requested, summary.applied, summary.skipped, summary.failed
                        ),
                    });
                    self.apply_outcome = Some(*outcome);
                    self.apply_error = None;
                    self.apply_running = false;
                    self.apply_review = None;
                    changed = true;
                    finished = true;
                }
                Ok(ApplyJobMessage::RolledBack(outcome)) => {
                    let transaction_id = outcome.transaction.transaction_id.clone();
                    let label = match &outcome.result {
                        RollbackResult::FullyRolledBack => "fully rolled back",
                        RollbackResult::PartiallyRolledBack { .. } => "partially rolled back",
                        RollbackResult::RollbackFailed { .. } => "rollback failed",
                    };
                    self.history_records.push(RenameHistoryRecord {
                        action: RenameHistoryAction::Rollback,
                        transaction_id,
                        message: label.to_string(),
                    });
                    self.rollback_result = Some(outcome.result.clone());
                    self.rollback_error = None;
                    self.rollback_running = false;
                    self.apply_outcome = Some(ApplyOutcome {
                        transaction: outcome.transaction.clone(),
                        summary:
                            archivefs_core::dat::rename_apply::TransactionSummary::from_transaction(
                                &outcome.transaction,
                            ),
                    });
                    changed = true;
                    finished = true;
                }
                Ok(ApplyJobMessage::HardConflicts(detail)) => {
                    self.apply_error = Some(detail);
                    self.subset_available = true;
                    self.apply_running = false;
                    changed = true;
                    finished = true;
                }
                Ok(ApplyJobMessage::Failed(error)) => {
                    if self.apply_running {
                        self.apply_error = Some(error);
                        self.apply_running = false;
                    } else if self.rollback_running {
                        self.rollback_error = Some(error);
                        self.rollback_running = false;
                    }
                    changed = true;
                    finished = true;
                }
                Ok(ApplyJobMessage::Cancelled) => {
                    self.apply_running = false;
                    self.rollback_running = false;
                    changed = true;
                    finished = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.apply_running = false;
                    self.rollback_running = false;
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.apply_job = None;
        }
        changed
    }

    /// Re-reads still-actionable transactions from the journal directory,
    /// reconciling any that need it, so the counts and rollback eligibility
    /// reflect what actually happened. A settled `Applied` transaction is
    /// listed as-is, eligible for rollback.
    fn refresh_recovery(&mut self) {
        let mut recovery = Self::load_reconciled_recovery_transactions(&self.transaction_dir);
        // The transaction applied this session is already shown by the apply
        // outcome card; do not list it a second time in the recovery list.
        if let Some(outcome) = &self.apply_outcome {
            recovery.retain(|transaction| {
                transaction.transaction_id != outcome.transaction.transaction_id
            });
        }
        // Re-applied on every reload: this is a session-only UI dismissal
        // (see `dismissed_recovery_ids`'s own doc), not a journal edit, so it
        // has to be re-excluded every time this function re-reads the
        // journal directory from disk.
        recovery.retain(|transaction| {
            !self
                .dismissed_recovery_ids
                .contains(&transaction.transaction_id)
        });
        self.recovery_transactions = recovery;
    }

    /// Every still-actionable transaction in `transaction_dir`, reconciled
    /// against the filesystem/entry-state evidence where needed - the one
    /// path both [`Self::refresh_recovery`] (a page already open) and
    /// [`Self::load_with_transaction_dir`] (a fresh load, e.g. after a
    /// restart) use, so the two can never disagree about what a settled
    /// transaction's state actually is.
    ///
    /// A transaction is reconciled (via
    /// [`archivefs_core::dat::rename_apply::reconcile_recovery`]) whenever
    /// either an entry is still in-flight (`Applying`/`RollingBack`) or the
    /// transaction's own overall state is still `Applying` even though its
    /// entries may have already durably settled - the second case is what a
    /// final journal write that failed to land after every entry finished
    /// looks like, and is exactly what `reconcile_recovery` now also
    /// repairs.
    fn load_reconciled_recovery_transactions(
        transaction_dir: &Path,
    ) -> Vec<archivefs_core::dat::rename_apply::RenameTransaction> {
        let (mut recovery, _) =
            archivefs_core::dat::rename_apply::find_rollbackable_transactions(transaction_dir);
        for transaction in &mut recovery {
            let needs_reconciliation = transaction.state
                == archivefs_core::dat::rename_apply::TransactionState::Applying
                || transaction.entries.iter().any(|entry| {
                    matches!(
                        entry.state,
                        archivefs_core::dat::rename_apply::EntryState::Applying
                            | archivefs_core::dat::rename_apply::EntryState::RollingBack
                    )
                });
            if needs_reconciliation {
                let _ = archivefs_core::dat::rename_apply::reconcile_recovery(
                    transaction,
                    transaction_dir,
                );
            }
        }
        recovery
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = self.poll_apply();
        changed |= self.poll_managed_dat_job();
        // Surface (and reconcile) interrupted transactions on every frame so the
        // recovery banner appears as soon as the page is entered.
        self.refresh_recovery();
        let Some(job) = self.job.as_mut() else {
            return changed;
        };
        let mut finished = false;
        // Read the clock once per drain pass. Every queued message was produced
        // between this pass and the last one, so they all share one elapsed
        // value; timestamping each message afresh would make a drained backlog
        // look like an enormous files-per-second rate and collapse the ETA to
        // near zero. The `delta_seconds > 0` guard inside `EtaEstimator::update`
        // then skips every message after the first of the burst. The job stays
        // alive for the whole pass - terminal messages only flag `finished`,
        // which clears `self.job` after the loop - so reading `job.started_at`
        // here is safe.
        let elapsed = job.started_at.elapsed().as_secs_f64();
        // Coalescing: the detail line is derived once per drain pass from the
        // last progress event, so a backlog of a thousand messages builds one
        // string instead of a thousand. The tracker still ingests every event
        // (that is what keeps the ETA smooth); only the string work is shared.
        let mut last_audit_progress: Option<DatAuditProgress> = None;
        loop {
            match job.messages.try_recv() {
                Ok(JobMessage::Progress(line)) => {
                    // Once cancellation has been requested, stale progress must
                    // not restore an active-looking detail line.
                    if !job.cancel_requested {
                        job.latest = line;
                    }
                    changed = true;
                }
                Ok(JobMessage::AuditProgress(event)) => {
                    // Once cancellation is requested, progress is frozen: the
                    // detail line, the position, and the ETA all stop moving so
                    // a stale report cannot restore an active-looking state.
                    if !job.cancel_requested
                        && let Some(tracker) = job.audit_progress.as_mut()
                    {
                        tracker.update(&event, elapsed);
                        last_audit_progress = Some(event);
                    }
                    changed = true;
                }
                Ok(JobMessage::Validated(report)) => {
                    if job.kind == JobKind::Validate && job.cancel_requested {
                        // A single Validate's result landing after
                        // cancellation was requested must not repopulate
                        // state: the user stopped this job. `ValidateAll`
                        // does not take this branch - see its own arm below
                        // for why a bulk run's already-computed results are
                        // never discarded this way.
                        finished = true;
                    } else {
                        let id = report.source_id.clone();
                        // Captured before the overwrite below, so a
                        // `ValidateAll` run can tell whether this source's
                        // recorded health actually changed.
                        let previous_health = self.draft.get(&id).map(|entry| entry.health.clone());
                        // The health the run observed is written onto the
                        // *draft*, so it becomes an unsaved change like any
                        // other and the user chooses whether to keep it -
                        // exactly the same for a bulk run as for a single one.
                        let mut new_health = None;
                        if let Some(entry) = self.draft.get_mut(&id) {
                            let health = report.to_health(&entry.path.clone(), entry.kind);
                            new_health = Some(health.clone());
                            entry.health = health;
                        }
                        // The grouped diagnostics are derived once here, when
                        // the report lands, and cached: the view is rebuilt
                        // every frame, and re-grouping thousands of diagnostics
                        // per frame would be pure churn for an unchanged report.
                        let groups = group_diagnostics(&id, &report);
                        let state = report.state;
                        self.validations.insert(id.clone(), *report);
                        self.diagnostic_groups.insert(id, groups);
                        changed = true;
                        match job.kind {
                            JobKind::ValidateAll => {
                                // Committed regardless of `cancel_requested`:
                                // this source had already finished (or was
                                // already in flight) when Cancel was pressed,
                                // so its real result is kept - only sources
                                // not yet reached are ever skipped. The job
                                // itself only ends on `ValidateAllFinished`.
                                if let (Some(bulk), Some(new_health)) =
                                    (job.bulk.as_mut(), new_health)
                                {
                                    bulk.completed += 1;
                                    match classify_validate_all_outcome(
                                        state,
                                        previous_health.as_ref(),
                                        &new_health,
                                    ) {
                                        ValidateAllOutcome::Valid => bulk.valid += 1,
                                        ValidateAllOutcome::Changed => bulk.changed += 1,
                                        ValidateAllOutcome::Failed => bulk.failed += 1,
                                    }
                                }
                            }
                            JobKind::Validate | JobKind::Audit => finished = true,
                        }
                    }
                }
                Ok(JobMessage::ValidatingNext { id, display_name }) => {
                    // Tracks reality (which source the worker is actually
                    // reading) even while "Stopping…" is showing, exactly
                    // like a single Validate/Audit job's own `source_id`
                    // does - only the terminal message is gated on
                    // cancellation, not this.
                    job.source_id = id;
                    if let Some(bulk) = job.bulk.as_ref() {
                        job.latest = format!(
                            "Validating {} of {}: {display_name}",
                            bulk.completed + 1,
                            bulk.total
                        );
                    }
                    changed = true;
                }
                Ok(JobMessage::ValidateAllFinished) => {
                    if let Some(bulk) = job.bulk {
                        self.last_validate_all_summary = Some(bulk.summary());
                    }
                    changed = true;
                    finished = true;
                }
                Ok(JobMessage::Audited {
                    generation,
                    outcome,
                    enrichment,
                    plan,
                }) => {
                    if job.cancel_requested {
                        // A cancelled audit never appears complete - even when
                        // the worker finished before it observed the flag, the
                        // page must not present the late result as a completed
                        // audit.
                        finished = true;
                    } else if generation != self.audit_generation {
                        // A stale generation (an older worker's result landing
                        // after a newer audit started) can never replace the
                        // current audit or plan.
                        finished = true;
                    } else {
                        // The elapsed time is measured from the job's own start
                        // instant, so the summary can say how long the run took
                        // without the worker carrying any timestamps.
                        self.audit_elapsed_seconds = Some(job.started_at.elapsed().as_secs());
                        self.audit = Some(outcome);
                        self.identity_enrichment_completed = self.database_path.is_some();
                        self.identity_enrichment = enrichment;
                        self.audit_error = None;
                        match plan {
                            Some(plan) => {
                                self.rename_plan = Some(*plan);
                                self.rename_plan_error = None;
                            }
                            None => {
                                self.rename_plan = None;
                                self.rename_plan_error = Some(
                                    "the rename plan could not be produced (cancelled or the \
                                     source files could not be inspected)"
                                        .to_string(),
                                );
                            }
                        }
                        changed = true;
                        finished = true;
                    }
                }
                Ok(JobMessage::Failed(error)) => {
                    match job.kind {
                        JobKind::Audit => {
                            self.audit = None;
                            self.audit_elapsed_seconds = None;
                            if !job.cancel_requested {
                                self.audit_error = Some(error);
                            }
                        }
                        JobKind::Validate | JobKind::ValidateAll => {
                            if !job.cancel_requested {
                                self.action_error = Some(error);
                            }
                        }
                    }
                    changed = true;
                    finished = true;
                }
                Ok(JobMessage::Cancelled) => {
                    // A cancelled audit produces no summary and no elapsed time.
                    self.audit = None;
                    self.audit_elapsed_seconds = None;
                    changed = true;
                    finished = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // The worker stopped without sending `ValidateAllFinished`
                    // (e.g. it panicked). Whatever it had already committed to
                    // `self.draft`/`self.validations` is real and stays; the
                    // tally accumulated so far is still worth showing rather
                    // than silently losing the whole run's summary.
                    if job.kind == JobKind::ValidateAll
                        && let Some(bulk) = job.bulk
                    {
                        self.last_validate_all_summary = Some(bulk.summary());
                        changed = true;
                    }
                    finished = true;
                    break;
                }
            }
        }
        // Derive the detail line once from the last progress event of the pass.
        if let Some(event) = last_audit_progress {
            job.latest = describe(&event);
        }
        if finished {
            self.job = None;
        }
        changed
    }

    /// Applies one action.
    pub(crate) fn apply(&mut self, action: DatSourcesPageAction) {
        // Any action clears the "saved" flash: leaving it up while the user
        // makes a new edit would say the current state is what is on disk.
        if !matches!(action, DatSourcesPageAction::Save) {
            self.save_state = DatSaveState::Idle;
        }
        match action {
            DatSourcesPageAction::AddFile { path } => self.add(path, DatSourceKind::File),
            DatSourcesPageAction::AddWHDLoadDat { path } => self.add_whdload_catalogue(path),
            DatSourcesPageAction::OpenNoIntroDownloadPage => {
                self.no_intro_action_error = open_no_intro_download_page();
            }
            DatSourcesPageAction::ChooseNoIntroPack { path } => {
                self.no_intro_selected_pack = Some(path);
                self.no_intro_inspection = None;
                self.no_intro_action_error = None;
                self.no_intro_import_status = None;
            }
            DatSourcesPageAction::InspectNoIntroPack => self.inspect_no_intro_pack(),
            DatSourcesPageAction::ImportNoIntroPack => self.import_no_intro_pack(),
            DatSourcesPageAction::AddFolder { path } => self.add(path, DatSourceKind::Folder),
            DatSourcesPageAction::SetEnabled { id, enabled } => {
                if let Some(entry) = self.draft.get_mut(&id) {
                    entry.enabled = enabled;
                }
            }
            DatSourcesPageAction::SetPlatform { id, platform } => {
                if let Some(entry) = self.draft.get_mut(&id) {
                    entry.platform = platform.filter(|value| !value.trim().is_empty());
                }
            }
            DatSourcesPageAction::Remove { id } => {
                self.action_error = None;
                // A job in flight for exactly this source must not be allowed
                // to complete after removal: `poll()` would otherwise write a
                // validation or audit result for a source no longer in the
                // registry. The GUI already keeps Remove disabled while any
                // job runs, but that is a presentation-layer gate, not a
                // guarantee this state machine can rely on - so it is
                // enforced here too.
                self.abandon_job_for(&id);
                // Registry only. The DAT file, the folder, and everything in it
                // are untouched - see `DatSourceRegistry::remove`.
                if self.draft.remove(&id).is_some() {
                    self.validations.remove(&id);
                    self.diagnostic_groups.remove(&id);
                    if self
                        .audit
                        .as_ref()
                        .is_some_and(|outcome| outcome.source_id == id)
                    {
                        // A result attributed to a source that is no longer
                        // registered would have nothing to point at.
                        self.audit = None;
                        self.rename_plan = None;
                        self.rename_plan_error = None;
                        self.review_decisions.clear();
                        self.abandon_apply_work();
                    }
                }
            }
            DatSourcesPageAction::Validate { id } => self.start_validate(id),
            DatSourcesPageAction::ValidateAll => self.start_validate_all(),
            DatSourcesPageAction::Audit { id, scan_root } => self.start_audit(id, scan_root),
            DatSourcesPageAction::AuditAllEnabled { scan_root } => {
                self.start_combined_audit(scan_root)
            }
            DatSourcesPageAction::OpenDatSources
            | DatSourcesPageAction::OpenAdvancedIdentifyRename => {}
            DatSourcesPageAction::AddManagedMameSoftwareList { authoritative_name } => {
                self.add_managed_mame_software_list(authoritative_name);
            }
            DatSourcesPageAction::RemoveManagedMameSoftwareList { authoritative_name } => {
                self.remove_managed_mame_software_list(&authoritative_name);
            }
            DatSourcesPageAction::AddManagedRedumpBios { system } => {
                self.add_managed_redump_bios(system);
            }
            DatSourcesPageAction::RemoveManagedRedumpBios { system } => {
                self.remove_managed_redump_bios(system);
            }
            DatSourcesPageAction::AddManagedRedumpGames { system } => {
                self.add_managed_redump_games(system);
            }
            DatSourcesPageAction::RemoveManagedRedumpGames { system } => {
                self.remove_managed_redump_games(system);
            }
            DatSourcesPageAction::CheckManagedDat { source_id } => {
                self.start_managed_dat_operation(source_id, ManagedDatOperation::Check);
            }
            DatSourcesPageAction::UpdateManagedDat { source_id } => {
                self.start_managed_dat_operation(source_id, ManagedDatOperation::Update);
            }
            DatSourcesPageAction::ImportTosecReleasePack { root } => self.import_tosec_pack(root),
            DatSourcesPageAction::RemoveTosecReleasePack { pack_id } => {
                self.remove_tosec_pack(&pack_id);
            }
            DatSourcesPageAction::SetTosecSelection {
                pack_id,
                key,
                enabled,
            } => self.set_tosec_selection(&pack_id, key, enabled),
            DatSourcesPageAction::ApplyTosecSelection { pack_id } => {
                self.apply_tosec_selection(&pack_id);
            }
            DatSourcesPageAction::CancelJob => {
                if let Some(job) = self.job.as_mut() {
                    job.cancel.store(true, Ordering::Relaxed);
                    // The visible card flips to "Stopping…" this frame; the job
                    // stays busy until the worker confirms termination.
                    job.cancel_requested = true;
                }
            }
            DatSourcesPageAction::Revert => {
                // A running job's result would otherwise still land after the
                // discard it was supposed to be swept away by: `poll()` does
                // not check that the job's source survived the revert, so a
                // job left running here would populate `self.audit` (or a
                // stale `self.validations` entry) for a source the user just
                // discarded - including one that no longer exists in the
                // registry at all, if it had never been saved. Dropping the
                // job unconditionally, not just when its target vanished,
                // matches what "discard changes" means: nothing this job
                // would report is still trustworthy against the reverted
                // state, whether or not the row it targeted survives.
                self.abandon_running_job();
                self.draft = self.saved.clone();
                self.action_error = None;
                // Discard also forgets the session's validation records: a
                // re-added source (which reuses its auto-suggested id) must not
                // show yesterday's Inspect detail or diagnostic groups next to
                // a "Not checked" badge. Both maps are discarded together.
                self.validations.clear();
                self.diagnostic_groups.clear();
                // A "Validate all" summary describes a run against the
                // discarded draft's sources; it goes with it, same as the
                // per-source validation records above.
                self.last_validate_all_summary = None;
                // The policy scope is not persisted, so discarding returns it
                // to the global scope rather than leaving a platform selected
                // whose override the user just discarded.
                self.policy_scope = None;
                // The audit result, its plan and the session-only review
                // decisions describe a discarded state, so they go with it.
                self.audit = None;
                self.audit_elapsed_seconds = None;
                self.rename_plan = None;
                self.rename_plan_error = None;
                self.review_decisions.clear();
                self.abandon_apply_work();
            }
            DatSourcesPageAction::Save => self.save(),
            DatSourcesPageAction::SelectPolicyScope { scope } => {
                self.policy_scope = scope;
            }
            DatSourcesPageAction::MoveRegion {
                scope,
                index,
                delta,
            } => {
                self.with_policy_targets(&scope, |targets| {
                    move_index(targets.region_list(), index, delta);
                });
            }
            DatSourcesPageAction::AddRegion { scope, region } => {
                self.with_policy_targets(&scope, |targets| {
                    let list = targets.region_list();
                    if !list.iter().any(|value| value == region.as_str()) {
                        list.push(region.as_str().to_string());
                    }
                });
            }
            DatSourcesPageAction::RemoveRegion { scope, index } => {
                self.with_policy_targets(&scope, |targets| {
                    let list = targets.region_list();
                    if index < list.len() {
                        list.remove(index);
                    }
                });
            }
            DatSourcesPageAction::MoveLanguage {
                scope,
                index,
                delta,
            } => {
                self.with_policy_targets(&scope, |targets| {
                    move_index(targets.language_list(), index, delta);
                });
            }
            DatSourcesPageAction::AddLanguage { scope, preference } => {
                self.with_policy_targets(&scope, |targets| {
                    let list = targets.language_list();
                    if !list.iter().any(|value| value == preference.as_str()) {
                        list.push(preference.as_str().to_string());
                    }
                });
            }
            DatSourcesPageAction::RemoveLanguage { scope, index } => {
                self.with_policy_targets(&scope, |targets| {
                    let list = targets.language_list();
                    if index < list.len() {
                        list.remove(index);
                    }
                });
            }
            DatSourcesPageAction::ClearRegion { scope } => {
                self.with_policy_targets(&scope, |targets| {
                    targets.region_list().clear();
                });
            }
            DatSourcesPageAction::ClearLanguage { scope } => {
                self.with_policy_targets(&scope, |targets| {
                    targets.language_list().clear();
                });
            }
            DatSourcesPageAction::SetRevisionPolicy { scope, policy } => {
                self.with_policy_targets(&scope, |targets| {
                    *targets.revision = Some(policy.as_str().to_string());
                });
            }
            DatSourcesPageAction::SetClonePolicy { scope, policy } => {
                self.with_policy_targets(&scope, |targets| {
                    *targets.clone = Some(policy.as_str().to_string());
                });
            }
            DatSourcesPageAction::SetContentSelection { scope, policy } => {
                self.with_policy_targets(&scope, |targets| {
                    *targets.content = Some(policy.as_str().to_string());
                });
                // The completed audit remains authoritative, but its selection
                // annotation and any action plan were built for the old policy.
                self.audit = None;
                self.audit_error = None;
                self.rename_plan = None;
                self.rename_plan_error = None;
                self.review_decisions.clear();
                self.abandon_apply_work();
            }
            DatSourcesPageAction::SetReviewDecision { path, decision } => match decision {
                Some(decision) => {
                    self.review_decisions.insert(path, decision);
                }
                None => {
                    self.review_decisions.remove(&path);
                }
            },
            DatSourcesPageAction::ClearReviewDecisions => {
                self.review_decisions.clear();
            }
            DatSourcesPageAction::SelectAllActionable => self.select_all_actionable(),
            DatSourcesPageAction::BeginApplyReview => self.begin_apply_review(),
            DatSourcesPageAction::QuickRenamePrepareApply => {
                // Quick Rename's single "prepare to rename" step: the whole
                // point of the button is "make every currently safe file
                // ready to rename," so it always selects every actionable
                // proposal fresh (never a partial manual selection left over
                // from Review changes) before building the same apply
                // review the advanced planner uses. No new engine: this is
                // exactly `SelectAllActionable` followed by
                // `BeginApplyReview`, just as one click instead of two.
                self.select_all_actionable();
                self.begin_apply_review();
            }
            DatSourcesPageAction::ConfirmApply { typed } => {
                self.confirm_apply(HardConflictMode::AbortAll, typed)
            }
            DatSourcesPageAction::ConfirmApplySafeSubset { typed } => {
                self.confirm_apply(HardConflictMode::SkipUnsafeSubset, typed)
            }
            DatSourcesPageAction::CancelApplyReview => {
                self.apply_review = None;
                self.apply_error = None;
                self.subset_available = false;
            }
            DatSourcesPageAction::RollbackTransaction { id } => self.start_rollback(id),
            DatSourcesPageAction::RecoveryChoice { id, choice } => {
                self.handle_recovery_choice(id, choice)
            }
            DatSourcesPageAction::ClearApplyOutcome => {
                self.apply_outcome = None;
                self.rollback_result = None;
                self.apply_error = None;
                self.rollback_error = None;
            }
            DatSourcesPageAction::ResetQuickRenameSession => {
                self.audit = None;
                self.audit_error = None;
                self.identity_enrichment = None;
                self.rename_plan = None;
                self.rename_plan_error = None;
                self.review_decisions.clear();
                self.abandon_apply_work();
            }
            DatSourcesPageAction::HideSettledRecoveryHistory => {
                for transaction in &self.recovery_transactions {
                    if transaction.state == TransactionState::Applied {
                        self.dismissed_recovery_ids
                            .insert(transaction.transaction_id.clone());
                    }
                }
                self.recovery_transactions
                    .retain(|transaction| transaction.state != TransactionState::Applied);
            }
        }
    }

    /// Marks every currently actionable (`Suggested`) proposal as accepted
    /// for review. Session-only, like every other review decision; never
    /// touches a file.
    fn select_all_actionable(&mut self) {
        if let Some(plan) = self.rename_plan.as_ref() {
            for proposal in &plan.proposals {
                if proposal.state.is_actionable() {
                    self.review_decisions.insert(
                        proposal.source_path.to_string_lossy().into_owned(),
                        ReviewDecision::AcceptedForReview,
                    );
                }
            }
        }
    }

    /// Builds the transaction for the approved, applicable proposals and shows
    /// the read-only review. No mutation.
    fn begin_apply_review(&mut self) {
        let Some(plan) = self.rename_plan.as_ref() else {
            self.apply_error = Some("no rename plan is available".to_string());
            return;
        };
        let approved: std::collections::BTreeSet<String> = self
            .review_decisions
            .iter()
            .filter(|(_, decision)| archivefs_core::dat::rename_apply::is_approved(decision))
            .map(|(path, _)| path.clone())
            .collect();
        if approved.is_empty() {
            self.apply_error = Some(
                "no proposals are accepted for review; accept at least one Suggested proposal"
                    .to_string(),
            );
            return;
        }
        match archivefs_core::dat::rename_apply::build_transaction(plan, &approved, plan.generation)
        {
            Ok(transaction) => {
                self.apply_approved = approved;
                self.apply_review = Some(transaction);
                self.apply_error = None;
                self.subset_available = false;
            }
            Err(error) => {
                self.apply_error = Some(error.to_string());
            }
        }
    }

    /// Confirms the review and runs the apply on a worker thread. The typed
    /// confirmation is validated here (the drawing layer collects it), so a
    /// large batch cannot be confirmed without the exact phrase.
    fn confirm_apply(&mut self, mode: HardConflictMode, typed: String) {
        let Some(review) = self.apply_review.as_ref() else {
            return;
        };
        let count = review.entries.len();
        let required = typed_confirmation_phrase(count);
        if count > TYPED_CONFIRMATION_THRESHOLD && typed.trim() != required {
            self.apply_error = Some(format!("type {required} to confirm this batch"));
            return;
        }
        self.spawn_apply(mode);
    }

    /// Spawns the apply worker for the current review transaction.
    fn spawn_apply(&mut self, mode: HardConflictMode) {
        if self.apply_job.is_some() {
            return;
        }
        let Some(review) = self.apply_review.as_ref() else {
            return;
        };
        let mut transaction = review.clone();
        let approved = self.apply_approved.clone();
        let current_generation = self.rename_plan.as_ref().map(|p| p.generation).unwrap_or(0);
        let trusted = self.trusted.clone();
        let journal_dir = self.transaction_dir.clone();
        let (sender, messages) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let apply_sender = sender.clone();

        std::thread::spawn(move || {
            let result = apply_transaction(&mut ApplyExecution {
                transaction: &mut transaction,
                approved_paths: approved,
                current_generation,
                trusted,
                journal_dir,
                hard_conflict_mode: mode,
                cancel: &worker_cancel,
                directory_policy:
                    archivefs_core::dat::rename_apply::preflight::DirectoryPolicy::SameDirectory,
                allow_symlink_source: false,
            });
            let _ = match result {
                Ok(outcome) => apply_sender.send(ApplyJobMessage::Applied(Box::new(outcome))),
                Err(ApplyError::Cancelled) => apply_sender.send(ApplyJobMessage::Cancelled),
                Err(ApplyError::HardConflicts(conflicts)) => {
                    let detail = conflicts
                        .iter()
                        .map(|(path, reasons)| {
                            format!(
                                "{}: {}",
                                path.file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| path.display().to_string()),
                                reasons.join("; ")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    apply_sender.send(ApplyJobMessage::HardConflicts(format!(
                        "preflight found {} hard conflict(s); nothing was renamed.\n{detail}",
                        conflicts.len()
                    )))
                }
                Err(error) => apply_sender.send(ApplyJobMessage::Failed(error.to_string())),
            };
        });

        self.apply_job = Some(ApplyJob { cancel, messages });
        self.apply_running = true;
        self.apply_error = None;
        self.subset_available = false;
    }

    /// Rolls back a transaction (the last applied one, or a recovered one).
    fn start_rollback(&mut self, transaction_id: String) {
        if self.apply_job.is_some() {
            return;
        }
        // Load the journal for this transaction id.
        let Some(path) =
            archivefs_core::dat::rename_apply::journal_path(&self.transaction_dir, &transaction_id)
        else {
            self.rollback_error = Some("transaction id cannot name a journal".to_string());
            return;
        };
        let mut transaction = match archivefs_core::dat::rename_apply::read_journal(&path) {
            Ok(transaction) => transaction,
            Err(error) => {
                self.rollback_error = Some(format!("journal unreadable: {error}"));
                return;
            }
        };
        let journal_dir = self.transaction_dir.clone();
        let (sender, messages) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let rollback_sender = sender.clone();
        std::thread::spawn(move || {
            let result = rollback_transaction(&mut transaction, &journal_dir, &worker_cancel);
            let _ = match result {
                Ok(outcome) => rollback_sender.send(ApplyJobMessage::RolledBack(Box::new(outcome))),
                Err(error) => rollback_sender.send(ApplyJobMessage::Failed(error)),
            };
        });
        self.apply_job = Some(ApplyJob { cancel, messages });
        self.rollback_running = true;
        self.rollback_error = None;
    }

    /// Handles a crash-recovery choice for an interrupted transaction.
    fn handle_recovery_choice(&mut self, id: String, choice: RecoveryChoice) {
        match choice {
            RecoveryChoice::RollBack => self.start_rollback(id),
            RecoveryChoice::LeaveUntouched => {
                // Persisted durably to the journal itself
                // (`recovery_resolution`) so the choice survives a restart -
                // no journal is written to lie about `state`, and nothing is
                // deleted. See `RenameTransaction::needs_attention` for how
                // this is told apart from "genuinely still needs a
                // decision" without touching the truthful `state` field at
                // all.
                match archivefs_core::dat::rename_apply::resolve_leave_untouched(
                    &self.transaction_dir,
                    &id,
                ) {
                    Ok(updated) => {
                        self.recovery_resolution_error = None;
                        if let Some(existing) = self
                            .recovery_transactions
                            .iter_mut()
                            .find(|transaction| transaction.transaction_id == id)
                        {
                            *existing = updated;
                        }
                    }
                    Err(error) => {
                        self.recovery_resolution_error =
                            Some(format!("your choice could not be saved: {error}"));
                    }
                }
            }
        }
    }

    /// History & Logs records produced by apply/rollback outcomes, drained by
    /// the shell. No private paths are included.
    pub(crate) fn drain_history_records(&mut self) -> Vec<RenameHistoryRecord> {
        std::mem::take(&mut self.history_records)
    }

    /// Abandons in-memory apply/review state (the journals and files are
    /// untouched; an in-flight worker simply stops delivering to this page).
    fn abandon_apply_work(&mut self) {
        if let Some(job) = self.apply_job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.apply_review = None;
        self.apply_outcome = None;
        self.apply_error = None;
        self.subset_available = false;
        self.apply_running = false;
        self.rollback_running = false;
        self.rollback_result = None;
        self.rollback_error = None;
    }

    /// Applies `edit` to the four policy fields at `scope` (global or one
    /// platform's override).
    ///
    /// Editing a platform scope creates the override entry on demand; when
    /// the edit leaves it empty it is pruned again, so saving never writes an
    /// empty `[policy.platforms.X]` table the user did not ask for.
    fn with_policy_targets(
        &mut self,
        scope: &Option<String>,
        edit: impl FnOnce(&mut PolicyTargets<'_>),
    ) {
        match scope {
            None => {
                let policy = self.draft.policy_mut();
                let mut targets = PolicyTargets::new(
                    &mut policy.region_preferences,
                    &mut policy.language_preferences,
                    &mut policy.revision_policy,
                    &mut policy.clone_policy,
                    &mut policy.content_selection,
                );
                edit(&mut targets);
                targets.normalise_empty_lists();
            }
            Some(platform) => {
                let overrides = self
                    .draft
                    .policy_mut()
                    .platforms
                    .get_or_insert_with(BTreeMap::new);
                let entry = overrides.entry(platform.clone()).or_default();
                let mut targets = PolicyTargets::new(
                    &mut entry.region_preferences,
                    &mut entry.language_preferences,
                    &mut entry.revision_policy,
                    &mut entry.clone_policy,
                    &mut entry.content_selection,
                );
                edit(&mut targets);
                targets.normalise_empty_lists();
                self.prune_empty_overrides();
            }
        }
    }

    /// Removes platform override entries that no longer set anything, and the
    /// whole `platforms` map when it is empty.
    fn prune_empty_overrides(&mut self) {
        let policy = self.draft.policy_mut();
        let Some(overrides) = policy.platforms.as_mut() else {
            return;
        };
        overrides.retain(|_, entry| {
            entry.region_preferences.is_some()
                || entry.language_preferences.is_some()
                || entry.revision_policy.is_some()
                || entry.clone_policy.is_some()
                || entry.content_selection.is_some()
        });
        if overrides.is_empty() {
            policy.platforms = None;
        }
    }

    fn add(&mut self, path: PathBuf, kind: DatSourceKind) {
        self.action_error = None;
        let entry = DatSourceEntry {
            origin: Some("added on the DAT Sources page".to_string()),
            ..DatSourceEntry::new(
                self.draft.suggest_id(&path),
                suggest_display_name(&path),
                path,
                kind,
            )
        };
        if let Err(error) = self.draft.add(entry) {
            self.action_error = Some(error.to_string());
        }
    }

    fn inspect_no_intro_pack(&mut self) {
        self.no_intro_action_error = None;
        let Some(path) = self.no_intro_selected_pack.as_deref() else {
            self.no_intro_action_error = Some("Choose a No-Intro ZIP first.".to_string());
            return;
        };
        match inspect_no_intro_pack(path) {
            Ok(inspection) => self.no_intro_inspection = Some(inspection),
            Err(error) => self.no_intro_action_error = Some(error.to_string()),
        }
    }

    fn import_no_intro_pack(&mut self) {
        self.no_intro_action_error = None;
        let Some(path) = self.no_intro_selected_pack.as_deref() else {
            self.no_intro_action_error = Some("Choose a No-Intro ZIP first.".to_string());
            return;
        };
        match import_no_intro_pack(path) {
            Ok(report) => {
                self.no_intro_import_status = Some(report.status);
                self.no_intro_installed = load_current_no_intro_pack_summary().ok().flatten();
                self.no_intro_inspection = None;
                let old_pack_ids: Vec<String> = self
                    .draft
                    .entries()
                    .iter()
                    .filter(|entry| {
                        entry.origin.as_deref() == Some("browser-assisted No-Intro pack import")
                    })
                    .map(|entry| entry.id.clone())
                    .collect();
                for id in old_pack_ids {
                    let _ = self.draft.remove(&id);
                }
                for source in report.accepted {
                    if self
                        .draft
                        .entries()
                        .iter()
                        .any(|entry| entry.path == source.artifact_path)
                    {
                        continue;
                    }
                    let entry = DatSourceEntry {
                        display_name: format!("No-Intro: {}", source.system_name),
                        origin: Some("browser-assisted No-Intro pack import".to_string()),
                        ..DatSourceEntry::new(
                            self.draft.suggest_id(&source.artifact_path),
                            format!("No-Intro: {}", source.system_name),
                            source.artifact_path,
                            DatSourceKind::File,
                        )
                    };
                    if let Err(error) = self.draft.add(entry) {
                        self.no_intro_action_error = Some(error.to_string());
                        break;
                    }
                }
                self.save();
            }
            Err(error) => self.no_intro_action_error = Some(error.to_string()),
        }
    }

    /// Registers only the known public WHDLoad catalogue shape.  The source
    /// remains `UserLocal`: its provenance is presentation/audit information,
    /// never authority to update a remote Retroplay collection.
    fn add_whdload_catalogue(&mut self, path: PathBuf) {
        self.action_error = None;
        let parsed = match parse_dat_file(&path, DatLimits::default()) {
            Ok(parsed) => parsed.dat,
            Err(error) => {
                self.action_error = Some(format!("WHDLoad DAT could not be parsed: {error}"));
                return;
            }
        };
        if parsed.source.format != DatFormat::ClrMamePro
            || parsed.source.name.as_deref() != Some("Commodore - Amiga - WHDLoad")
        {
            self.action_error = Some(
                "That file is not the expected Commodore - Amiga - WHDLoad ClrMamePro catalogue; it was not added."
                    .to_string(),
            );
            return;
        }
        let entry = DatSourceEntry {
            display_name: "WHDLoad / Retroplay catalogue".to_string(),
            origin: Some(
                "WHDLoad / Retroplay-derived local catalogue selected through DAT Sources"
                    .to_string(),
            ),
            ..DatSourceEntry::new(
                self.draft.suggest_id(&path),
                "WHDLoad / Retroplay catalogue".to_string(),
                path,
                DatSourceKind::File,
            )
        };
        if let Err(error) = self.draft.add(entry) {
            self.action_error = Some(error.to_string());
        }
    }

    /// Persists a new managed source immediately. Unlike local DAT sources,
    /// this configuration is not a draft: it grants only the fixed typed MAME
    /// descriptor authority, and must survive a restart before any download.
    fn add_managed_mame_software_list(&mut self, authoritative_name: String) {
        self.managed_action_error = None;
        if self.managed_load_error.is_some() {
            self.managed_action_error = Some(
                "Not changing managed sources: their existing configuration could not be read."
                    .to_string(),
            );
            return;
        }
        let mut next = self.managed_sources.clone();
        if let Err(error) =
            next.add_mame_software_list(authoritative_name, ManagedDatUpdatePolicy::Manual)
        {
            self.managed_action_error = Some(error.to_string());
            return;
        }
        match save_managed_dat_sources_to(&self.managed_config_path, &next) {
            Ok(()) => self.managed_sources = next,
            Err(error) => self.managed_action_error = Some(error.to_string()),
        }
    }

    /// Removes configuration only. The core deliberately leaves immutable
    /// snapshots and state in place for a later explicit maintenance command.
    fn remove_managed_mame_software_list(&mut self, authoritative_name: &str) {
        self.managed_action_error = None;
        if self.managed_job.is_some() {
            return;
        }
        if self.managed_load_error.is_some() {
            self.managed_action_error = Some(
                "Not changing managed sources: their existing configuration could not be read."
                    .to_string(),
            );
            return;
        }
        let mut next = self.managed_sources.clone();
        if next.remove_mame_software_list(authoritative_name).is_none() {
            return;
        }
        match save_managed_dat_sources_to(&self.managed_config_path, &next) {
            Ok(()) => {
                self.managed_sources = next;
                self.managed_statuses.remove(&managed_source_key(
                    &ManagedDatSourceId::mame_software_list(authoritative_name.to_string())
                        .expect("configured MAME source names were validated by core"),
                ));
            }
            Err(error) => self.managed_action_error = Some(error.to_string()),
        }
    }

    fn save_managed_sources(&mut self, next: ManagedDatSources) {
        match save_managed_dat_sources_to(&self.managed_config_path, &next) {
            Ok(()) => self.managed_sources = next,
            Err(error) => self.managed_action_error = Some(error.to_string()),
        }
    }

    fn can_change_managed_sources(&mut self) -> bool {
        self.managed_action_error = None;
        if self.managed_job.is_some() || self.managed_load_error.is_some() {
            if self.managed_load_error.is_some() {
                self.managed_action_error = Some(
                    "Not changing managed sources: their existing configuration could not be read."
                        .to_string(),
                );
            }
            return false;
        }
        true
    }

    fn add_managed_redump_bios(&mut self, system: RedumpBiosSystem) {
        if !self.can_change_managed_sources() {
            return;
        }
        let mut next = self.managed_sources.clone();
        if let Err(error) = next.add_redump_bios(system, ManagedDatUpdatePolicy::Manual) {
            self.managed_action_error = Some(error.to_string());
            return;
        }
        self.save_managed_sources(next);
    }

    fn remove_managed_redump_bios(&mut self, system: RedumpBiosSystem) {
        if !self.can_change_managed_sources() {
            return;
        }
        let mut next = self.managed_sources.clone();
        if next.remove_redump_bios(system).is_none() {
            return;
        }
        let source_id = ManagedDatSourceId::redump_bios(system);
        self.managed_statuses
            .remove(&managed_source_key(&source_id));
        self.save_managed_sources(next);
    }

    fn add_managed_redump_games(&mut self, system: RedumpGameSystem) {
        if !self.can_change_managed_sources() {
            return;
        }
        let mut next = self.managed_sources.clone();
        if let Err(error) = next.add_redump_games(system, ManagedDatUpdatePolicy::Manual) {
            self.managed_action_error = Some(error.to_string());
            return;
        }
        self.save_managed_sources(next);
    }

    fn remove_managed_redump_games(&mut self, system: RedumpGameSystem) {
        if !self.can_change_managed_sources() {
            return;
        }
        let mut next = self.managed_sources.clone();
        if next.remove_redump_games(system).is_none() {
            return;
        }
        let source_id = ManagedDatSourceId::redump_games(system);
        self.managed_statuses
            .remove(&managed_source_key(&source_id));
        self.save_managed_sources(next);
    }

    /// Imports a user-selected, already-extracted release-pack directory.
    /// Inventory is strictly read-only; registration remains a separate,
    /// explicit action after the user has enabled groups.
    fn import_tosec_pack(&mut self, root: PathBuf) {
        self.tosec_action_error = None;
        if self.tosec_load_error.is_some() {
            self.tosec_action_error = Some(
                "Not changing TOSEC release packs: existing pack configuration could not be read."
                    .to_string(),
            );
            return;
        }
        let inventory = match inventory_release_pack(&root) {
            Ok(inventory) => inventory,
            Err(error) => {
                self.tosec_action_error = Some(error.to_string());
                return;
            }
        };
        if self
            .tosec_packs
            .iter()
            .any(|pack| pack.pack_id == inventory.pack_id)
        {
            self.tosec_action_error = Some(
                "This TOSEC release pack is already configured. Remove it first before importing a new inventory."
                    .to_string(),
            );
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let mut next = self.tosec_packs.clone();
        next.push(PersistedTosecPack {
            pack_id: inventory.pack_id,
            root_path: inventory.pack_root,
            imported_unix_seconds: now,
            selections: BTreeSet::new(),
            dats: inventory.dats,
        });
        next.sort_by(|left, right| left.pack_id.cmp(&right.pack_id));
        match save_tosec_packs(&self.tosec_packs_path, &next) {
            Ok(()) => self.tosec_packs = next,
            Err(error) => self.tosec_action_error = Some(error.to_string()),
        }
    }

    /// Removing a pack changes its saved selection/inventory configuration
    /// only. Already registered local DAT entries remain until the user makes
    /// an explicit selection application while the pack still exists.
    fn remove_tosec_pack(&mut self, pack_id: &str) {
        self.tosec_action_error = None;
        if self.tosec_load_error.is_some() {
            self.tosec_action_error = Some(
                "Not changing TOSEC release packs: existing pack configuration could not be read."
                    .to_string(),
            );
            return;
        }
        let mut next = self.tosec_packs.clone();
        let before = next.len();
        next.retain(|pack| pack.pack_id != pack_id);
        if next.len() == before {
            return;
        }
        match save_tosec_packs(&self.tosec_packs_path, &next) {
            Ok(()) => self.tosec_packs = next,
            Err(error) => self.tosec_action_error = Some(error.to_string()),
        }
    }

    fn set_tosec_selection(&mut self, pack_id: &str, key: TosecSelectionKey, enabled: bool) {
        self.tosec_action_error = None;
        if self.tosec_load_error.is_some() {
            self.tosec_action_error = Some(
                "Not changing TOSEC release packs: existing pack configuration could not be read."
                    .to_string(),
            );
            return;
        }
        let mut next = self.tosec_packs.clone();
        let Some(pack) = next.iter_mut().find(|pack| pack.pack_id == pack_id) else {
            return;
        };
        if enabled {
            let selected_group: Vec<_> = pack
                .dats
                .iter()
                .filter(|dat| dat.selection_key() == key)
                .collect();
            if selected_group.is_empty() {
                self.tosec_action_error = Some(
                    "That TOSEC selection group is no longer present in the stored inventory."
                        .to_string(),
                );
                return;
            }
            if selected_group.iter().any(|dat| tosec_dat_is_deferred(dat)) {
                self.tosec_action_error = Some(
                    "TOSEC-ISO and TOSEC-PIX catalogues are deferred by current TOSEC support and cannot be enabled."
                        .to_string(),
                );
                return;
            }
            pack.selections.insert(key);
        } else {
            pack.selections.remove(&key);
        }
        match save_tosec_packs(&self.tosec_packs_path, &next) {
            Ok(()) => self.tosec_packs = next,
            Err(error) => self.tosec_action_error = Some(error.to_string()),
        }
    }

    fn apply_tosec_selection(&mut self, pack_id: &str) {
        self.tosec_action_error = None;
        self.tosec_last_apply = None;
        if self.is_busy() || self.tosec_load_error.is_some() || self.load_error.is_some() {
            self.tosec_action_error = Some(
                "Cannot apply TOSEC selections while a DAT operation is running or the local DAT registry could not be read."
                    .to_string(),
            );
            return;
        }
        if self.is_dirty() {
            self.tosec_action_error = Some(
                "Save or revert local DAT source edits before applying TOSEC selections, so no local changes are lost."
                    .to_string(),
            );
            return;
        }
        let Some(pack) = self.tosec_packs.iter().find(|pack| pack.pack_id == pack_id) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        match apply_selection_to_registry(pack, &self.config_path, now) {
            Ok(outcome) => match load_dat_sources_config_from(&self.config_path) {
                Ok(config) => {
                    let (registry, problems) = DatSourceRegistry::from_config(&config);
                    self.saved = registry.clone();
                    self.draft = registry;
                    self.load_problems = problems;
                    self.tosec_last_apply = Some(TosecApplyView {
                        pack_id: pack_id.to_string(),
                        registered: outcome.registered.len(),
                        already_registered: outcome.already_registered.len(),
                        removed: outcome.removed.len(),
                        deferred: outcome.deferred.len(),
                        conflicts: outcome.conflicts.len(),
                        failed: outcome.failed.len(),
                    });
                    if !outcome.failed.is_empty() {
                        self.tosec_action_error = Some(format!(
                            "{} selected TOSEC DAT(s) were not registered. They remain unregistered; existing known-good entries were kept.",
                            outcome.failed.len()
                        ));
                    } else if !outcome.conflicts.is_empty() {
                        self.tosec_action_error = Some(format!(
                            "{} selected TOSEC DAT(s) conflicted with existing sources; those entries were preserved.",
                            outcome.conflicts.len()
                        ));
                    }
                }
                Err(error) => {
                    self.tosec_action_error = Some(format!(
                        "TOSEC selection was applied, but the local DAT registry could not be reloaded: {error}"
                    ));
                }
            },
            Err(error) => self.tosec_action_error = Some(error.to_string()),
        }
    }

    /// Starts the only network-capable managed-DAT operation. This is called
    /// solely from an explicit page action, never while loading or rendering.
    fn start_managed_dat_operation(
        &mut self,
        source_id: ManagedDatSourceId,
        operation: ManagedDatOperation,
    ) {
        if self.is_busy() || self.managed_load_error.is_some() {
            return;
        }
        let descriptor = match self.managed_sources.descriptors().and_then(|descriptors| {
            descriptors
                .into_iter()
                .find(|descriptor| descriptor.source_id() == &source_id)
                .ok_or_else(|| {
                    archivefs_core::ArchiveFsError::Config(
                        "managed DAT source is not configured".to_string(),
                    )
                })
        }) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                self.managed_action_error = Some(error.to_string());
                return;
            }
        };
        let source_key = managed_source_key(&source_id);
        if operation == ManagedDatOperation::Update
            && !matches!(
                self.managed_statuses.get(&source_key),
                Some(ManagedDatStatusView::UpdateAvailable { .. })
            )
        {
            return;
        }
        self.managed_action_error = None;
        self.managed_statuses.insert(
            source_key,
            match operation {
                ManagedDatOperation::Check => ManagedDatStatusView::Checking,
                ManagedDatOperation::Update => ManagedDatStatusView::Updating,
            },
        );
        let managed_root = self.managed_root.clone();
        let (sender, messages) = sync_channel(1);
        let worker_source_id = source_id.clone();
        std::thread::spawn(move || {
            let now_unix_seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let options = ManagedDatUpdateOptions::new(managed_root, now_unix_seconds);
            let transport = HttpsManagedDatTransport::default();
            let result = match operation {
                ManagedDatOperation::Check => {
                    check_managed_dat_update(&descriptor, &options, &transport)
                }
                ManagedDatOperation::Update => {
                    update_managed_dat(&descriptor, &options, &transport)
                }
            };
            let _ = sender.send(ManagedDatJobMessage {
                source_id: worker_source_id,
                result,
            });
        });
        self.managed_job = Some(RunningManagedDatJob {
            source_id,
            messages,
        });
    }

    /// Drains one terminal managed-DAT operation and refreshes only the
    /// display state. The core operation itself has already atomically
    /// preserved or promoted the snapshot before this point.
    fn poll_managed_dat_job(&mut self) -> bool {
        let Some(job) = self.managed_job.as_mut() else {
            return false;
        };
        match job.messages.try_recv() {
            Ok(message) => {
                let status = match message.result {
                    Ok(outcome) => managed_dat_status_from_outcome(outcome),
                    Err(error) => ManagedDatStatusView::Failed {
                        detail: error.to_string(),
                    },
                };
                self.managed_statuses
                    .insert(managed_source_key(&message.source_id), status);
                self.managed_job = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.managed_statuses.insert(
                    managed_source_key(&job.source_id),
                    ManagedDatStatusView::Failed {
                        detail: "Managed DAT worker stopped before reporting a result".to_string(),
                    },
                );
                self.managed_job = None;
                true
            }
        }
    }

    fn save(&mut self) {
        if self.load_error.is_some() {
            self.save_state = DatSaveState::Failed(
                "Not saving: the existing registry file could not be read, and overwriting it \
                 would discard it."
                    .to_string(),
            );
            return;
        }
        match save_dat_sources_config_to(&self.config_path, &self.draft.to_config()) {
            Ok(()) => {
                self.saved = self.draft.clone();
                self.save_state = DatSaveState::Saved;
            }
            Err(error) => self.save_state = DatSaveState::Failed(error.to_string()),
        }
    }

    fn start_validate(&mut self, id: String) {
        if self.is_busy() {
            return;
        }
        let Some(entry) = self.draft.get(&id).cloned() else {
            return;
        };
        self.action_error = None;
        let (sender, messages) = sync_channel(PROGRESS_QUEUE_DEPTH);
        let cancel = Arc::new(AtomicBool::new(false));
        let limits = self.limits;
        let name = entry.display_name.clone();
        let platform_display = authoritative_platform(&entry);

        std::thread::spawn(move || {
            send_progress(&sender, JobMessage::Progress(format!("Reading {name}…")));
            let report = validate_dat_source(&entry, limits);
            let _ = sender.send(JobMessage::Validated(Box::new(report)));
        });

        self.job = Some(RunningJob {
            kind: JobKind::Validate,
            source_id: id,
            cancel,
            cancel_requested: false,
            messages,
            latest: "Starting…".to_string(),
            started_at: Instant::now(),
            audit_progress: None,
            platform_display,
            bulk: None,
        });
    }

    /// Validates every currently configured source, sequentially, on one
    /// worker thread reusing [`validate_dat_source`] unchanged - the same
    /// authority [`Self::start_validate`] uses, run once per source instead
    /// of once. A no-op while any job is already running, exactly like
    /// [`Self::start_validate`].
    fn start_validate_all(&mut self) {
        if self.is_busy() {
            return;
        }
        self.action_error = None;
        self.last_validate_all_summary = None;
        let entries: Vec<DatSourceEntry> = self.draft.sorted_all().into_iter().cloned().collect();
        let total = entries.len();
        if total == 0 {
            // Nothing to schedule: report the (empty) summary immediately
            // rather than spawning a worker and a job for no sources.
            self.last_validate_all_summary = Some(ValidateAllSummary::default());
            return;
        }
        let (sender, messages) = sync_channel(PROGRESS_QUEUE_DEPTH);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let limits = self.limits;

        std::thread::spawn(move || {
            for entry in entries {
                // Checked between sources, never mid-parse: `validate_dat_source`
                // has no cancellation contract of its own (see the comment on
                // `RunningJobView::cancellable` for a single Validate), so the
                // source already being read always finishes; only sources not
                // yet started are ever skipped.
                if worker_cancel.load(Ordering::Relaxed) {
                    return;
                }
                if sender
                    .send(JobMessage::ValidatingNext {
                        id: entry.id.clone(),
                        display_name: entry.display_name.clone(),
                    })
                    .is_err()
                {
                    // The page dropped the receiver (the job was abandoned) -
                    // stop working rather than validating sources nobody will
                    // ever see the result of.
                    return;
                }
                let report = validate_dat_source(&entry, limits);
                if sender
                    .send(JobMessage::Validated(Box::new(report)))
                    .is_err()
                {
                    return;
                }
            }
            let _ = sender.send(JobMessage::ValidateAllFinished);
        });

        self.job = Some(RunningJob {
            kind: JobKind::ValidateAll,
            source_id: String::new(),
            cancel,
            cancel_requested: false,
            messages,
            latest: format!("Starting… (0 of {total})"),
            started_at: Instant::now(),
            audit_progress: None,
            platform_display: None,
            bulk: Some(BulkValidationProgress {
                total,
                completed: 0,
                valid: 0,
                changed: 0,
                failed: 0,
            }),
        });
    }

    fn start_audit(&mut self, id: String, scan_root: PathBuf) {
        if self.is_busy() {
            return;
        }
        let Some(entry) = self.draft.get(&id).cloned() else {
            return;
        };
        self.audit = None;
        self.identity_enrichment_completed = false;
        self.identity_enrichment = None;
        self.audit_error = None;
        self.audit_elapsed_seconds = None;
        self.rename_plan = None;
        self.rename_plan_error = None;
        self.review_decisions.clear();
        self.abandon_apply_work();
        // Each audit is a generation. A result from an older generation is a
        // stale plan and is dropped, so an old plan can never replace a new one.
        self.audit_generation = self.audit_generation.wrapping_add(1);
        let generation = self.audit_generation;
        let (sender, messages) = sync_channel(PROGRESS_QUEUE_DEPTH);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let trusted = self.trusted.clone();
        let platform_display = authoritative_platform(&entry);
        let canonical_platform = entry
            .platform
            .as_deref()
            .and_then(|platform| archivefs_core::canonical_platform_for_alias(platform));
        // The audit is annotated with the effective DAT policy so multi-
        // candidate verdicts can be shown in the user's preferred order. The
        // policy is resolved now, from the draft, by the core resolver; the
        // worker thread only reads the resolution.
        let policy = resolve(
            self.draft.policy(),
            canonical_platform,
            participating_sources(&self.draft, canonical_platform),
        );
        let request = DatAuditRequest {
            source_id: entry.id.clone(),
            source_display_name: entry.display_name.clone(),
            dat_path: entry.path.clone(),
            dat_kind: entry.kind,
            scan_root,
            limits: self.limits,
            policy: Some(policy),
            platform: canonical_platform.map(str::to_string),
        };
        let database_path = self.database_path.clone();

        std::thread::spawn(move || {
            let report_sender = sender.clone();
            let outcome = run_dat_audit(&request, &trusted, &worker_cancel, &|progress| {
                send_progress(&report_sender, JobMessage::AuditProgress(progress));
            });
            let _ = match outcome {
                Ok(outcome) => {
                    // Cancellation may race with the audit's final comparison.
                    // Re-check at the metadata-write boundary so a result the
                    // page will discard cannot still enrich the catalogue.
                    let enrichment = if worker_cancel.load(Ordering::Acquire) {
                        None
                    } else if let Some(database_path) = database_path {
                        match archivefs_core::Database::open_or_create(&database_path).and_then(
                            |mut database| {
                                database.enrich_platforms_from_dat_audit(&outcome, generation)
                            },
                        ) {
                            Ok(enrichment) => {
                                send_progress(
                                    &sender,
                                    JobMessage::Progress(format!(
                                        "Platform identity enrichment: {} applied, {} already current, {} manual assignment(s) preserved, {} conflict(s) require review.",
                                        enrichment.applied,
                                        enrichment.unchanged,
                                        enrichment.manual_preserved,
                                        enrichment.conflicts,
                                    )),
                                );
                                Some(Box::new(enrichment))
                            }
                            Err(error) => {
                                send_progress(
                                    &sender,
                                    JobMessage::Progress(format!(
                                        "DAT audit completed, but platform identity metadata could not be updated: {error}"
                                    )),
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };
                    // Build the read-only rename plan from the finished audit.
                    // This is cheap (no re-scan, no hashing) but cancellable and
                    // runs on the worker, never on the UI thread.
                    send_progress(
                        &sender,
                        JobMessage::Progress("Building rename plan…".to_string()),
                    );
                    let plan = build_rename_plan(
                        &outcome,
                        &RenamePlanContext { generation },
                        &worker_cancel,
                    )
                    .ok()
                    .map(Box::new);
                    sender.send(JobMessage::Audited {
                        generation,
                        outcome: Box::new(outcome),
                        enrichment,
                        plan,
                    })
                }
                Err(archivefs_core::dat::sources::audit_run::DatAuditError::Cancelled) => {
                    sender.send(JobMessage::Cancelled)
                }
                Err(error) => sender.send(JobMessage::Failed(error.to_string())),
            };
        });

        self.job = Some(RunningJob {
            kind: JobKind::Audit,
            source_id: id,
            cancel,
            cancel_requested: false,
            messages,
            latest: "Starting…".to_string(),
            started_at: Instant::now(),
            audit_progress: Some(AuditProgressTracker::new()),
            platform_display,
            bulk: None,
        });
    }

    /// Starts the normal Identify & Rename audit.  The source list is built
    /// entirely from already-enabled local entries and locally validated
    /// managed *current* snapshots; it performs no check, download, or source
    /// configuration mutation.
    fn start_combined_audit(&mut self, scan_root: PathBuf) {
        if self.is_busy() {
            return;
        }
        let sources = self.combined_audit_sources();
        if sources.is_empty() {
            self.audit_error = Some(
                "No enabled, installed game DAT catalogues are available. Add a local DAT or install a managed game catalogue first."
                    .to_string(),
            );
            return;
        }
        self.audit = None;
        self.identity_enrichment_completed = false;
        self.identity_enrichment = None;
        self.audit_error = None;
        self.audit_elapsed_seconds = None;
        self.rename_plan = None;
        self.rename_plan_error = None;
        self.review_decisions.clear();
        self.abandon_apply_work();
        self.audit_generation = self.audit_generation.wrapping_add(1);
        let generation = self.audit_generation;
        let (sender, messages) = sync_channel(PROGRESS_QUEUE_DEPTH);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let trusted = self.trusted.clone();
        let source_count = sources.len();
        let request = CombinedDatAuditRequest {
            sources,
            scan_root,
            limits: self.limits,
        };

        std::thread::spawn(move || {
            let report_sender = sender.clone();
            let outcome = run_combined_dat_audit(&request, &trusted, &worker_cancel, &|progress| {
                send_progress(&report_sender, JobMessage::AuditProgress(progress));
            });
            let _ = match outcome {
                Ok(outcome) => {
                    send_progress(
                        &sender,
                        JobMessage::Progress("Building combined rename plan…".to_string()),
                    );
                    let plan = build_rename_plan(
                        &outcome,
                        &RenamePlanContext { generation },
                        &worker_cancel,
                    )
                    .ok()
                    .map(Box::new);
                    sender.send(JobMessage::Audited {
                        generation,
                        outcome: Box::new(outcome),
                        enrichment: None,
                        plan,
                    })
                }
                Err(archivefs_core::dat::sources::audit_run::DatAuditError::Cancelled) => {
                    sender.send(JobMessage::Cancelled)
                }
                Err(error) => sender.send(JobMessage::Failed(error.to_string())),
            };
        });

        self.job = Some(RunningJob {
            kind: JobKind::Audit,
            source_id: "all-enabled-evidence".to_string(),
            cancel,
            cancel_requested: false,
            messages,
            latest: format!("Starting with {source_count} evidence catalogue(s)…"),
            started_at: Instant::now(),
            audit_progress: Some(AuditProgressTracker::new()),
            platform_display: None,
            bulk: None,
        });
    }

    fn combined_audit_sources(&self) -> Vec<CombinedDatAuditSource> {
        let mut sources: Vec<CombinedDatAuditSource> = self
            .draft
            .entries()
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| CombinedDatAuditSource {
                source_id: entry.id.clone(),
                source_display_name: entry.display_name.clone(),
                dat_path: entry.path.clone(),
                dat_kind: entry.kind,
                platform: entry
                    .platform
                    .as_deref()
                    .and_then(archivefs_core::canonical_platform_for_alias)
                    .map(str::to_string),
            })
            .collect();

        for config in self.managed_sources.entries() {
            let mut one = ManagedDatSources::new();
            if one
                .add_mame_software_list(config.authoritative_name.clone(), config.update_policy)
                .is_ok()
                && let Ok(mut resolved) = resolve_managed_dat_sources(&one, &self.managed_root)
                && let Some(source) = resolved.pop().and_then(|resolved| resolved.current)
            {
                sources.push(combined_source_from_managed(
                    source,
                    format!("MAME software list: {}", config.authoritative_name),
                ));
            }
        }
        // BIOS DATs are deliberately absent.  They describe firmware, never
        // game identity, and must not enter a rename plan.
        for config in self.managed_sources.redump_games_entries() {
            let mut one = ManagedDatSources::new();
            if one
                .add_redump_games(config.system, config.update_policy)
                .is_ok()
                && let Ok(mut resolved) = resolve_redump_games_sources(&one, &self.managed_root)
                && let Some(source) = resolved.pop().and_then(|resolved| resolved.current)
            {
                sources.push(combined_source_from_managed(
                    source,
                    format!("Redump game DAT: {}", redump_game_label(config.system)),
                ));
            }
        }
        sources.sort_by(|left, right| left.source_id.cmp(&right.source_id));
        sources
    }

    /// The validation report for one source, if it has been validated this
    /// session.
    pub(crate) fn validation(&self, id: &str) -> Option<&DatValidationReport> {
        self.validations.get(id)
    }

    /// Builds the view model. Pure: no I/O beyond a metadata check for
    /// staleness, no clock beyond formatting a stored timestamp (and the
    /// running job's elapsed time, read from the instant the job started).
    pub(crate) fn view(&self) -> DatSourcesPageView {
        let rows: Vec<DatSourceRowView> = self
            .draft
            .sorted_all()
            .into_iter()
            .map(|entry| self.row_view(entry))
            .collect();

        DatSourcesPageView {
            managed_rows: self.managed_rows_view(),
            redump_bios_rows: self.redump_bios_rows_view(),
            redump_game_rows: self.redump_game_rows_view(),
            managed_load_error: self.managed_load_error.clone(),
            managed_action_error: self.managed_action_error.clone(),
            tosec_packs: self.tosec_packs_view(),
            tosec_load_error: self.tosec_load_error.clone(),
            tosec_action_error: self.tosec_action_error.clone(),
            tosec_last_apply: self.tosec_last_apply.clone(),
            no_intro_selected_pack: self.no_intro_selected_pack.as_ref().and_then(|path| {
                std::fs::metadata(path).ok().map(|metadata| {
                    (
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "selected ZIP".to_string()),
                        metadata.len(),
                    )
                })
            }),
            no_intro_inspection: self.no_intro_inspection.clone(),
            no_intro_installed: self.no_intro_installed.clone(),
            no_intro_action_error: self.no_intro_action_error.clone(),
            no_intro_import_status: self.no_intro_import_status,
            unresolved: self
                .draft
                .unresolved_settings()
                .iter()
                .map(|setting: &UnresolvedDatSetting| UnresolvedDatRowView {
                    explanation: setting.describe(),
                })
                .collect(),
            load_problems: self.load_problems.clone(),
            dirty: self.is_dirty(),
            config_path: self.config_path.clone(),
            save_state: self.save_state.clone(),
            load_error: self.load_error.clone(),
            action_error: self.action_error.clone(),
            pending_consequences: self.pending_consequences(&rows),
            background_busy: self.is_busy(),
            last_validate_all_summary: self.last_validate_all_summary,
            running: self.job.as_ref().map(|job| RunningJobView {
                // Empty for `ValidateAll`: there is no one source name to
                // quote in the heading (`RunningJobView::heading` reads this
                // as "no subject" and omits the quoted part). The source
                // currently being read is still shown, in `detail`.
                source_id: match job.kind {
                    JobKind::ValidateAll => String::new(),
                    JobKind::Validate | JobKind::Audit => job.source_id.clone(),
                },
                what: match job.kind {
                    JobKind::Validate => "Validating",
                    JobKind::ValidateAll => "Validating all sources",
                    JobKind::Audit => "Auditing",
                },
                detail: job.latest.clone(),
                // A single Validate is bounded by `DatLimits` and finishes on
                // its own; offering a Cancel that the parser does not check
                // would be a button that lies. `ValidateAll` is different: it
                // is this page's own sequential loop over sources, checked
                // between each one, so Cancel is real here even though the
                // source already being read still finishes.
                cancellable: matches!(job.kind, JobKind::Audit | JobKind::ValidateAll),
                cancellation_requested: job.cancel_requested,
                // The elapsed clock is read here rather than at poll time so
                // the running card keeps ticking between progress messages; it
                // is still a pure function of state with no I/O.
                progress: job
                    .audit_progress
                    .as_ref()
                    .map(|tracker| tracker.view(job.started_at.elapsed().as_secs())),
                platform_display: job.platform_display.clone(),
            }),
            library_folders: self.library_folders.clone(),
            audit: self
                .audit
                .as_ref()
                .map(|outcome| Box::new(audit_view(outcome, self.audit_elapsed_seconds))),
            audit_error: self.audit_error.clone(),
            identity_enrichment: self.identity_enrichment.clone(),
            policy: self.policy_view(),
            rename_plan: self.rename_plan_view(),
            rename_apply: self.rename_apply_view(),
            rows,
        }
    }

    /// Resolves typed managed configuration only against local state/object
    /// files. This uses no transport and never creates state, so rendering the
    /// page cannot check or download anything.
    fn managed_rows_view(&self) -> Vec<ManagedDatSourceRowView> {
        self.managed_sources
            .entries()
            .iter()
            .filter_map(|config| {
                let descriptor = config.descriptor().ok()?;
                // Resolve each configured source independently: malformed or
                // missing local state for one source must not hide another
                // known-good managed snapshot.
                let mut one_source = ManagedDatSources::new();
                if one_source
                    .add_mame_software_list(config.authoritative_name.clone(), config.update_policy)
                    .is_err()
                {
                    return Some(self.managed_row_view_without_state(
                        &descriptor,
                        Some("Managed source configuration is invalid".to_string()),
                    ));
                }
                match resolve_managed_dat_sources(&one_source, &self.managed_root) {
                    Ok(mut sources) => sources.pop().map(|source| {
                        self.managed_row_from_parts(
                            source.descriptor,
                            ManagedDatProvider::MameSoftwareList,
                            format!("System/List: {}", source.config.authoritative_name),
                            true,
                            source.config.update_policy,
                            source.state.as_ref(),
                            source.current.as_ref(),
                            source.previous.as_ref(),
                            None,
                        )
                    }),
                    Err(error) => Some(self.managed_row_view_without_state(
                        &descriptor,
                        Some(format!("Managed state could not be resolved: {error}")),
                    )),
                }
            })
            .collect()
    }

    fn managed_row_from_parts(
        &self,
        descriptor: ManagedDatSourceDescriptor,
        provider: ManagedDatProvider,
        source_label: String,
        configured: bool,
        update_policy: ManagedDatUpdatePolicy,
        state: Option<&ManagedDatState>,
        current: Option<&ManagedDatReadOnlySource>,
        previous: Option<&ManagedDatReadOnlySource>,
        resolution_error: Option<String>,
    ) -> ManagedDatSourceRowView {
        let source_id = descriptor.source_id().clone();
        let key = managed_source_key(&source_id);
        let authoritative_name = if descriptor.expected_softwarelist_name().is_empty() {
            source_label.clone()
        } else {
            descriptor.expected_softwarelist_name().to_string()
        };
        let status = self.managed_statuses.get(&key).cloned().unwrap_or_else(|| {
            if !configured {
                ManagedDatStatusView::NotInstalled
            } else if current.is_some() {
                ManagedDatStatusView::Idle
            } else {
                ManagedDatStatusView::NotInstalled
            }
        });
        ManagedDatSourceRowView {
            source_id: source_id.clone(),
            provider,
            source_label,
            authoritative_name,
            configured,
            update_policy,
            installed: current.is_some(),
            current_revision: state.and_then(|state| state.upstream_revision.clone()),
            last_checked: state
                .and_then(|state| state.last_checked_at_unix_seconds)
                .map(format_unix_timestamp),
            update_enabled: matches!(status, ManagedDatStatusView::UpdateAvailable { .. }),
            busy: self
                .managed_job
                .as_ref()
                .is_some_and(|job| job.source_id == source_id),
            status: resolution_error
                .map_or(status, |detail| ManagedDatStatusView::Failed { detail }),
            technical: ManagedDatTechnicalView {
                sha256: state.map(|state| state.sha256.clone()),
                etag: state.and_then(|state| state.etag.clone()),
                last_modified: state.and_then(|state| state.last_modified.clone()),
                current_path: current.map(|current| current.path().display().to_string()),
                previous_snapshot: state
                    .and_then(|state| state.previous_snapshot.as_ref())
                    .map(|snapshot| snapshot.sha256.clone()),
                previous_path: previous.map(|previous| previous.path().display().to_string()),
            },
        }
    }

    fn managed_row_view_without_state(
        &self,
        descriptor: &ManagedDatSourceDescriptor,
        error: Option<String>,
    ) -> ManagedDatSourceRowView {
        self.managed_row_from_parts(
            descriptor.clone(),
            ManagedDatProvider::MameSoftwareList,
            format!("System/List: {}", descriptor.expected_softwarelist_name()),
            true,
            ManagedDatUpdatePolicy::Manual,
            None,
            None,
            None,
            error,
        )
    }

    fn redump_bios_rows_view(&self) -> Vec<ManagedDatSourceRowView> {
        [
            RedumpBiosSystem::PlayStation,
            RedumpBiosSystem::PlayStation2,
            RedumpBiosSystem::Xbox,
        ]
        .into_iter()
        .map(|system| self.redump_bios_row_view(system))
        .collect()
    }

    fn redump_bios_row_view(&self, system: RedumpBiosSystem) -> ManagedDatSourceRowView {
        let descriptor = ManagedDatSourceDescriptor::redump_bios(system)
            .expect("closed Redump BIOS systems must have valid descriptors");
        let configured = self
            .managed_sources
            .redump_bios_entries()
            .iter()
            .find(|entry| entry.system == system);
        let policy = configured
            .map(|entry| entry.update_policy)
            .unwrap_or(ManagedDatUpdatePolicy::Manual);
        let mut one = ManagedDatSources::new();
        if configured.is_some() && one.add_redump_bios(system, policy).is_ok() {
            match resolve_redump_bios_sources(&one, &self.managed_root) {
                Ok(mut sources) => {
                    let source = sources.pop().expect("one configured Redump BIOS source");
                    return self.managed_row_from_parts(
                        source.descriptor,
                        ManagedDatProvider::RedumpBios,
                        format!("System: {}", redump_bios_label(system)),
                        true,
                        policy,
                        source.state.as_ref(),
                        source.current.as_ref(),
                        source.previous.as_ref(),
                        None,
                    );
                }
                Err(error) => {
                    return self.managed_row_from_parts(
                        descriptor,
                        ManagedDatProvider::RedumpBios,
                        format!("System: {}", redump_bios_label(system)),
                        true,
                        policy,
                        None,
                        None,
                        None,
                        Some(format!("Managed state could not be resolved: {error}")),
                    );
                }
            }
        }
        self.managed_row_from_parts(
            descriptor,
            ManagedDatProvider::RedumpBios,
            format!("System: {}", redump_bios_label(system)),
            false,
            policy,
            None,
            None,
            None,
            None,
        )
    }

    fn redump_game_rows_view(&self) -> Vec<ManagedDatSourceRowView> {
        [
            RedumpGameSystem::PlayStation,
            RedumpGameSystem::PlayStation2,
            RedumpGameSystem::Xbox,
        ]
        .into_iter()
        .map(|system| self.redump_game_row_view(system))
        .collect()
    }

    fn redump_game_row_view(&self, system: RedumpGameSystem) -> ManagedDatSourceRowView {
        let descriptor = ManagedDatSourceDescriptor::redump_games(system)
            .expect("closed Redump game systems must have valid descriptors");
        let configured = self
            .managed_sources
            .redump_games_entries()
            .iter()
            .find(|entry| entry.system == system);
        let policy = configured
            .map(|entry| entry.update_policy)
            .unwrap_or(ManagedDatUpdatePolicy::Manual);
        let mut one = ManagedDatSources::new();
        if configured.is_some() && one.add_redump_games(system, policy).is_ok() {
            match resolve_redump_games_sources(&one, &self.managed_root) {
                Ok(mut sources) => {
                    let source = sources.pop().expect("one configured Redump game source");
                    return self.managed_row_from_parts(
                        source.descriptor,
                        ManagedDatProvider::RedumpGames,
                        format!("System: {}", redump_game_label(system)),
                        true,
                        policy,
                        source.state.as_ref(),
                        source.current.as_ref(),
                        source.previous.as_ref(),
                        None,
                    );
                }
                Err(error) => {
                    return self.managed_row_from_parts(
                        descriptor,
                        ManagedDatProvider::RedumpGames,
                        format!("System: {}", redump_game_label(system)),
                        true,
                        policy,
                        None,
                        None,
                        None,
                        Some(format!("Managed state could not be resolved: {error}")),
                    );
                }
            }
        }
        self.managed_row_from_parts(
            descriptor,
            ManagedDatProvider::RedumpGames,
            format!("System: {}", redump_game_label(system)),
            false,
            policy,
            None,
            None,
            None,
            None,
        )
    }

    fn tosec_packs_view(&self) -> Vec<TosecPackView> {
        self.tosec_packs.iter().map(tosec_pack_view).collect()
    }

    /// Builds the apply/recovery view. Pure: it renders the core's built
    /// transaction, the worker's outcome, and the recovered journals.
    fn rename_apply_view(&self) -> RenameApplyView {
        let review = self.apply_review.as_ref().map(|transaction| {
            let count = transaction.entries.len();
            ApplyReviewView {
                transaction: transaction.clone(),
                rows: transaction
                    .entries
                    .iter()
                    .map(|entry| ApplyReviewRow {
                        current_basename: entry.original_basename.clone(),
                        proposed_basename: entry.proposed_basename.clone(),
                    })
                    .collect(),
                trusted_root: self
                    .trusted
                    .roots()
                    .first()
                    .map(|root| root.to_string_lossy().into_owned()),
                required_phrase: (count > TYPED_CONFIRMATION_THRESHOLD)
                    .then(|| typed_confirmation_phrase(count)),
            }
        });
        let outcome = self.apply_outcome.as_ref().map(|outcome| ApplyOutcomeView {
            transaction_id: outcome.transaction.transaction_id.clone(),
            state: outcome.transaction.state,
            requested: outcome.summary.requested,
            applied: outcome.summary.applied,
            skipped: outcome.summary.skipped,
            failed: outcome.summary.failed,
            rows: outcome
                .transaction
                .entries
                .iter()
                .map(|entry| ApplyRowView {
                    current_basename: entry.original_basename.clone(),
                    proposed_basename: entry.proposed_basename.clone(),
                    state: entry.state,
                    failure_reason: entry.failure_reason.clone(),
                })
                .collect(),
        });
        let rollback_result = self.rollback_result.as_ref().map(|result| match result {
            RollbackResult::FullyRolledBack => RollbackResultView {
                label: "Fully rolled back",
                detail: "every applied rename was reversed and confirmed.".to_string(),
            },
            RollbackResult::PartiallyRolledBack {
                rolled_back,
                failed,
            } => RollbackResultView {
                label: "Partially rolled back",
                detail: format!(
                    "{} rename(s) reversed; {} could not be: {}",
                    rolled_back.len(),
                    failed.len(),
                    failed
                        .iter()
                        .map(|(path, reason)| format!("{} ({reason})", path.display()))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            },
            RollbackResult::RollbackFailed { failed } => RollbackResultView {
                label: "Rollback failed",
                detail: failed
                    .iter()
                    .map(|(path, reason)| format!("{} ({reason})", path.display()))
                    .collect::<Vec<_>>()
                    .join("; "),
            },
        });
        let recovery = self
            .recovery_transactions
            .iter()
            .map(|transaction| RecoveryTransactionView {
                transaction_id: transaction.transaction_id.clone(),
                state: transaction.state,
                applied_count: transaction.applied_count(),
                total_count: transaction.entries.len(),
                human_summary: rename_transaction_human_summary(&transaction.entries),
                source_scan_root: transaction.source_scan_root.clone(),
                resolution: transaction.recovery_resolution,
            })
            .collect();
        RenameApplyView {
            review,
            outcome,
            apply_error: self.apply_error.clone(),
            subset_available: self.subset_available,
            rollback_result,
            rollback_error: self.rollback_error.clone(),
            apply_running: self.apply_running,
            rollback_running: self.rollback_running,
            recovery,
            journal_dir: self.transaction_dir.to_string_lossy().into_owned(),
            recovery_resolution_error: self.recovery_resolution_error.clone(),
        }
    }

    /// Builds the rename-planning section view from the stored plan and the
    /// session-only review decisions. Renders the core's output; nothing here
    /// re-derives or re-ranks.
    fn rename_plan_view(&self) -> Option<RenamePlanView> {
        let Some(plan) = self.rename_plan.as_ref() else {
            // A plan build that failed is still worth showing, so the user
            // learns why rather than the section silently vanishing.
            return self.rename_plan_error.as_ref().map(|error| RenamePlanView {
                generation: self.audit_generation,
                scan_root: String::new(),
                scan_root_short: String::new(),
                platform_display: None,
                source_display_name: String::new(),
                counts: archivefs_core::dat::rename_plan::RenamePlanCounts::default(),
                audited_total: 0,
                verified_total: 0,
                truncated: false,
                rows: Vec::new(),
                error: Some(error.clone()),
            });
        };
        let rows = plan
            .proposals
            .iter()
            .map(|proposal| RenamePlanRowView {
                source_path: proposal.source_path.clone(),
                current_basename: proposal.current_basename.clone(),
                proposed_basename: proposal.proposed_basename.clone(),
                platform_display: proposal.platform_display.clone(),
                source_display_name: proposal.source_display_name.clone(),
                game_name: proposal.game_name.clone(),
                rom_name: proposal.rom_name.clone(),
                verdict_label: proposal.verdict_label.clone(),
                content: content_technical_view(
                    &proposal.content_classification,
                    &proposal.original_metadata,
                ),
                state: proposal.state,
                object_kind_label: proposal.object_kind.label(),
                explanations: proposal.explanations.clone(),
                ambiguity_reason: proposal.ambiguity_reason.clone(),
                collision_detail: proposal.collision.as_ref().map(|collision| {
                    if collision.colliding_is_symlink {
                        format!("{} (the colliding path is a symlink)", collision.detail)
                    } else {
                        collision.detail.clone()
                    }
                }),
                blockers: proposal.blockers.clone(),
                extension_preserved: proposal.extension_status
                    == Some(archivefs_core::dat::rename_plan::ExtensionStatus::Preserved),
                sanitisation_notes: proposal.sanitisation_notes.clone(),
                decision: self
                    .review_decisions
                    .get(&proposal.source_path.to_string_lossy().into_owned())
                    .copied(),
            })
            .collect();
        Some(RenamePlanView {
            generation: plan.generation,
            scan_root: plan.scan_root.clone(),
            scan_root_short: shorten_path(&plan.scan_root),
            platform_display: plan.platform_display.clone(),
            source_display_name: plan.source_display_name.clone(),
            counts: plan.counts,
            audited_total: plan.audited_total,
            verified_total: plan.verified_total,
            truncated: plan.truncated,
            rows,
            error: self.rename_plan_error.clone(),
        })
    }

    /// Builds the DAT Matching Policy section view, resolving the effective
    /// policy for the scope being edited through the core resolver.
    fn policy_view(&self) -> DatPolicyView {
        // The scope the user selected can outlive the source whose platform
        // offered it: if that platform is no longer available (every source
        // for it removed), fall back to the global scope rather than showing
        // a scope with nothing to inspect.
        let scope = match &self.policy_scope {
            Some(platform) if self.scope_is_available(platform) => Some(platform.clone()),
            _ => None,
        };
        let config = self.draft.policy();
        let effective = resolve(
            config,
            scope.as_deref(),
            participating_sources(&self.draft, scope.as_deref()),
        );

        let scopes_available = self.policy_scopes_available();
        let scope_label = match &scope {
            None => "All platforms".to_string(),
            Some(platform) => archivefs_core::platform::display_name_for(platform).to_string(),
        };

        // The lists shown for editing are the scope's *authored* lists - what
        // is actually persisted there - not the resolved ones. The Effective
        // Policy Summary below is where the resolved (possibly inherited)
        // values appear, so editing a platform scope starts from an empty
        // override and never silently copies the whole global list into it.
        let region_preferences = authored_region_list(config, &scope)
            .into_iter()
            .filter_map(|value| {
                RegionId::parse(&value).map(|region| PolicyPreferenceRowView {
                    value: region.as_str().to_string(),
                    label: region.label().to_string(),
                    position: 0,
                })
            })
            .enumerate()
            .map(|(index, mut row)| {
                row.position = index + 1;
                row
            })
            .collect();
        let language_preferences = authored_language_list(config, &scope)
            .into_iter()
            .filter_map(|value| {
                LanguagePreference::parse(&value).map(|preference| PolicyPreferenceRowView {
                    value: preference.as_str().to_string(),
                    label: preference.label().to_string(),
                    position: 0,
                })
            })
            .enumerate()
            .map(|(index, mut row)| {
                row.position = index + 1;
                row
            })
            .collect();

        let problems = validate_policy_config(config)
            .into_iter()
            .map(|problem| problem.message)
            .collect();

        let summary = effective_summary_view(&effective, &scope);
        DatPolicyView {
            scope,
            scope_label,
            scopes_available,
            region_preferences,
            language_preferences,
            revision_policy: effective.revision_policy,
            clone_policy: effective.clone_policy,
            content_selection: effective.content_selection,
            effective: summary,
            problems,
            editable: self.load_error.is_none(),
        }
    }

    /// Whether `platform` is still offered as a policy scope: some source is
    /// assigned to it.
    fn scope_is_available(&self, platform: &str) -> bool {
        self.draft.entries().iter().any(|entry| {
            entry
                .platform
                .as_deref()
                .and_then(archivefs_core::canonical_platform_for_alias)
                == Some(platform)
        })
    }

    /// The platforms a policy scope can target: every platform some source is
    /// assigned to, canonicalised, deduplicated, sorted by display name.
    fn policy_scopes_available(&self) -> Vec<PolicyScopeOption> {
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        for entry in self.draft.entries() {
            if let Some(platform) = &entry.platform
                && let Some(canonical) = archivefs_core::canonical_platform_for_alias(platform)
            {
                seen.insert(canonical.to_string(), canonical.to_string());
            }
        }
        let mut options: Vec<PolicyScopeOption> = seen
            .into_values()
            .map(|platform| PolicyScopeOption {
                id: Some(platform.clone()),
                label: archivefs_core::platform::display_name_for(&platform).to_string(),
            })
            .collect();
        options.sort_by(|a, b| a.label.cmp(&b.label));
        options
    }

    fn row_view(&self, entry: &DatSourceEntry) -> DatSourceRowView {
        let saved = self.saved.get(&entry.id);
        let changed = match saved {
            None => true,
            Some(saved) => saved != entry,
        };
        let validation = self.validation(&entry.id);
        // Read the cached groups when the report landed through a validation
        // run; fall back to deriving them on demand so test-injected reports
        // (which bypass the poll path) still render.
        let groups = self
            .diagnostic_groups
            .get(&entry.id)
            .cloned()
            .unwrap_or_else(|| {
                validation
                    .map(|report| group_diagnostics(&entry.id, report))
                    .unwrap_or_default()
            });
        let incomplete_load = validation.is_some_and(|report| report.truncated);
        let dat_files_read = incomplete_load.then(|| validation.unwrap().files.len() as u64);
        let dat_files_total = incomplete_load
            .then(|| {
                validation
                    .and_then(|report| report.total_dat_files)
                    .map(|n| n as u64)
            })
            .flatten();
        DatSourceRowView {
            id: entry.id.clone(),
            display_name: entry.display_name.clone(),
            path: entry.path.to_string_lossy().into_owned(),
            kind_label: entry.kind.label(),
            enabled: entry.enabled,
            platform_display: entry.platform_display(),
            platform_id: entry.platform.clone(),
            platform_unresolved: !entry.platform_is_resolved(),
            formats: entry.health.formats.clone().unwrap_or_default(),
            health_state: entry.health.state(),
            health_detail: entry.health.detail.clone(),
            last_validated: entry
                .health
                .last_validated_unix_seconds
                .map(format_unix_timestamp),
            health_stale: entry.health.is_stale_for(&entry.path, entry.kind),
            entry_count: entry.health.entry_count,
            rom_count: entry.health.rom_count,
            changed,
            busy: self
                .job
                .as_ref()
                .is_some_and(|job| job.source_id == entry.id),
            detail: self.validation(&entry.id).map(inspect_view),
            groups,
            incomplete_load,
            dat_files_read,
            dat_files_total,
            // The full warning details are kept inline on this card; nothing is
            // recorded in History & Logs today, so nothing points there.
            history_link_available: false,
        }
    }

    /// Plain-language description of what saving would do, one line per change.
    fn pending_consequences(&self, rows: &[DatSourceRowView]) -> Vec<String> {
        if !self.is_dirty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for row in rows.iter().filter(|row| row.changed) {
            match self.saved.get(&row.id) {
                None => out.push(format!(
                    "'{}' will be registered, pointing at {}.",
                    row.display_name, row.path
                )),
                Some(saved) => {
                    if saved.enabled != row.enabled {
                        out.push(if row.enabled {
                            format!("'{}' will be used again.", row.display_name)
                        } else {
                            format!(
                                "'{}' will no longer be used. It stays registered.",
                                row.display_name
                            )
                        });
                    }
                    if saved.platform != self.draft.get(&row.id).and_then(|e| e.platform.clone()) {
                        out.push(match &row.platform_display {
                            Some(platform) => format!(
                                "'{}' will be treated as a {platform} catalogue.",
                                row.display_name
                            ),
                            None => format!(
                                "'{}' will no longer be tied to one platform.",
                                row.display_name
                            ),
                        });
                    }
                    if saved.health
                        != self
                            .draft
                            .get(&row.id)
                            .map(|e| e.health.clone())
                            .unwrap_or_default()
                    {
                        out.push(format!(
                            "'{}' will record the result of the check just run.",
                            row.display_name
                        ));
                    }
                }
            }
        }
        // Removals: named from the saved side, since they are gone from the draft.
        for saved in self.saved.sorted_all() {
            if self.draft.get(&saved.id).is_none() {
                out.push(format!(
                    "'{}' will be removed from the registry. The file at {} is not deleted.",
                    saved.display_name,
                    saved.path.display()
                ));
            }
        }
        if out.is_empty() {
            out.push("The registry will be rewritten with your changes.".to_string());
        }
        out
    }
}

fn combined_source_from_managed(
    source: ManagedDatReadOnlySource,
    source_display_name: String,
) -> CombinedDatAuditSource {
    CombinedDatAuditSource {
        source_id: format!(
            "managed:{:?}:{}",
            source.source_id().provider,
            source.source_id().source_key
        ),
        source_display_name,
        dat_path: source.path().to_path_buf(),
        dat_kind: DatSourceKind::File,
        platform: None,
    }
}

/// Turns a validation report into the Inspect panel's rows.
fn inspect_view(report: &DatValidationReport) -> InspectView {
    InspectView {
        files: report
            .files
            .iter()
            .map(|file| match &file.outcome {
                DatFileOutcome::Parsed {
                    format,
                    ecosystem,
                    name,
                    version,
                    entry_count,
                    rom_count,
                    diagnostics,
                } => {
                    let errors: Vec<String> = diagnostics
                        .iter()
                        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
                        .map(|diagnostic| diagnostic.message.clone())
                        .collect();
                    let warnings: Vec<String> = diagnostics
                        .iter()
                        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
                        .map(|diagnostic| diagnostic.message.clone())
                        .collect();
                    let notes: Vec<String> = diagnostics
                        .iter()
                        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Note)
                        .map(|diagnostic| diagnostic.message.clone())
                        .collect();
                    InspectFileView {
                        file_name: file.file_name.clone(),
                        status: if !errors.is_empty() {
                            "Failed"
                        } else if warnings.is_empty() {
                            "OK"
                        } else {
                            "OK, with warnings"
                        },
                        detail: {
                            let mut parts = vec![
                                format.label().to_string(),
                                ecosystem.label().to_string(),
                                format!("{entry_count} entries, {rom_count} ROMs"),
                            ];
                            if let Some(name) = name {
                                parts.insert(
                                    0,
                                    match version {
                                        Some(version) => format!("{name} ({version})"),
                                        None => name.clone(),
                                    },
                                );
                            }
                            parts.join(" · ")
                        },
                        errors,
                        warnings,
                        notes,
                    }
                }
                DatFileOutcome::Failed { error } => InspectFileView {
                    file_name: file.file_name.clone(),
                    status: "Failed",
                    detail: error.clone(),
                    errors: Vec::new(),
                    warnings: Vec::new(),
                    notes: Vec::new(),
                },
            })
            .collect(),
        duplicate_identities: report
            .duplicate_identities
            .iter()
            .map(|duplicate| {
                format!(
                    "'{}' is claimed by {}",
                    duplicate.identity,
                    duplicate.file_names.join(" and ")
                )
            })
            .collect(),
        skipped: report
            .skipped
            .iter()
            .map(|skipped| format!("{}: {}", skipped.file_name, skipped.reason))
            .collect(),
        truncated: report.truncated,
    }
}

fn describe(progress: &DatAuditProgress) -> String {
    match progress {
        DatAuditProgress::ReadingCatalogue { file_name } => {
            format!("Reading catalogue {file_name}…")
        }
        DatAuditProgress::CatalogueReady { entries, roms } => {
            format!("Catalogue ready: {entries} entries, {roms} ROMs")
        }
        DatAuditProgress::Scanning {
            files_found,
            current_dir,
        } => match current_dir {
            // The full directory is never put into the detail line: only a
            // shortened form, so no private path leaks into text that could be
            // logged.
            Some(dir) => format!(
                "Looking for files… {files_found} so far · in {}",
                shorten_path(dir)
            ),
            None => format!("Looking for files… {files_found} so far"),
        },
        DatAuditProgress::Hashing {
            index,
            total,
            file_name,
        } => format!("Checking {index} of {total}: {file_name}"),
        DatAuditProgress::Comparing { files } => {
            format!("Comparing {files} files against the catalogue…")
        }
    }
}

/// Turns a core outcome into rows, without adding or merging any category. The
/// in-memory outcome is the only input; nothing is re-scanned to build this.
fn audit_view(outcome: &DatAuditOutcome, elapsed_seconds: Option<u64>) -> AuditResultView {
    let summary = &outcome.report.summary;
    // Every category the core counts, each with the meaning the core documents
    // for it. Zero counts are kept: "0 ambiguous" is a result, and hiding it
    // would make the reader wonder whether it was checked.
    let categories = vec![
        AuditCategoryView {
            label: "Exact",
            count: summary.exact,
            meaning: "A cryptographic hash (SHA-256, SHA-1 or MD5) matched exactly one catalogue entry.",
        },
        AuditCategoryView {
            label: "Exact (multiple)",
            count: summary.exact_multiple,
            meaning: "A cryptographic hash matched several catalogue entries; all are listed.",
        },
        AuditCategoryView {
            label: "Probable",
            count: summary.probable,
            meaning: "CRC32 (with size, where known) matched one entry. A 32-bit checksum is weaker evidence than a hash.",
        },
        AuditCategoryView {
            label: "Probable (multiple)",
            count: summary.probable_multiple,
            meaning: "CRC32 matched several entries. Deliberately not called exact: a 32-bit collision is as likely as a real duplicate.",
        },
        AuditCategoryView {
            label: "Filename only",
            count: summary.filename_only,
            meaning: "The name is in the catalogue and no hash was available. This says a name matched, not that this file did.",
        },
        AuditCategoryView {
            label: "Ambiguous",
            count: summary.ambiguous,
            meaning: "Candidates exist but the evidence disagrees - for example a CRC32 match whose size does not fit.",
        },
        AuditCategoryView {
            label: "Not in catalogue",
            count: summary.not_in_dat,
            meaning: "Hashes were compared and matched nothing. The file is not in this catalogue.",
        },
        AuditCategoryView {
            label: "No usable evidence",
            count: summary.no_evidence,
            meaning: "No hash could be compared and the name matched nothing.",
        },
    ];

    let content_by_path: std::collections::HashMap<_, _> = outcome
        .content
        .matches
        .iter()
        .map(|matched| (matched.local_path.as_str(), matched))
        .collect();
    let evidence_by_path: std::collections::HashMap<_, Vec<_>> = outcome
        .evidence_sources
        .iter()
        .fold(std::collections::HashMap::new(), |mut by_path, evidence| {
            by_path
                .entry(evidence.local_path.as_str())
                .or_default()
                .push(evidence.source_display_name.as_str());
            by_path
        });
    let entries: Vec<AuditEntryView> = outcome
        .report
        .entries
        .iter()
        .take(MAX_AUDIT_ENTRIES_SHOWN)
        .map(|entry| {
            let content = content_by_path
                .get(entry.local_path.as_str())
                .map(|matched| {
                    matched
                        .candidates
                        .iter()
                        .map(|candidate| {
                            content_technical_view(
                                &candidate.classification,
                                &candidate.original_metadata,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            AuditEntryView {
                file_name: entry.local_filename.clone(),
                verdict: entry.verdict.label(),
                detail: verdict_detail(&entry.verdict),
                evidence_sources: evidence_by_path
                    .get(entry.local_path.as_str())
                    .map(|sources| {
                        let mut sources = sources.clone();
                        sources.sort_unstable();
                        sources.dedup();
                        sources.into_iter().map(str::to_string).collect()
                    })
                    .unwrap_or_default(),
                content,
            }
        })
        .collect();
    let entries_truncated = outcome.report.entries.len().saturating_sub(entries.len());

    AuditResultView {
        source_display_name: outcome.source_display_name.clone(),
        source_id: outcome.source_id.clone(),
        dat_path: outcome.dat_path.clone(),
        scan_root: outcome.scan_root.clone(),
        scan_root_short: shorten_path(&outcome.scan_root),
        catalogue_names: outcome.catalogue_names.clone(),
        catalogue_entries: outcome.catalogue_entries,
        headline: outcome.headline(),
        elapsed_seconds,
        categories,
        entries,
        entries_truncated,
        archives: outcome
            .archives
            .iter()
            .map(|archive| ArchiveAuditView {
                archive_name: archive
                    .archive_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| archive.archive_path.display().to_string()),
                completion: archive_completion_label(&archive.completion),
                members: archive
                    .members
                    .iter()
                    .map(|member| ArchiveMemberAuditView {
                        index: member.evidence.index,
                        name: member.evidence.member_name_display.clone(),
                        status: archive_member_status_label(&member.evidence.status),
                        verdict: member
                            .verdict
                            .as_ref()
                            .map(|verdict| verdict.label().to_string()),
                        detail: member
                            .verdict
                            .as_ref()
                            .map(verdict_detail)
                            .unwrap_or_default(),
                        evidence_sources: {
                            let mut sources = member
                                .evidence_sources
                                .iter()
                                .map(|source| source.source_display_name.clone())
                                .collect::<Vec<_>>();
                            sources.sort();
                            sources.dedup();
                            sources
                        },
                    })
                    .collect(),
            })
            .collect(),
        unhashed: outcome
            .unhashed
            .iter()
            .map(|file| (file.file_name.clone(), file.detail.clone()))
            .collect(),
        unreadable_catalogues: outcome.unreadable_catalogues.clone(),
        truncated: outcome.truncated,
        files_scanned: outcome.files_scanned,
        content_selection: outcome.content.selection,
        content_summary: outcome.content.catalogue,
        policy: outcome.policy.as_ref().map(audit_policy_view),
        completion: dat_completion_view(outcome),
    }
}

fn archive_completion_label(
    completion: &archivefs_core::dat::archive::ArchivePassCompletion,
) -> String {
    use archivefs_core::dat::archive::{ArchivePassCompletion, ArchivePassStopReason};
    match completion {
        ArchivePassCompletion::Complete => "Complete archive pass".to_string(),
        ArchivePassCompletion::Incomplete { reason } => match reason {
            ArchivePassStopReason::Cancelled => "Incomplete: cancelled".to_string(),
            ArchivePassStopReason::MemberRefused { index, .. } => {
                format!("Incomplete: stopped at member #{index}")
            }
            ArchivePassStopReason::RunLogicalBudget => {
                "Incomplete: audit decode budget reached".to_string()
            }
            ArchivePassStopReason::OuterFileChanged => {
                "Incomplete: archive changed while it was read".to_string()
            }
            ArchivePassStopReason::SourceError { detail } => {
                format!("Incomplete: {detail}")
            }
        },
    }
}

fn archive_member_status_label(
    status: &archivefs_core::dat::archive::ArchiveMemberStatus,
) -> String {
    use archivefs_core::dat::archive::ArchiveMemberStatus;
    match status {
        ArchiveMemberStatus::HashComplete => "Decoded + hashed".to_string(),
        ArchiveMemberStatus::EmptyFile => "Empty member".to_string(),
        ArchiveMemberStatus::NestedArchive => "Nested archive refused".to_string(),
        ArchiveMemberStatus::Encrypted => "Encrypted member refused".to_string(),
        ArchiveMemberStatus::UnsupportedCodec { method } => {
            format!("Unsupported codec: {method}")
        }
        ArchiveMemberStatus::RefusedLimits { reason } => format!("Refused: {reason}"),
        ArchiveMemberStatus::Corrupt { detail } => format!("Corrupt: {detail}"),
        ArchiveMemberStatus::NotVerified { reason } => format!("Not verified: {reason}"),
    }
}

fn content_technical_view(
    classification: &DatContentClassification,
    metadata: &archivefs_core::dat::classification::DatOriginalMetadata,
) -> ContentTechnicalView {
    ContentTechnicalView {
        classification: classification.class.label().to_string(),
        confidence: classification.confidence.label().to_string(),
        evidence: classification
            .evidence
            .iter()
            .map(|item| {
                let upstream = item
                    .original_value
                    .as_deref()
                    .map(|value| format!(" — upstream: {value}"))
                    .unwrap_or_default();
                format!("{} ({}){upstream}", item.kind.label(), item.rule)
            })
            .collect(),
        original_metadata: metadata
            .fields
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        classifier_version: classification.classifier_version.clone(),
    }
}

/// Turns the core policy annotation into rows, without re-ranking anything:
/// the resolution the core produced is rendered as-is.
fn audit_policy_view(
    policy: &archivefs_core::dat::sources::audit_run::DatAuditPolicyOutcome,
) -> AuditPolicyView {
    let mut notes = Vec::new();
    for note in policy.notes.iter() {
        notes.push(AuditPolicyNoteView {
            file_name: std::path::Path::new(&note.local_path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| note.local_path.clone()),
            verdict_label: note.verdict_label.clone(),
            ranked: note
                .resolution
                .entries
                .iter()
                .map(|entry| format!("{}. {}", entry.position, entry.candidate.label()))
                .collect(),
            explanations: note.resolution.explanations.clone(),
            decided: note.resolution.decided,
            winner: note
                .resolution
                .winner_index
                .and_then(|index| note.resolution.entries.get(index))
                .map(|entry| entry.candidate.label()),
            ambiguous: note.resolution.ambiguous,
            ambiguity_reason: note.resolution.ambiguity_reason.clone(),
        });
    }
    AuditPolicyView {
        source_ordering: policy.source_ordering.clone(),
        notes,
        notes_truncated: None,
    }
}

fn verdict_detail(verdict: &archivefs_core::dat::audit::AuditVerdict) -> String {
    use archivefs_core::dat::audit::AuditVerdict as V;
    match verdict {
        V::Exact {
            game_name,
            algorithm,
            ..
        } => format!("{game_name} ({algorithm})"),
        V::ExactMultipleCandidates {
            algorithm,
            count,
            game_names,
        }
        | V::ProbableMultipleCandidates {
            algorithm,
            count,
            game_names,
        } => format!(
            "{count} candidates by {algorithm}: {}",
            game_names.join(", ")
        ),
        V::Probable { game_name, .. } | V::FilenameOnly { game_name, .. } => game_name.clone(),
        V::Ambiguous { detail } => detail.clone(),
        V::NotInDat | V::NoUsableEvidence => String::new(),
    }
}

/// A stored Unix timestamp as a date and time, in UTC.
///
/// Hand-rolled rather than pulling in a date library for one label: the build
/// has no date dependency, and the only requirement here is that the value be
/// readable and unambiguous.
fn format_unix_timestamp(seconds: u64) -> String {
    let days_total = (seconds / 86_400) as i64;
    let time_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days_total);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60
    )
}

/// Days since 1970-01-01 to a civil date. Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// Unsubmitted UI state
// ---------------------------------------------------------------------------

/// Text and disclosures that are not policy.
///
/// Deliberately not part of [`DatSourcesPageState`]: an open picker and a
/// half-chosen audit folder are not preferences, and neither belongs in
/// something whose difference from disk defines the unsaved-change state.
#[derive(Default)]
pub(crate) struct DatSourcesPageUi {
    /// Which source's detail disclosure is open.
    pub(crate) open_inspect: Option<String>,
    /// Which source's platform picker is open.
    pub(crate) open_platform_picker: Option<String>,
    pub(crate) platform_query: String,
    /// Which source's audit target chooser is open.
    pub(crate) open_audit_picker: Option<String>,
    /// Whether the normal all-enabled Identify & Rename target chooser is
    /// open. It is transient UI only and never starts a scan by itself.
    pub(crate) open_combined_audit_picker: bool,
    pub(crate) quick_review_open: bool,
    /// Whether Quick Rename's success summary is showing the itemized
    /// per-file "View details" disclosure. Reset whenever a new apply
    /// outcome replaces the one it was open for.
    pub(crate) quick_success_details_open: bool,
    /// Which source is awaiting removal confirmation.
    pub(crate) confirm_remove: Option<String>,
    /// Typed MAME software-list name for the deliberately narrow managed
    /// source add flow. There is intentionally no URL/provider field.
    pub(crate) managed_mame_name: String,
    pub(crate) confirm_remove_managed: Option<ManagedDatSourceId>,
    pub(crate) open_managed_technical: Option<ManagedDatSourceId>,
    /// Per-pack free-text group filter. It limits rendering only; persisted
    /// selection remains the typed System/Category/Media key.
    pub(crate) tosec_group_filter: BTreeMap<String, String>,
    pub(crate) show_tosec_raw: BTreeSet<String>,
    /// Which diagnostic group's drill-down is open, as the group's stable id.
    /// One group expands at a time; expanding another collapses this one.
    pub(crate) open_diagnostic: Option<String>,
    /// Which rename-plan rows' review choices are open (source path).
    pub(crate) plan_review_open: Option<String>,
    /// Which rename-plan filter is active.
    pub(crate) plan_filter: RenamePlanFilter,
    /// 0-based page of the rename-plan rows list currently shown, so a
    /// large plan (Game Boy's proven 1839 actionable entries) never
    /// renders every row of the current filter's list at once. Selection
    /// itself (`review_decisions`, on [`DatSourcesPageState`]) is keyed by
    /// path and is unaffected by which page is on screen.
    pub(crate) plan_page: usize,
    /// The typed confirmation phrase for a large apply batch.
    pub(crate) plan_typed_confirmation: String,
}

impl DatSourcesPageUi {
    /// Forgets every unsubmitted choice.
    pub(crate) fn clear(&mut self) {
        self.open_inspect = None;
        self.open_platform_picker = None;
        self.platform_query.clear();
        self.open_audit_picker = None;
        self.open_combined_audit_picker = false;
        self.quick_review_open = false;
        self.quick_success_details_open = false;
        self.confirm_remove = None;
        self.managed_mame_name.clear();
        self.confirm_remove_managed = None;
        self.open_managed_technical = None;
        self.tosec_group_filter.clear();
        self.show_tosec_raw.clear();
        self.open_diagnostic = None;
        self.plan_review_open = None;
        self.plan_page = 0;
        self.plan_typed_confirmation.clear();
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Draws the page and returns at most one requested action.
pub(crate) fn show_dat_sources_page(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;

    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::VERIFY,
        "Verify Games",
        "Use DAT catalogues to identify and check your games.",
    );

    if let Some(error) = &view.load_error {
        widgets::banner(
            ui,
            "Registry not read",
            &format!(
                "{error}\nShowing an empty list. Saving is disabled so the existing file is not \
                 overwritten."
            ),
            widgets::StatusTone::Blocked,
        );
        ui.add_space(8.0);
    }

    widgets::banner(
        ui,
        "Your files are safe",
        SAFE_PROMISE,
        widgets::StatusTone::Info,
    );
    ui.label(
        egui::RichText::new(READ_ONLY_PROMISE)
            .color(theme::muted(ui))
            .small(),
    );
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(format!("Supported formats: {SUPPORTED_FORMATS}"))
            .color(theme::muted(ui)),
    );
    ui.add_space(10.0);

    if let Some(acquisition_action) = show_evidence_acquisition_section(ui, view) {
        action = Some(acquisition_action);
    }
    ui.add_space(10.0);

    if let Some(bar_action) = show_toolbar(ui, view) {
        if action.is_none() {
            action = Some(bar_action);
        }
    }
    ui.add_space(10.0);

    if let Some(running) = &view.running
        && let Some(job_action) = show_running_job(ui, running)
    {
        action = Some(job_action);
    }

    if let Some(error) = &view.action_error {
        widgets::banner(
            ui,
            "That could not be done",
            error,
            widgets::StatusTone::Blocked,
        );
        ui.add_space(8.0);
    }

    widgets::section_header(
        ui,
        "Local DAT Sources",
        Some("User-added DAT files and folders. These stay local-only and are never updateable."),
    );
    if !view.rows.is_empty() {
        ui.horizontal(|ui| {
            if action.is_none()
                && widgets::action_button(
                    ui,
                    "Validate all",
                    widgets::ActionStyle::Secondary,
                    !view.background_busy,
                )
                .clicked()
            {
                action = Some(DatSourcesPageAction::ValidateAll);
            }
        });
        ui.add_space(6.0);
    }
    if let Some(summary) = &view.last_validate_all_summary {
        show_validate_all_summary(ui, summary);
        ui.add_space(8.0);
    }
    if view.is_empty() {
        widgets::empty_state(
            ui,
            &crate::ui::icons::with_icon(crate::ui::icons::VERIFY, "No DATs added"),
            "Add a DAT catalogue to verify your games. Nothing is downloaded and nothing is \
             changed.",
            None,
        );
    } else {
        for row in &view.rows {
            if action.is_none()
                && let Some(row_action) = show_source_row(ui, row, view, ui_state)
            {
                action = Some(row_action);
            }
            ui.add_space(8.0);
        }
    }

    ui.add_space(12.0);
    if action.is_none()
        && let Some(managed_action) = show_managed_dat_sources_section(ui, view, ui_state)
    {
        action = Some(managed_action);
    }

    ui.add_space(12.0);
    if action.is_none()
        && let Some(tosec_action) = show_tosec_release_packs_section(ui, view, ui_state)
    {
        action = Some(tosec_action);
    }

    // The policy section is always shown: its preferences and the Effective
    // Policy Summary are meaningful even before any source is registered.
    if action.is_none()
        && let Some(policy_action) = show_dat_policy_section(ui, &view.policy)
    {
        action = Some(policy_action);
    }

    if let Some(error) = &view.audit_error {
        ui.add_space(8.0);
        widgets::banner(
            ui,
            "Audit could not run",
            error,
            widgets::StatusTone::Blocked,
        );
    }
    if let Some(enrichment) = &view.identity_enrichment {
        ui.add_space(8.0);
        if enrichment.conflicts > 0 {
            let mut detail = String::new();
            for conflict in enrichment.conflict_details.iter().take(3) {
                if !detail.is_empty() {
                    detail.push('\n');
                }
                let evidence = conflict
                    .evidence
                    .iter()
                    .map(|item| {
                        format!(
                            "{}: {}",
                            item.source.label(),
                            archivefs_core::platform::display_name_for(&item.platform)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ");
                detail.push_str(&format!("{evidence} — Review required"));
            }
            if enrichment.conflict_details.len() > 3 {
                detail.push_str(&format!(
                    "\n…and {} more conflict(s)",
                    enrichment.conflict_details.len() - 3
                ));
            }
            widgets::banner(
                ui,
                "Platform conflict",
                &detail,
                widgets::StatusTone::Warning,
            );
        } else if enrichment.applied > 0 {
            widgets::banner(
                ui,
                "Platform identity enriched",
                &format!(
                    "{} library item(s) received verified DAT platform provenance. No ROM files or links were changed.",
                    enrichment.applied
                ),
                widgets::StatusTone::Success,
            );
        }
    }
    if let Some(audit) = &view.audit {
        ui.add_space(10.0);
        show_audit_result(ui, audit);
    }

    // The read-only rename-planning section, shown when the latest audit
    // produced a plan. It only ever displays and records review decisions.
    if let Some(plan) = &view.rename_plan
        && action.is_none()
        && let Some(plan_action) = show_rename_plan_section(ui, plan, ui_state)
    {
        action = Some(plan_action);
    }

    // The gated apply/recovery section. The user reviews the built transaction
    // and confirms; the core executor performs any rename on a worker thread.
    if action.is_none()
        && let Some(apply_action) = show_rename_apply_section(ui, &view.rename_apply, ui_state)
    {
        action = Some(apply_action);
    }

    if !view.load_problems.is_empty() || !view.unresolved.is_empty() {
        ui.add_space(10.0);
        show_kept_but_not_understood(ui, view);
    }

    action
}

/// A small, task-oriented entry point into the existing DAT-source flows.
///
/// This deliberately does not manufacture a generic downloader. No-Intro's
/// DAT-o-MATIC flow is request/form driven and does not expose a stable
/// anonymous download contract; TOSEC's official site exposes release pages
/// and generated pack downloads, but no durable machine-readable resolver.
/// Both therefore remain explicit local-import workflows. The controls here
/// make those authority models discoverable without requiring a user to
/// understand raw DAT filenames.
fn show_evidence_acquisition_section(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    let managed_configured = view
        .managed_rows
        .iter()
        .chain(view.redump_bios_rows.iter())
        .chain(view.redump_game_rows.iter())
        .filter(|row| row.configured)
        .count();
    let managed_installed = view
        .managed_rows
        .iter()
        .chain(view.redump_bios_rows.iter())
        .chain(view.redump_game_rows.iter())
        .filter(|row| row.installed)
        .count();
    let available_tosec_packs = view
        .tosec_packs
        .iter()
        .filter(|pack| pack.availability == PackAvailability::Available)
        .count();
    let selected_tosec_dats: usize = view
        .tosec_packs
        .iter()
        .map(|pack| pack.selected_dat_count)
        .sum();

    widgets::section_header(
        ui,
        "Get evidence for your library",
        Some(
            "Choose the source that fits your games. Importing or configuring a catalogue never changes game files.",
        ),
    );

    ui.columns(2, |columns| {
        widgets::card(&mut columns[0], |ui| {
            ui.label(egui::RichText::new("No-Intro — cartridge ROMs").strong());
            ui.label(
                egui::RichText::new(
                    "Download a pack manually from DAT-o-MATIC, then inspect it here. EmuWiz validates each DAT's internal metadata; the ZIP filename is never treated as authority.",
                )
                .color(theme::muted(ui))
                .small(),
            );
            if widgets::action_button(
                ui,
                "Open DAT-o-MATIC",
                widgets::ActionStyle::Secondary,
                !view.background_busy,
            )
            .clicked()
            {
                action = Some(DatSourcesPageAction::OpenNoIntroDownloadPage);
            }
            if widgets::action_button(
                ui,
                "Choose downloaded ZIP…",
                widgets::ActionStyle::Primary,
                !view.background_busy,
            )
            .clicked()
                && action.is_none()
                && let Some(path) = choose_no_intro_pack()
            {
                action = Some(DatSourcesPageAction::ChooseNoIntroPack { path });
            }
            if let Some((path, bytes)) = &view.no_intro_selected_pack {
                ui.label(format!("Selected: {path} ({})", format_bytes(*bytes)));
                if widgets::action_button(
                    ui,
                    "Inspect / validate pack",
                    widgets::ActionStyle::Quiet,
                    !view.background_busy,
                )
                .clicked()
                    && action.is_none()
                {
                    action = Some(DatSourcesPageAction::InspectNoIntroPack);
                }
            }
        });

        widgets::card(&mut columns[1], |ui| {
            ui.label(egui::RichText::new("TOSEC — vintage systems").strong());
            ui.label(
                egui::RichText::new(format!(
                    "Managed download is not available: the official site has no durable pack resolver. {available_tosec_packs} imported pack(s) · {selected_tosec_dats} selected DAT(s). Enable System / Category / Media below."
                ))
                .color(theme::muted(ui))
                .small(),
            );
            if widgets::action_button(
                ui,
                "Choose extracted TOSEC pack…",
                widgets::ActionStyle::Primary,
                !view.background_busy && view.tosec_load_error.is_none(),
            )
            .clicked()
                && action.is_none()
                && let Some(root) = choose_tosec_release_pack()
            {
                action = Some(DatSourcesPageAction::ImportTosecReleasePack { root });
            }
        });
    });
    ui.add_space(8.0);
    ui.columns(2, |columns| {
        widgets::card(&mut columns[0], |ui| {
            ui.label(egui::RichText::new("WHDLoad — Amiga packages").strong());
            ui.label(
                egui::RichText::new(
                    "Import the public Commodore - Amiga - WHDLoad DAT. It records complete Retroplay-derived LHA package checksums; EmuWiz never identifies a package from its filename.",
                )
                .color(theme::muted(ui))
                .small(),
            );
            if widgets::action_button(
                ui,
                "Choose WHDLoad DAT…",
                widgets::ActionStyle::Primary,
                !view.background_busy,
            )
            .clicked()
                && action.is_none()
                && let Some(path) = choose_whdload_dat_file()
            {
                action = Some(DatSourcesPageAction::AddWHDLoadDat { path });
            }
        });
        widgets::card(&mut columns[1], |ui| {
            ui.label(egui::RichText::new("Redump — disc and BIOS metadata").strong());
            ui.label(
                egui::RichText::new(format!(
                    "{managed_configured} managed source(s) configured · {managed_installed} installed. Fixed PlayStation, PlayStation 2, and Xbox sources are configured below, then checked or updated only when you click an action."
                ))
                .color(theme::muted(ui))
                .small(),
            );
        });
        widgets::card(&mut columns[1], |ui| {
            ui.label(egui::RichText::new("MAME and other local DATs").strong());
            ui.label(
                egui::RichText::new(
                    "Add a fixed MAME software-list by its authoritative name below, or import any local Logiqx / ClrMamePro DAT using the Local DAT Sources controls.",
                )
                .color(theme::muted(ui))
                .small(),
            );
            if widgets::action_button(
                ui,
                "Choose local DAT…",
                widgets::ActionStyle::Secondary,
                !view.background_busy,
            )
            .clicked()
                && action.is_none()
                && let Some(path) = choose_local_dat_file("Choose a local DAT")
            {
                action = Some(DatSourcesPageAction::AddFile { path });
            }
        });
    });
    if let Some(error) = &view.no_intro_action_error {
        widgets::banner(
            ui,
            "No-Intro pack action failed",
            error,
            widgets::StatusTone::Blocked,
        );
    }
    if let Some(inspection) = &view.no_intro_inspection {
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new("No-Intro pack inspection").strong());
            ui.label(format!(
                "Validated as {} from DAT metadata. {} valid DAT(s), {} rejected member(s).",
                no_intro_classification_label(inspection.classification),
                inspection.accepted.len(),
                inspection.rejected.len()
            ));
            for member in &inspection.accepted {
                ui.label(format!(
                    "• {} · {:?} · version {}",
                    member.system_name,
                    member.variant,
                    member.upstream_version.as_deref().unwrap_or("unknown")
                ));
            }
            if !inspection.rejected.is_empty() {
                ui.collapsing("Rejected or unsupported members", |ui| {
                    for member in &inspection.rejected {
                        ui.label(format!("{} — {}", member.member, member.reason));
                    }
                });
            }
            ui.label(egui::RichText::new(
                "Inspection is read-only. Nothing is installed until you explicitly import this pack.",
            ).color(theme::muted(ui)).small());
            if widgets::action_button(
                ui,
                "Import validated pack",
                widgets::ActionStyle::Primary,
                !view.background_busy,
            )
            .clicked()
                && action.is_none()
            {
                action = Some(DatSourcesPageAction::ImportNoIntroPack);
            }
        });
    }
    if let Some(installed) = &view.no_intro_installed {
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new("Installed No-Intro snapshot").strong());
            ui.label(format!(
                "{} · {} valid DAT(s) · snapshot hash {}…",
                no_intro_classification_label(installed.classification),
                installed.accepted.len(),
                installed.pack_sha256.chars().take(12).collect::<String>()
            ));
            ui.label("This is evidence metadata only; it does not imply a complete commercial-game collection.");
            if view.no_intro_import_status == Some(NoIntroPackImportStatus::Unchanged) {
                ui.label("The selected pack is already installed (Unchanged).");
            }
        });
    }
    action
}

fn choose_no_intro_pack() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Choose downloaded No-Intro pack ZIP")
        .add_filter("No-Intro pack ZIP", &["zip"])
        .pick_file()
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

fn no_intro_classification_label(classification: NoIntroPackClassification) -> &'static str {
    match classification {
        NoIntroPackClassification::Standard => "Standard No-Intro",
        NoIntroPackClassification::Aftermarket => "Aftermarket / Love Pack",
        NoIntroPackClassification::Bios => "No-Intro BIOS",
        NoIntroPackClassification::Mixed => "Mixed No-Intro pack",
        NoIntroPackClassification::Unknown => "No-Intro pack with unknown variant",
    }
}

fn open_no_intro_download_page() -> Option<String> {
    match std::process::Command::new("xdg-open")
        .arg(NO_INTRO_DATOMATIC_DOWNLOAD_PAGE)
        .status()
    {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!("Could not open DAT-o-MATIC (status {status}).")),
        Err(error) => Some(format!("Could not open DAT-o-MATIC: {error}")),
    }
}

fn choose_local_dat_file(title: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .add_filter("DAT catalogues", &["dat", "xml"])
        .pick_file()
}

fn choose_whdload_dat_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Choose Commodore - Amiga - WHDLoad DAT")
        .add_filter("WHDLoad DAT", &["dat"])
        .pick_file()
}

fn choose_tosec_release_pack() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Choose an extracted TOSEC release pack")
        .pick_folder()
}

/// One line of the compact evidence-readiness list: a Ready/Missing badge
/// plus the underlying count, so a normal user sees at a glance which
/// evidence a scan will use without opening DAT Sources or choosing a DAT
/// by hand.
fn show_evidence_readiness_row(ui: &mut egui::Ui, label: &str, count: usize, detail: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).strong());
        if count > 0 {
            widgets::status_badge(ui, "Ready", widgets::StatusTone::Success);
        } else {
            widgets::status_badge(ui, "Missing", widgets::StatusTone::Pending);
        }
    });
    ui.label(egui::RichText::new(detail).color(theme::muted(ui)).small());
}

/// Draws the task-oriented entry point for evidence-backed library renaming.
///
/// This intentionally reuses the same `DatSourcesPageState` actions and
/// read-only audit/plan/apply pipeline as the advanced DAT Sources page. It
/// does not infer a catalogue from a folder name or a file extension: it uses
/// every enabled local catalogue and installed managed *game* snapshot, then
/// preserves the exact agreeing source provenance. The user chooses only a
/// library folder (or one regular file). The resulting plan still requires an
/// explicit review and apply.
pub(crate) fn show_identify_rename_page(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;

    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::VERIFY,
        "Identify & Rename",
        "Identify files from enabled evidence catalogues, review canonical names, then apply only the changes you approve.",
    );
    widgets::banner(
        ui,
        "No filename guessing",
        "A rename is suggested only after an exact cryptographic DAT match. Unmatched, ambiguous, unsupported, and conflicting files stay untouched.",
        widgets::StatusTone::Info,
    );
    ui.add_space(8.0);

    if let Some(running) = &view.running
        && let Some(job_action) = show_running_job(ui, running)
    {
        action = Some(job_action);
    }
    if let Some(error) = &view.audit_error {
        ui.add_space(8.0);
        widgets::banner(
            ui,
            "Identification could not run",
            error,
            widgets::StatusTone::Blocked,
        );
    }

    // WHDLoad is identified only by the exact display name EmuWiz itself
    // assigns on import (`add_whdload_catalogue`) - never by platform, so
    // this stays a UI-only evidence-category split, not a per-platform rule.
    let whdload_enabled = view
        .rows
        .iter()
        .filter(|row| row.enabled && row.display_name == "WHDLoad / Retroplay catalogue")
        .count();
    let local_enabled = view
        .rows
        .iter()
        .filter(|row| row.enabled && row.display_name != "WHDLoad / Retroplay catalogue")
        .count();
    let mame_installed = view.managed_rows.iter().filter(|row| row.installed).count();
    let redump_installed = view
        .redump_game_rows
        .iter()
        .filter(|row| row.installed)
        .count();
    let selected_tosec: usize = view
        .tosec_packs
        .iter()
        .map(|pack| pack.selected_dat_count)
        .sum();
    widgets::section_header(
        ui,
        "Available evidence",
        Some(
            "One scan compares each file with every eligible installed catalogue. BIOS DATs are excluded. DAT Sources is where you add or change any of these; nothing here needs a manual per-scan choice.",
        ),
    );
    widgets::card(ui, |ui| {
        show_evidence_readiness_row(
            ui,
            "No-Intro / Local DATs",
            local_enabled,
            &format!("{local_enabled} enabled local catalogue(s)"),
        );
        show_evidence_readiness_row(
            ui,
            "TOSEC",
            selected_tosec,
            &format!("{selected_tosec} selected TOSEC DAT(s)"),
        );
        show_evidence_readiness_row(
            ui,
            "Redump",
            redump_installed,
            &format!("{redump_installed} installed game catalogue(s)"),
        );
        show_evidence_readiness_row(
            ui,
            "MAME",
            mame_installed,
            &format!("{mame_installed} installed software list(s)"),
        );
        show_evidence_readiness_row(
            ui,
            "WHDLoad",
            whdload_enabled,
            &format!("{whdload_enabled} enabled WHDLoad catalogue(s)"),
        );
        ui.add_space(4.0);
        // TOSEC and WHDLoad are valid audit inputs too. Keep the gate aligned
        // with `combined_audit_sources`, otherwise a user who has enabled
        // only the BBC's TOSEC evidence can never reach the folder picker.
        let can_scan = !view.background_busy
            && local_enabled + selected_tosec + whdload_enabled + mame_installed + redump_installed
                > 0;
        if widgets::action_button(
            ui,
            if ui_state.open_combined_audit_picker {
                "Cancel library choice"
            } else {
                "Choose library or file…"
            },
            widgets::ActionStyle::Primary,
            can_scan,
        )
        .clicked()
            && action.is_none()
        {
            ui_state.open_combined_audit_picker = !ui_state.open_combined_audit_picker;
        }
        if ui_state.open_combined_audit_picker
            && action.is_none()
            && let Some(audit_action) = show_combined_audit_target_picker(ui, view)
        {
            action = Some(audit_action);
        }
    });

    if let Some(audit) = &view.audit {
        ui.add_space(10.0);
        show_audit_result(ui, audit);
    }
    if let Some(plan) = &view.rename_plan
        && action.is_none()
        && let Some(plan_action) = show_rename_plan_section(ui, plan, ui_state)
    {
        action = Some(plan_action);
    }
    if action.is_none()
        && let Some(apply_action) = show_rename_apply_section(ui, &view.rename_apply, ui_state)
    {
        action = Some(apply_action);
    }

    if matches!(action, Some(DatSourcesPageAction::AuditAllEnabled { .. })) {
        ui_state.open_combined_audit_picker = false;
    }
    action
}

/// Draws the plain-language front door onto the same Identify & Rename state.
/// It intentionally keeps the technical plan hidden until the user asks to
/// review it; all scanning, selection, transaction, journal and apply actions
/// remain the existing production actions above.
pub(crate) fn show_quick_rename_page(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::CLEAN_UP,
        "Quick Rename",
        "Safely identify and rename games using verified catalogue evidence.",
    );
    widgets::banner(
        ui,
        "Your files are safe",
        "Scanning is read-only. Renaming still requires review, fresh safety checks, and a recovery journal.",
        widgets::StatusTone::Info,
    );

    if let Some(running) = &view.running
        && let Some(job_action) = show_running_job(ui, running)
    {
        action = Some(job_action);
    }
    if let Some(error) = &view.audit_error {
        widgets::banner(
            ui,
            "Scan could not run",
            error,
            widgets::StatusTone::Blocked,
        );
    }

    // Recovery transactions, split three ways so the simple workflow only
    // ever force-shows history that is genuinely about *this* folder:
    //   A) unresolved (non-`Applied`) AND for the current target -> shown
    //      directly, since it genuinely blocks trusting this folder's state.
    //   B) unresolved but for some OTHER folder/library entirely -> not a
    //      reason to interrupt this one; collapsed with (C) below.
    //   C) settled/`Applied` -> optional rollback history regardless of
    //      folder; always collapsed.
    // The current target is the best folder identity actually available:
    // a completed plan's `scan_root`, falling back to the last audit's own
    // `scan_root` if the plan failed to build. Before either exists there
    // is no current target to compare against at all - matching nothing is
    // not the same as *proven* irrelevant, so every unresolved transaction
    // stays in group (A) rather than being guessed into (B).
    if action.is_none() && !view.rename_apply.recovery.is_empty() {
        let current_root: Option<&str> = view
            .rename_plan
            .as_ref()
            .map(|plan| plan.scan_root.as_str())
            .or_else(|| view.audit.as_ref().map(|audit| audit.scan_root.as_str()));
        let mut blocking = Vec::new();
        let mut other = Vec::new();
        for recovery in &view.rename_apply.recovery {
            // Truthfully interrupted (`state.needs_recovery()`) AND not
            // already resolved (`resolution.is_none()`) AND for this
            // folder: only that combination genuinely still needs a
            // decision right now. An acknowledged "Leave untouched" moves
            // here regardless of folder, exactly like settled history.
            if recovery.state.needs_recovery()
                && recovery.resolution.is_none()
                && current_root
                    .is_none_or(|root| transaction_targets_root(&recovery.source_scan_root, root))
            {
                blocking.push(recovery.clone());
            } else {
                other.push(recovery.clone());
            }
        }
        if !blocking.is_empty() {
            ui.add_space(10.0);
            widgets::section_header(
                ui,
                "Unresolved rename transaction",
                Some(
                    "This must be resolved before Quick Rename can safely continue in this folder.",
                ),
            );
            if let Some(recovery_action) =
                show_recovery_transactions(ui, &blocking, view.rename_apply.rollback_running)
            {
                action = Some(recovery_action);
            }
        }
        if action.is_none() && !other.is_empty() {
            ui.add_space(10.0);
            egui::CollapsingHeader::new(format!("View recovery/history ({})", other.len()))
                .id_salt("quick_rename_history")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "Transactions for other folders, and settled transactions still \
                             eligible for rollback.",
                        )
                        .color(theme::muted(ui))
                        .small(),
                    );
                    ui.add_space(4.0);
                    if other
                        .iter()
                        .any(|recovery| recovery.state == TransactionState::Applied)
                        && ui.button("Hide settled history").clicked()
                    {
                        action = Some(DatSourcesPageAction::HideSettledRecoveryHistory);
                    }
                    if let Some(recovery_action) =
                        show_recovery_transactions(ui, &other, view.rename_apply.rollback_running)
                    {
                        action = Some(recovery_action);
                    }
                });
        }
        if let Some(error) = &view.rename_apply.recovery_resolution_error {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Your choice could not be saved",
                error,
                widgets::StatusTone::Blocked,
            );
        }
    }

    let showing_confirmation_or_success =
        view.rename_apply.review.is_some() || view.rename_apply.outcome.is_some();

    if action.is_none()
        && !showing_confirmation_or_success
        && view.rename_plan.is_none()
        && view.rename_apply.review.is_none()
    {
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new("Library").strong());
            ui.label(
                egui::RichText::new(
                    "EmuWiz will use every enabled, applicable evidence catalogue automatically.",
                )
                .color(theme::muted(ui))
                .small(),
            );
            if widgets::action_button(
                ui,
                if ui_state.open_combined_audit_picker {
                    "Cancel library choice"
                } else {
                    "Choose library or folder…"
                },
                widgets::ActionStyle::Primary,
                !view.background_busy,
            )
            .clicked()
            {
                ui_state.open_combined_audit_picker = !ui_state.open_combined_audit_picker;
            }
            if ui_state.open_combined_audit_picker
                && let Some(picker_action) = show_combined_audit_target_picker(ui, view)
            {
                action = Some(picker_action);
            }
        });
    }

    if let Some(plan) = &view.rename_plan {
        let safe = plan.counts.suggested;
        let canonical = plan.counts.already_canonical;
        let unsupported = plan.counts.unsupported;
        let ambiguous = plan.counts.ambiguous;
        let conflicts = plan.counts.conflicts;
        let unresolved = ambiguous + conflicts + plan.counts.blocked;
        // Metadata and other non-game files (for example RomM's
        // gamelist.xml, artwork and manuals) remain in Details but are not
        // presented as game candidates in this normal-user summary.
        let candidate_count = plan.rows.len().saturating_sub(
            plan.counts.excluded_by_content_policy + plan.counts.unclassified_content,
        );

        // The summary card and its buttons are the starting point of the
        // simple workflow; once a confirmation or a result is on screen,
        // Quick Rename shows exactly one thing at a time so the user is
        // never looking at two conflicting next steps.
        if action.is_none() && !showing_confirmation_or_success {
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!("{candidate_count} game archive(s) found"))
                        .strong(),
                );
                ui.label(format!("{safe} safe renames"));
                ui.label(format!("{canonical} already correct"));
                ui.label(format!("{unsupported} verified but unsupported"));
                ui.label(format!("{unresolved} files left unchanged"));
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if widgets::action_button(
                        ui,
                        "Review changes",
                        widgets::ActionStyle::Secondary,
                        safe > 0,
                    )
                    .clicked()
                    {
                        ui_state.quick_review_open = true;
                    }
                    if widgets::action_button(
                        ui,
                        format!("Rename {safe} verified files"),
                        widgets::ActionStyle::Primary,
                        safe > 0,
                    )
                    .clicked()
                    {
                        // One click does everything: select every currently
                        // safe/actionable proposal, then build the exact
                        // same production transaction the advanced planner
                        // builds. See `DatSourcesPageAction::QuickRenamePrepareApply`.
                        ui_state.quick_success_details_open = false;
                        action = Some(DatSourcesPageAction::QuickRenamePrepareApply);
                    }
                    if unresolved > 0 {
                        if ui.button("Get missing evidence").clicked() {
                            action = Some(DatSourcesPageAction::OpenDatSources);
                        }
                        if ui.button("See unresolved files").clicked() {
                            ui_state.quick_review_open = true;
                        }
                    }
                    // Always available, not only after a completed apply:
                    // covers a scan with zero safe renames, or simply
                    // deciding not to proceed with this folder - the user
                    // is never forced to navigate away and back to pick a
                    // different library.
                    if widgets::action_button(
                        ui,
                        "Rename another library",
                        widgets::ActionStyle::Quiet,
                        true,
                    )
                    .clicked()
                    {
                        action = Some(DatSourcesPageAction::ResetQuickRenameSession);
                    }
                });
                if unresolved > 0 {
                    ui.label(
                        egui::RichText::new(
                            "Some files need more evidence or are not safe to rename.",
                        )
                        .color(theme::muted(ui))
                        .small(),
                    );
                }
            });
        }

        // "Review changes" is the deliberately separate advanced route: it
        // is the full Identify & Rename planner (filters, per-row Accept/
        // Ignore/Needs review, "Planning only" terminology and all), never
        // required for the simple path above.
        if ui_state.quick_review_open {
            if ui.button("← Back to Quick Rename summary").clicked() {
                ui_state.quick_review_open = false;
            }
            if action.is_none()
                && let Some(plan_action) = show_rename_plan_section(ui, plan, ui_state)
            {
                action = Some(plan_action);
            }
            if action.is_none()
                && let Some(apply_action) =
                    show_rename_apply_review_and_outcome(ui, &view.rename_apply, ui_state)
            {
                action = Some(apply_action);
            }
        } else {
            // The simple path's own confirmation and success cards - no
            // planner terminology, no row-by-row selection, no trusted-root
            // or transaction-ID detail.
            if action.is_none()
                && let Some(review) = &view.rename_apply.review
            {
                if let Some(confirm_action) = show_quick_rename_confirmation(
                    ui,
                    review,
                    unsupported,
                    ambiguous,
                    conflicts,
                    &view.rename_apply,
                    ui_state,
                ) {
                    action = Some(confirm_action);
                }
            }
            if action.is_none()
                && let Some(outcome) = &view.rename_apply.outcome
                && let Some(success_action) = show_quick_rename_success(ui, outcome, ui_state)
            {
                action = Some(success_action);
            }
        }
    }
    // Rollback feedback is rendered at the page level, not nested under
    // `rename_plan` - a user can roll back a settled transaction straight
    // from the recovery/history section above without ever having loaded a
    // plan (or scanned) in this session, and the result must still be
    // visible. Suppressed only in the exact case where
    // `show_rename_apply_review_and_outcome` already rendered this same
    // data above (the advanced `Review changes` route, which is only ever
    // reachable once a plan exists) - never suppressed merely because
    // `quick_review_open` happens to be `true` while no plan is loaded,
    // since that combination shows nothing in the advanced branch either.
    if action.is_none() && !(ui_state.quick_review_open && view.rename_plan.is_some()) {
        if let Some(result) = &view.rename_apply.rollback_result {
            widgets::banner(
                ui,
                result.label,
                &result.detail,
                match result.label {
                    "Fully rolled back" => widgets::StatusTone::Success,
                    _ => widgets::StatusTone::Warning,
                },
            );
        }
        if let Some(error) = &view.rename_apply.rollback_error {
            widgets::banner(
                ui,
                "Rollback could not run",
                error,
                widgets::StatusTone::Blocked,
            );
        }
    }
    if action.is_none()
        && !ui_state.quick_review_open
        && ui.button("Open advanced Identify & Rename").clicked()
    {
        action = Some(DatSourcesPageAction::OpenAdvancedIdentifyRename);
    }
    if matches!(action, Some(DatSourcesPageAction::AuditAllEnabled { .. })) {
        ui_state.open_combined_audit_picker = false;
    }
    action
}

fn show_managed_dat_sources_section(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Managed DAT Sources",
        Some(
            "Built-in MAME and Redump sources. Checks and downloads happen only after you click an action.",
        ),
    );

    if let Some(error) = &view.managed_load_error {
        widgets::banner(
            ui,
            "Managed source configuration not read",
            error,
            widgets::StatusTone::Blocked,
        );
        ui.add_space(6.0);
    }
    if let Some(error) = &view.managed_action_error {
        widgets::banner(
            ui,
            "Managed source action could not be completed",
            error,
            widgets::StatusTone::Blocked,
        );
        ui.add_space(6.0);
    }

    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Add MAME software list").strong());
        ui.label(
            egui::RichText::new(
                "Enter the authoritative MAME software-list name. EmuWiz uses its fixed built-in MAME contract; URLs and providers are not configurable here.",
            )
            .color(theme::muted(ui))
            .small(),
        );
        ui.horizontal(|ui| {
            ui.label("Software-list name:");
            ui.text_edit_singleline(&mut ui_state.managed_mame_name);
            let enabled = !view.background_busy
                && view.managed_load_error.is_none()
                && !ui_state.managed_mame_name.trim().is_empty();
            if widgets::action_button(
                ui,
                "Add managed source",
                widgets::ActionStyle::Primary,
                enabled,
            )
            .clicked()
            {
                action = Some(DatSourcesPageAction::AddManagedMameSoftwareList {
                    authoritative_name: ui_state.managed_mame_name.trim().to_string(),
                });
                ui_state.managed_mame_name.clear();
            }
        });
    });
    ui.add_space(8.0);

    for row in &view.managed_rows {
        if action.is_none()
            && let Some(row_action) = show_managed_dat_source_row(ui, row, view, ui_state)
        {
            action = Some(row_action);
        }
        ui.add_space(8.0);
    }
    ui.add_space(4.0);
    widgets::section_header(
        ui,
        "Redump BIOS DATs",
        Some("Fixed Redump firmware metadata sources. No URLs or provider settings are exposed."),
    );
    for row in &view.redump_bios_rows {
        if action.is_none()
            && let Some(row_action) = show_managed_dat_source_row(ui, row, view, ui_state)
        {
            action = Some(row_action);
        }
        ui.add_space(8.0);
    }
    widgets::section_header(
        ui,
        "Redump Game/Disc DATs",
        Some("Fixed Redump catalogues for PlayStation, PlayStation 2, and Xbox only."),
    );
    for row in &view.redump_game_rows {
        if action.is_none()
            && let Some(row_action) = show_managed_dat_source_row(ui, row, view, ui_state)
        {
            action = Some(row_action);
        }
        ui.add_space(8.0);
    }
    action
}

fn show_managed_dat_source_row(
    ui: &mut egui::Ui,
    row: &ManagedDatSourceRowView,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    let busy = view.background_busy;
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(managed_provider_label(row.provider)).strong());
            let (label, tone) = managed_dat_status_presentation(&row.status);
            widgets::status_badge(ui, label, tone);
            if row.busy {
                ui.spinner();
            }
        });
        ui.label(&row.source_label);
        ui.label(
            egui::RichText::new(if row.configured {
                format!(
                    "Configured · {} updates",
                    managed_update_policy_label(row.update_policy)
                )
            } else {
                "Not configured".to_string()
            })
            .color(theme::muted(ui))
            .small(),
        );
        if !row.configured {
            ui.horizontal(|ui| {
                let add_label = format!("Enable {}", managed_provider_short_label(row.provider));
                if widgets::action_button(ui, &add_label, widgets::ActionStyle::Primary, !busy)
                    .clicked()
                    && action.is_none()
                {
                    action = managed_add_action(row);
                }
                let _ = widgets::action_button(ui, "Check", widgets::ActionStyle::Secondary, false);
                let _ = widgets::action_button(ui, "Update", widgets::ActionStyle::Primary, false);
            });
            return;
        }
        ui.label(
            egui::RichText::new(if row.installed {
                "Installed"
            } else {
                "Not installed"
            })
            .color(theme::muted(ui)),
        );
        if let Some(revision) = &row.current_revision {
            ui.label(
                egui::RichText::new(format!("Current revision: {revision}"))
                    .color(theme::muted(ui))
                    .small(),
            );
        }
        if let Some(last_checked) = &row.last_checked {
            ui.label(
                egui::RichText::new(format!("Last checked: {last_checked}"))
                    .color(theme::muted(ui))
                    .small(),
            );
        } else {
            ui.label(
                egui::RichText::new("Last checked: Never")
                    .color(theme::muted(ui))
                    .small(),
            );
        }
        match &row.status {
            ManagedDatStatusView::UpdateAvailable { upstream_revision } => {
                ui.label(
                    egui::RichText::new(format!("Available revision: {upstream_revision}"))
                        .color(theme::muted(ui))
                        .small(),
                );
            }
            ManagedDatStatusView::RateLimited {
                retry_after_seconds: Some(seconds),
            } => {
                ui.label(
                    egui::RichText::new(format!("Retry after: {seconds}s"))
                        .color(theme::muted(ui))
                        .small(),
                );
            }
            ManagedDatStatusView::Failed { detail } => {
                ui.label(
                    egui::RichText::new(detail)
                        .color(widgets::StatusTone::Blocked.color(ui))
                        .small(),
                );
            }
            _ => {}
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if widgets::action_button(
                ui,
                "Check",
                widgets::ActionStyle::Secondary,
                !busy && row.configured,
            )
            .clicked()
                && action.is_none()
            {
                action = Some(managed_dat_action(
                    row.source_id.clone(),
                    ManagedDatOperation::Check,
                ));
            }
            if widgets::action_button(
                ui,
                "Update",
                widgets::ActionStyle::Primary,
                !busy && row.configured && row.update_enabled,
            )
            .clicked()
                && action.is_none()
            {
                action = Some(managed_dat_action(
                    row.source_id.clone(),
                    ManagedDatOperation::Update,
                ));
            }
            if row.configured
                && widgets::action_button(ui, "Remove", widgets::ActionStyle::Quiet, !busy)
                    .clicked()
            {
                ui_state.confirm_remove_managed = Some(row.source_id.clone());
            }
            let technical_open = ui_state.open_managed_technical.as_ref() == Some(&row.source_id);
            if widgets::action_button(
                ui,
                if technical_open {
                    "Hide details"
                } else {
                    "Technical details"
                },
                widgets::ActionStyle::Quiet,
                true,
            )
            .clicked()
            {
                ui_state.open_managed_technical = if technical_open {
                    None
                } else {
                    Some(row.source_id.clone())
                };
            }
        });

        if ui_state.confirm_remove_managed.as_ref() == Some(&row.source_id) {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Remove this managed source configuration?",
                "This removes only the configured source. Existing managed snapshots are retained and no game files are touched.",
                widgets::StatusTone::Warning,
            );
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    "Remove configuration",
                    widgets::ActionStyle::Primary,
                    !busy,
                )
                .clicked()
                    && action.is_none()
                {
                    action = managed_remove_action(row);
                    ui_state.confirm_remove_managed = None;
                }
                if widgets::action_button(ui, "Keep it", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    ui_state.confirm_remove_managed = None;
                }
            });
        }

        if ui_state.open_managed_technical.as_ref() == Some(&row.source_id) {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Technical details").strong());
            managed_dat_technical_line(ui, "SHA-256", row.technical.sha256.as_deref());
            managed_dat_technical_line(ui, "ETag", row.technical.etag.as_deref());
            managed_dat_technical_line(ui, "Last-Modified", row.technical.last_modified.as_deref());
            managed_dat_technical_line(ui, "Current object", row.technical.current_path.as_deref());
            managed_dat_technical_line(
                ui,
                "Previous snapshot (not active)",
                row.technical.previous_snapshot.as_deref(),
            );
            managed_dat_technical_line(
                ui,
                "Previous object (not active)",
                row.technical.previous_path.as_deref(),
            );
        }
    });
    action
}

fn managed_source_key(source_id: &ManagedDatSourceId) -> String {
    format!("{:?}:{}", source_id.provider, source_id.source_key)
}

fn managed_provider_label(provider: ManagedDatProvider) -> &'static str {
    match provider {
        ManagedDatProvider::MameSoftwareList => "MAME software list",
        ManagedDatProvider::RedumpBios => "Redump BIOS DAT",
        ManagedDatProvider::RedumpGames => "Redump game/disc DAT",
    }
}

fn managed_provider_short_label(provider: ManagedDatProvider) -> &'static str {
    match provider {
        ManagedDatProvider::MameSoftwareList => "MAME source",
        ManagedDatProvider::RedumpBios => "BIOS source",
        ManagedDatProvider::RedumpGames => "game/disc source",
    }
}

fn managed_update_policy_label(policy: ManagedDatUpdatePolicy) -> &'static str {
    match policy {
        ManagedDatUpdatePolicy::Disabled => "disabled",
        ManagedDatUpdatePolicy::Manual => "manual",
    }
}

fn managed_add_action(row: &ManagedDatSourceRowView) -> Option<DatSourcesPageAction> {
    match row.provider {
        ManagedDatProvider::MameSoftwareList => None,
        ManagedDatProvider::RedumpBios => redump_bios_from_source_key(&row.source_id.source_key)
            .map(|system| DatSourcesPageAction::AddManagedRedumpBios { system }),
        ManagedDatProvider::RedumpGames => redump_game_from_source_key(&row.source_id.source_key)
            .map(|system| DatSourcesPageAction::AddManagedRedumpGames { system }),
    }
}

fn managed_remove_action(row: &ManagedDatSourceRowView) -> Option<DatSourcesPageAction> {
    match row.provider {
        ManagedDatProvider::MameSoftwareList => {
            Some(DatSourcesPageAction::RemoveManagedMameSoftwareList {
                authoritative_name: row.authoritative_name.clone(),
            })
        }
        ManagedDatProvider::RedumpBios => redump_bios_from_source_key(&row.source_id.source_key)
            .map(|system| DatSourcesPageAction::RemoveManagedRedumpBios { system }),
        ManagedDatProvider::RedumpGames => redump_game_from_source_key(&row.source_id.source_key)
            .map(|system| DatSourcesPageAction::RemoveManagedRedumpGames { system }),
    }
}

fn redump_bios_from_source_key(key: &str) -> Option<RedumpBiosSystem> {
    match key {
        "playstation" => Some(RedumpBiosSystem::PlayStation),
        "playstation2" => Some(RedumpBiosSystem::PlayStation2),
        "xbox" => Some(RedumpBiosSystem::Xbox),
        _ => None,
    }
}

fn redump_game_from_source_key(key: &str) -> Option<RedumpGameSystem> {
    match key {
        "playstation" => Some(RedumpGameSystem::PlayStation),
        "playstation2" => Some(RedumpGameSystem::PlayStation2),
        "xbox" => Some(RedumpGameSystem::Xbox),
        _ => None,
    }
}

fn redump_bios_label(system: RedumpBiosSystem) -> &'static str {
    match system {
        RedumpBiosSystem::PlayStation => "PlayStation",
        RedumpBiosSystem::PlayStation2 => "PlayStation 2",
        RedumpBiosSystem::Xbox => "Xbox",
    }
}

fn redump_game_label(system: RedumpGameSystem) -> &'static str {
    match system {
        RedumpGameSystem::PlayStation => "PlayStation",
        RedumpGameSystem::PlayStation2 => "PlayStation 2",
        RedumpGameSystem::Xbox => "Xbox",
    }
}

fn tosec_pack_view(pack: &PersistedTosecPack) -> TosecPackView {
    let mut groups: BTreeMap<TosecSelectionKey, (usize, usize, BTreeSet<String>)> = BTreeMap::new();
    let mut deferred_count = 0;
    for dat in &pack.dats {
        let key = dat.selection_key();
        let deferred = tosec_dat_is_deferred(dat);
        if deferred {
            deferred_count += 1;
        }
        let entry = groups.entry(key).or_default();
        entry.0 += 1;
        entry.1 += usize::from(deferred);
        entry.2.insert(dat.raw_category_label.clone());
    }
    TosecPackView {
        pack_id: pack.pack_id.clone(),
        root_path: pack.root_path.clone(),
        availability: pack.availability(),
        imported_at: format_unix_timestamp(pack.imported_unix_seconds),
        dat_count: pack.dats.len(),
        selected_dat_count: pack.selected_dats().count(),
        groups: groups
            .into_iter()
            .map(
                |(key, (dat_count, deferred_count, raw_categories))| TosecSelectionGroupView {
                    selected: pack.selections.contains(&key),
                    key,
                    dat_count,
                    deferred_count,
                    raw_categories: raw_categories.into_iter().collect(),
                },
            )
            .collect(),
        deferred_count,
    }
}

fn tosec_dat_is_deferred(dat: &TosecPackDat) -> bool {
    let upper = dat.raw_catalogue_name.to_ascii_uppercase();
    upper.starts_with("TOSEC-ISO") || upper.starts_with("TOSEC-PIX")
}

const MAX_TOSEC_GROUPS_RENDERED: usize = 200;

fn show_tosec_release_packs_section(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "TOSEC Release Packs",
        Some(
            "Read-only inventories of extracted TOSEC packs. Enable only the System / Category / Media groups you want, then apply them to Local DAT Sources.",
        ),
    );
    if let Some(error) = &view.tosec_load_error {
        widgets::banner(
            ui,
            "TOSEC pack configuration not read",
            error,
            widgets::StatusTone::Blocked,
        );
    }
    if let Some(error) = &view.tosec_action_error {
        widgets::banner(
            ui,
            "TOSEC release-pack action could not be completed",
            error,
            widgets::StatusTone::Blocked,
        );
    }
    if let Some(last) = &view.tosec_last_apply {
        widgets::banner(
            ui,
            "TOSEC selection applied",
            &format!(
                "{}: {} registered, {} already registered, {} deferred, {} conflicts, {} failed, {} removed after deselection.",
                last.pack_id,
                last.registered,
                last.already_registered,
                last.deferred,
                last.conflicts,
                last.failed,
                last.removed
            ),
            if last.failed == 0 && last.conflicts == 0 {
                widgets::StatusTone::Success
            } else {
                widgets::StatusTone::Warning
            },
        );
    }
    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Add extracted release pack").strong());
        ui.label(egui::RichText::new("Choose a local, already-extracted TOSEC directory. EmuWiz inventories it read-only and starts with no DAT groups enabled.").color(theme::muted(ui)).small());
        if widgets::action_button(
            ui,
            "Choose release-pack folder",
            widgets::ActionStyle::Primary,
            !view.background_busy && view.tosec_load_error.is_none(),
        )
        .clicked()
            && let Some(path) = choose_tosec_release_pack()
        {
            action = Some(DatSourcesPageAction::ImportTosecReleasePack { root: path });
        }
    });
    for pack in &view.tosec_packs {
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&pack.pack_id).strong());
                let (status, tone) = match pack.availability {
                    PackAvailability::Available => ("Available", widgets::StatusTone::Success),
                    PackAvailability::Missing => ("Pack missing", widgets::StatusTone::Blocked),
                };
                widgets::status_badge(ui, status, tone);
            });
            ui.label(
                egui::RichText::new(pack.root_path.display().to_string())
                    .color(theme::muted(ui))
                    .small(),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} DATs inventoried · {} selected · imported {}",
                    pack.dat_count, pack.selected_dat_count, pack.imported_at
                ))
                .color(theme::muted(ui))
                .small(),
            );
            if pack.deferred_count > 0 {
                ui.label(egui::RichText::new(format!("{} TOSEC-ISO / TOSEC-PIX catalogue(s) are visible but deferred and will not be registered.", pack.deferred_count)).color(widgets::StatusTone::Warning.color(ui)).small());
            }
            let filter = ui_state
                .tosec_group_filter
                .entry(pack.pack_id.clone())
                .or_default();
            ui.horizontal(|ui| {
                ui.label("Filter groups:");
                ui.text_edit_singleline(filter);
                let mut show_raw = ui_state.show_tosec_raw.contains(&pack.pack_id);
                if ui
                    .checkbox(&mut show_raw, "Show raw TOSEC details")
                    .changed()
                {
                    if show_raw {
                        ui_state.show_tosec_raw.remove(&pack.pack_id);
                    } else {
                        ui_state.show_tosec_raw.insert(pack.pack_id.clone());
                    }
                }
            });
            let normalized_filter = filter.trim().to_ascii_lowercase();
            let matching: Vec<_> = pack
                .groups
                .iter()
                .filter(|group| {
                    normalized_filter.is_empty()
                        || group
                            .key
                            .label()
                            .to_ascii_lowercase()
                            .contains(&normalized_filter)
                })
                .collect();
            let shown = matching.len().min(MAX_TOSEC_GROUPS_RENDERED);
            for group in matching.iter().take(MAX_TOSEC_GROUPS_RENDERED) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "{} · {} DAT(s)",
                        group.key.label(),
                        group.dat_count
                    ));
                    if group.deferred_count > 0 {
                        ui.label(
                            egui::RichText::new("deferred")
                                .color(widgets::StatusTone::Warning.color(ui))
                                .small(),
                        );
                    }
                    let button = if group.selected { "Disable" } else { "Enable" };
                    let toggle_enabled = !view.background_busy
                        && pack.availability == PackAvailability::Available
                        && (group.deferred_count == 0 || group.selected);
                    if group.deferred_count > 0 && !group.selected {
                        ui.label(
                            egui::RichText::new("Deferred by current TOSEC support")
                                .color(theme::muted(ui))
                                .small(),
                        );
                    } else if widgets::action_button(
                        ui,
                        button,
                        widgets::ActionStyle::Secondary,
                        toggle_enabled,
                    )
                    .clicked()
                        && action.is_none()
                    {
                        action = Some(DatSourcesPageAction::SetTosecSelection {
                            pack_id: pack.pack_id.clone(),
                            key: group.key.clone(),
                            enabled: !group.selected,
                        });
                    }
                });
                if ui_state.show_tosec_raw.contains(&pack.pack_id) {
                    ui.label(
                        egui::RichText::new(format!(
                            "Raw category: {}",
                            group.raw_categories.join("; ")
                        ))
                        .color(theme::muted(ui))
                        .small(),
                    );
                }
            }
            if matching.len() > shown {
                ui.label(egui::RichText::new(format!("Showing {shown} of {} matching groups. Refine the filter to keep rendering bounded.", matching.len())).color(theme::muted(ui)).small());
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    "Apply selected DATs",
                    widgets::ActionStyle::Primary,
                    !view.background_busy && pack.availability == PackAvailability::Available,
                )
                .clicked()
                    && action.is_none()
                {
                    action = Some(DatSourcesPageAction::ApplyTosecSelection {
                        pack_id: pack.pack_id.clone(),
                    });
                }
                if widgets::action_button(
                    ui,
                    "Remove pack configuration",
                    widgets::ActionStyle::Quiet,
                    !view.background_busy,
                )
                .clicked()
                    && action.is_none()
                {
                    action = Some(DatSourcesPageAction::RemoveTosecReleasePack {
                        pack_id: pack.pack_id.clone(),
                    });
                }
            });
        });
    }
    if view.tosec_packs.is_empty() {
        widgets::empty_state(
            ui,
            "No TOSEC release packs configured",
            "Choose an extracted local pack to inventory it. Nothing is registered until you explicitly enable groups and apply them.",
            None,
        );
    }
    action
}

fn managed_dat_technical_line(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    ui.label(
        egui::RichText::new(format!("{label}: {}", value.unwrap_or("Not recorded")))
            .color(theme::muted(ui))
            .small(),
    );
}

fn managed_dat_status_presentation(status: &ManagedDatStatusView) -> (&str, widgets::StatusTone) {
    match status {
        ManagedDatStatusView::NotInstalled => ("Not installed", widgets::StatusTone::Pending),
        ManagedDatStatusView::Idle => ("Ready", widgets::StatusTone::Info),
        ManagedDatStatusView::Checking => ("Checking…", widgets::StatusTone::Pending),
        ManagedDatStatusView::UpdateAvailable { .. } => {
            ("Update available", widgets::StatusTone::Warning)
        }
        ManagedDatStatusView::UpToDate => ("Up to date", widgets::StatusTone::Success),
        ManagedDatStatusView::Updating => ("Updating…", widgets::StatusTone::Pending),
        ManagedDatStatusView::Updated => ("Updated successfully", widgets::StatusTone::Success),
        ManagedDatStatusView::Offline => (
            "Offline — existing DAT remains available",
            widgets::StatusTone::Warning,
        ),
        ManagedDatStatusView::RateLimited { .. } => (
            "Rate limited — try again later",
            widgets::StatusTone::Warning,
        ),
        ManagedDatStatusView::Disabled => ("Manual updates disabled", widgets::StatusTone::Pending),
        ManagedDatStatusView::Failed { .. } => ("Update failed", widgets::StatusTone::Blocked),
    }
}

fn show_toolbar(ui: &mut egui::Ui, view: &DatSourcesPageView) -> Option<DatSourcesPageAction> {
    let mut action = None;
    let busy = view.background_busy;
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            // rfd's pickers are synchronous and return `None` on cancel or
            // failure; they never panic. Held here rather than in the state so
            // the state stays testable without a window.
            if widgets::action_button(ui, "Add DAT file…", widgets::ActionStyle::Primary, !busy)
                .clicked()
                && let Some(path) = choose_local_dat_file("Choose a DAT file")
            {
                action = Some(DatSourcesPageAction::AddFile { path });
            }
            if widgets::action_button(
                ui,
                "Add DAT folder…",
                widgets::ActionStyle::Secondary,
                !busy,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose a folder of DAT files")
                    .pick_folder()
            {
                action = Some(DatSourcesPageAction::AddFolder { path });
            }
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if view.dirty {
                widgets::status_badge(ui, "Unsaved changes", widgets::StatusTone::Warning);
            } else {
                widgets::status_badge(ui, "No unsaved changes", widgets::StatusTone::Success);
            }
            ui.add_space(8.0);
            let savable = view.dirty && view.load_error.is_none();
            if widgets::action_button(ui, "Save", widgets::ActionStyle::Primary, savable).clicked()
            {
                action = Some(DatSourcesPageAction::Save);
            }
            if widgets::action_button(
                ui,
                "Discard changes",
                widgets::ActionStyle::Secondary,
                view.dirty,
            )
            .clicked()
            {
                action = Some(DatSourcesPageAction::Revert);
            }
        });

        if view.dirty {
            ui.add_space(6.0);
            ui.label("Saving will:");
            for line in &view.pending_consequences {
                ui.label(format!("  • {line}"));
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Nothing is written until you save.").color(theme::muted(ui)),
            );
        }

        match &view.save_state {
            DatSaveState::Idle => {}
            DatSaveState::Saved => {
                ui.add_space(6.0);
                widgets::status_badge(ui, "Registry saved", widgets::StatusTone::Success);
            }
            DatSaveState::Failed(message) => {
                ui.add_space(6.0);
                widgets::banner(ui, "Save failed", message, widgets::StatusTone::Blocked);
            }
        }

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!("File: {}", view.config_path.display()))
                .color(theme::muted(ui))
                .small(),
        );
    });
    action
}

/// The final tally banner for the last completed (or cancelled) "Validate
/// all" run. Shown until the next run replaces it or a Revert discards it.
fn show_validate_all_summary(ui: &mut egui::Ui, summary: &ValidateAllSummary) {
    let checked = summary.total.saturating_sub(summary.skipped);
    let tone = if summary.failed > 0 {
        widgets::StatusTone::Warning
    } else if summary.total == 0 {
        widgets::StatusTone::Info
    } else {
        widgets::StatusTone::Success
    };
    let title = if summary.total == 0 {
        "Validate all: nothing to validate".to_string()
    } else if summary.skipped > 0 {
        format!(
            "Validate all: stopped after {checked} of {} sources",
            summary.total
        )
    } else {
        format!("Validate all: {checked} source(s) checked")
    };
    let detail = if summary.total == 0 {
        "No DAT sources are configured.".to_string()
    } else {
        let mut detail = format!(
            "{} valid, {} changed, {} failed",
            summary.valid, summary.changed, summary.failed
        );
        if summary.skipped > 0 {
            detail.push_str(&format!(", {} skipped (cancelled)", summary.skipped));
        }
        detail
    };
    widgets::banner(ui, &title, &detail, tone);
}

fn show_running_job(ui: &mut egui::Ui, running: &RunningJobView) -> Option<DatSourcesPageAction> {
    let mut action = None;
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new(running.heading()).strong());
            if running.cancellation_requested {
                // Cancel has been pressed; the button is gone and the wording
                // says so. The job stays busy until the worker confirms.
                widgets::status_badge(ui, "Stopping…", widgets::StatusTone::Warning);
            } else if running.cancellable
                && widgets::action_button(ui, "Cancel", widgets::ActionStyle::Secondary, true)
                    .clicked()
            {
                action = Some(DatSourcesPageAction::CancelJob);
            }
        });
        ui.label(egui::RichText::new(&running.detail).color(theme::muted(ui)));
        if let Some(platform) = &running.platform_display {
            // Shown only when the source's assigned platform is authoritative;
            // never guessed from the path.
            ui.label(
                egui::RichText::new(format!("Platform: {platform}"))
                    .color(theme::muted(ui))
                    .small(),
            );
        }
        if let Some(progress) = &running.progress {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(progress.line()).color(theme::muted(ui)));
            if let Some(path) = &progress.current_path {
                ui.label(
                    egui::RichText::new(format!("In: {path}"))
                        .color(theme::muted(ui))
                        .small(),
                );
            }
            // No ETA while stopping: the run is ending, so a remaining-time is
            // meaningless, and the frozen estimate must not keep being shown
            // next to "Stopping…".
            if !running.cancellation_requested
                && let Some(eta) = progress.eta_line()
            {
                ui.label(egui::RichText::new(eta).color(theme::muted(ui)).small());
            }
        }
    });
    ui.add_space(8.0);
    action
}

fn show_source_row(
    ui: &mut egui::Ui,
    row: &DatSourceRowView,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    let busy_elsewhere = view.background_busy;

    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            let mut enabled = row.enabled;
            if ui.checkbox(&mut enabled, "").changed() {
                action = Some(DatSourcesPageAction::SetEnabled {
                    id: row.id.clone(),
                    enabled,
                });
            }
            ui.label(egui::RichText::new(&row.display_name).strong());
            if !row.enabled {
                widgets::status_badge(ui, "Disabled", widgets::StatusTone::Pending);
            }
            widgets::status_badge(ui, health_label(row), health_tone(row.health_state));
            if row.changed {
                widgets::status_badge(ui, "Changed", widgets::StatusTone::Warning);
            }
        });

        ui.label(
            egui::RichText::new(format!("ID: {}", row.id))
                .color(theme::muted(ui))
                .monospace(),
        );
        ui.label(
            egui::RichText::new(format!("{} · {}", row.kind_label, row.path))
                .color(theme::muted(ui)),
        );

        // Format is only ever what a check observed. An unvalidated source says
        // so rather than guessing from the file extension.
        let format_line = if row.formats.is_empty() {
            "Format: not checked yet".to_string()
        } else {
            format!("Format: {}", row.formats.join(", "))
        };
        ui.label(egui::RichText::new(format_line).color(theme::muted(ui)));

        if let Some(detail) = &row.health_detail {
            ui.label(detail);
        }
        if let Some(when) = &row.last_validated {
            ui.label(
                egui::RichText::new(if row.health_stale {
                    format!("Checked {when} — the file has changed since, so this is out of date.")
                } else {
                    format!("Checked {when}")
                })
                .color(if row.health_stale {
                    widgets::StatusTone::Warning.color(ui)
                } else {
                    theme::muted(ui)
                })
                .small(),
            );
        }

        // An incomplete catalogue load is a distinct, prominent result: the
        // safety limit stopped the check part-way, so the verdict covers only
        // part of the folder and nothing may imply all of it was read.
        if row.incomplete_load {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Incomplete catalogue load",
                &row.incomplete_load_line().unwrap_or_default(),
                widgets::StatusTone::Warning,
            );
        }

        show_diagnostics_summary(ui, row, ui_state);

        ui.add_space(6.0);
        if action.is_none()
            && let Some(platform_action) = show_platform_control(ui, row, ui_state)
        {
            action = Some(platform_action);
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if widgets::action_button(
                ui,
                "Validate",
                widgets::ActionStyle::Secondary,
                !busy_elsewhere,
            )
            .clicked()
                && action.is_none()
            {
                action = Some(DatSourcesPageAction::Validate { id: row.id.clone() });
            }
            let inspecting = ui_state.open_inspect.as_deref() == Some(row.id.as_str());
            if widgets::action_button(
                ui,
                if inspecting {
                    "Hide details"
                } else {
                    "Inspect"
                },
                widgets::ActionStyle::Quiet,
                true,
            )
            .clicked()
            {
                ui_state.open_inspect = if inspecting {
                    None
                } else {
                    Some(row.id.clone())
                };
            }
            let auditing = ui_state.open_audit_picker.as_deref() == Some(row.id.as_str());
            if widgets::action_button(
                ui,
                if auditing {
                    "Cancel audit setup"
                } else {
                    "Audit…"
                },
                widgets::ActionStyle::Secondary,
                !busy_elsewhere,
            )
            .clicked()
            {
                ui_state.open_audit_picker = if auditing { None } else { Some(row.id.clone()) };
            }
            if widgets::action_button(ui, "Remove", widgets::ActionStyle::Quiet, !busy_elsewhere)
                .clicked()
            {
                ui_state.confirm_remove = Some(row.id.clone());
            }
        });

        if ui_state.confirm_remove.as_deref() == Some(row.id.as_str()) {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Remove this source?",
                &format!(
                    "'{}' will no longer be registered. The file at {} is not deleted, and no ROM \
                     is touched.",
                    row.display_name, row.path
                ),
                widgets::StatusTone::Warning,
            );
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    "Remove from registry",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
                    && action.is_none()
                {
                    action = Some(DatSourcesPageAction::Remove { id: row.id.clone() });
                }
                if widgets::action_button(ui, "Keep it", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    ui_state.confirm_remove = None;
                }
            });
        }

        if ui_state.open_audit_picker.as_deref() == Some(row.id.as_str())
            && action.is_none()
            && let Some(audit_action) = show_audit_target_picker(ui, row, view)
        {
            action = Some(audit_action);
        }

        if ui_state.open_inspect.as_deref() == Some(row.id.as_str()) {
            ui.add_space(6.0);
            show_inspect(ui, row);
        }
    });

    // Once the removal or the audit has been asked for, the disclosure has done
    // its job and stays open on a row that may no longer exist.
    if let Some(requested) = &action {
        match requested {
            DatSourcesPageAction::Remove { .. } => ui_state.confirm_remove = None,
            DatSourcesPageAction::Audit { .. } => ui_state.open_audit_picker = None,
            _ => {}
        }
    }
    action
}

fn health_label(row: &DatSourceRowView) -> String {
    if row.health_stale {
        format!("{} (out of date)", row.health_state.label())
    } else {
        row.health_state.label().to_string()
    }
}

/// Warnings and parser notes, each with a count, a concise inline summary, and
/// an expandable list, drawn directly on the source card so the health badge is
/// never the only thing the user can see.
///
/// Warnings are worth investigating; parser notes are expected behaviour and
/// are never presented as if something is wrong.
/// The pluralised, type-oriented label for a severity, as used in the section
/// badge: "error type", "warning type", "parser-note type".
fn severity_type_label(severity: DiagnosticSeverity, types: usize) -> String {
    let base = match severity {
        DiagnosticSeverity::Error => "error type",
        DiagnosticSeverity::Warning => "warning type",
        DiagnosticSeverity::Note => "parser-note type",
    };
    if types == 1 {
        base.to_string()
    } else {
        format!("{base}s")
    }
}

/// "line 3:12" / "line 3" / "Location unavailable", from what the parser
/// actually recorded. Never invents a location the parser did not provide.
fn format_location(line: Option<usize>, column: Option<usize>) -> String {
    match (line, column) {
        (Some(line), Some(column)) => format!("line {line}:{column}"),
        (Some(line), None) => format!("line {line}"),
        _ => "Location unavailable".to_string(),
    }
}

/// One group's drill-down: the verbatim message, the counts, and a bounded
/// occurrence list with safe filenames and parser-provided locations.
fn show_diagnostic_group(
    ui: &mut egui::Ui,
    group: &DiagnosticGroupView,
    ui_state: &mut DatSourcesPageUi,
) {
    let open = ui_state.open_diagnostic.as_deref() == Some(group.id.as_str());
    ui.horizontal_top(|ui| {
        ui.label(egui::RichText::new("•").color(theme::muted(ui)));
        ui.vertical(|ui| {
            // The original diagnostic message, preserved verbatim.
            ui.add(egui::Label::new(&group.message).wrap());
            ui.label(
                egui::RichText::new(format!(
                    "{} occurrence{} · affects {} DAT file{}",
                    group.occurrence_count,
                    if group.occurrence_count == 1 { "" } else { "s" },
                    group.affected_file_count,
                    if group.affected_file_count == 1 {
                        ""
                    } else {
                        "s"
                    },
                ))
                .color(theme::muted(ui))
                .small(),
            );
            let toggle = if open {
                "Hide locations"
            } else {
                "View locations"
            };
            if widgets::action_button(ui, toggle, widgets::ActionStyle::Quiet, true).clicked() {
                ui_state.open_diagnostic = if open { None } else { Some(group.id.clone()) };
            }
            if open {
                for occurrence in &group.occurrences {
                    ui.horizontal_top(|ui| {
                        ui.label(
                            egui::RichText::new(format_location(
                                occurrence.line,
                                occurrence.column,
                            ))
                            .color(theme::muted(ui))
                            .small()
                            .monospace(),
                        );
                        ui.label(egui::RichText::new(&occurrence.file_name).small());
                    });
                }
                if group.occurrences_truncated {
                    ui.label(
                        egui::RichText::new(format!(
                            "… and {} more occurrence(s)",
                            group.occurrence_count - group.occurrences.len()
                        ))
                        .color(theme::muted(ui))
                        .small(),
                    );
                }
            }
        });
    });
}

/// Diagnostics grouped by type, drawn directly on the source card so the health
/// badge is never the only thing the user can see.
///
/// Each severity (Errors, Warnings, Parser notes) is one section showing how
/// many distinct types and total occurrences there are, followed by one row per
/// diagnostic type. Expanding a row reveals its locations. One row expands at a
/// time; parser notes carry the reassurance that no action is needed.
fn show_diagnostics_summary(
    ui: &mut egui::Ui,
    row: &DatSourceRowView,
    ui_state: &mut DatSourcesPageUi,
) {
    if row.groups.is_empty() {
        return;
    }

    // Occurrence lists are deliberately bounded for rendering, so never use
    // their length for the headline. The stored occurrence totals remain
    // exact even for very large folders.
    let error_issues = row.diagnostic_occurrences(DiagnosticSeverity::Error);
    let attention_issues = error_issues + row.diagnostic_occurrences(DiagnosticSeverity::Warning);
    if attention_issues > 0 {
        // An Error means core marked the source Invalid ("part of what they
        // asked for is unusable" - see `dat/sources/validation.rs`), which is
        // not the same claim as "still works, some files were skipped". Say
        // so truthfully instead of reassuring the user past a real problem.
        let has_errors = error_issues > 0;
        ui.add_space(6.0);
        widgets::status_badge(
            ui,
            format!(
                "{attention_issues} catalogue issue{} found",
                if attention_issues == 1 { "" } else { "s" }
            ),
            if has_errors {
                widgets::StatusTone::Blocked
            } else {
                widgets::StatusTone::Warning
            },
        );
        ui.label(
            egui::RichText::new(if has_errors {
                "Some files could not be used and need your attention."
            } else {
                "The catalogue still works. Files that could not be used were skipped."
            })
            .color(theme::muted(ui))
            .small(),
        );
    }

    egui::CollapsingHeader::new("What happened?")
        .id_salt(("dat-diagnostics-summary", row.id.as_str()))
        .default_open(false)
        .show(ui, |ui| {
            let notes = row.diagnostic_occurrences(DiagnosticSeverity::Note);
            if notes > 0 {
                ui.label(format!(
                    "{notes} additional catalogue note{} recorded. No action is needed for these.",
                    if notes == 1 { "" } else { "s" }
                ));
            }
            widgets::technical_details(
                ui,
                ("dat-parser-technical-details", row.id.as_str()),
                |ui| {
                    for severity in [
                        DiagnosticSeverity::Error,
                        DiagnosticSeverity::Warning,
                        DiagnosticSeverity::Note,
                    ] {
                        let groups = row.groups_of(severity);
                        if groups.is_empty() {
                            continue;
                        }
                        let types = row.diagnostic_types(severity);
                        let occurrences = row.diagnostic_occurrences(severity);
                        let tone = match severity {
                            DiagnosticSeverity::Error => widgets::StatusTone::Blocked,
                            DiagnosticSeverity::Warning => widgets::StatusTone::Warning,
                            DiagnosticSeverity::Note => widgets::StatusTone::Info,
                        };
                        widgets::status_badge(
                            ui,
                            format!(
                                "{types} {}, {occurrences} occurrence{}",
                                severity_type_label(severity, types),
                                if occurrences == 1 { "" } else { "s" }
                            ),
                            tone,
                        );
                        for group in groups {
                            ui.add_space(2.0);
                            show_diagnostic_group(ui, group, ui_state);
                        }
                    }
                },
            );
        });

    if row.history_link_available {
        // Only ever drawn when the full details really are recorded in
        // History & Logs; today they are kept inline, so this is off.
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Full details are recorded in History & Logs.")
                .color(theme::muted(ui))
                .small(),
        );
    }
}

fn health_tone(state: DatHealthState) -> widgets::StatusTone {
    match state {
        // Never rendered as healthy or failed: it is neither.
        DatHealthState::NotChecked => widgets::StatusTone::Pending,
        DatHealthState::Valid => widgets::StatusTone::Success,
        DatHealthState::ValidWithWarnings => widgets::StatusTone::Warning,
        DatHealthState::Invalid | DatHealthState::Unreadable => widgets::StatusTone::Blocked,
    }
}

/// The platform assignment, using the same canonical registry the rest of the
/// GUI picks from, so an assignment can only ever name a platform the resolver
/// will actually match.
fn show_platform_control(
    ui: &mut egui::Ui,
    row: &DatSourceRowView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    let is_open = ui_state.open_platform_picker.as_deref() == Some(row.id.as_str());

    ui.horizontal(|ui| {
        ui.label("Platform:");
        match &row.platform_display {
            Some(platform) => {
                ui.label(egui::RichText::new(platform).strong());
                if row.platform_unresolved {
                    ui.label(
                        egui::RichText::new("(not recognised by this build; kept as written)")
                            .color(widgets::StatusTone::Warning.color(ui))
                            .small(),
                    );
                }
            }
            None => {
                ui.label(
                    egui::RichText::new("any (the catalogue's own header decides)")
                        .color(theme::muted(ui)),
                );
            }
        }
        if widgets::action_button(
            ui,
            if is_open { "Cancel" } else { "Change…" },
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
        {
            ui_state.open_platform_picker = if is_open { None } else { Some(row.id.clone()) };
            ui_state.platform_query.clear();
        }
        if row.platform_display.is_some()
            && widgets::action_button(ui, "Clear", widgets::ActionStyle::Quiet, true).clicked()
        {
            action = Some(DatSourcesPageAction::SetPlatform {
                id: row.id.clone(),
                platform: None,
            });
        }
    });

    if !is_open || action.is_some() {
        return action;
    }

    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Find a platform:");
            ui.add(
                egui::TextEdit::singleline(&mut ui_state.platform_query)
                    .hint_text("e.g. PlayStation 2")
                    .desired_width(220.0),
            );
        });
        let choices = platform_choices(&ui_state.platform_query);
        let total = platform_choice_count(&ui_state.platform_query);
        if choices.is_empty() {
            ui.label(egui::RichText::new("No platform matches.").color(theme::muted(ui)));
            return;
        }
        // Compact selectable rows instead of one full-width button per
        // match: the current platform (if it happens to be among the
        // matches) is highlighted, same as any other selectable-row list
        // in this app, rather than reading as a wall of identical buttons.
        for (id, display_name) in &choices {
            let is_current = row.platform_display.as_deref() == Some(*display_name);
            if ui.selectable_label(is_current, *display_name).clicked() && action.is_none() {
                action = Some(DatSourcesPageAction::SetPlatform {
                    id: row.id.clone(),
                    platform: Some((*id).to_string()),
                });
            }
        }
        if total > choices.len() {
            ui.label(
                egui::RichText::new(format!(
                    "Showing {} of {total} matches. Type to narrow the search.",
                    choices.len()
                ))
                .color(theme::muted(ui))
                .small(),
            );
        }
    });

    if action.is_some() {
        ui_state.open_platform_picker = None;
        ui_state.platform_query.clear();
    }
    action
}

/// How many platform choices the picker shows at once.
pub(crate) const MAX_PLATFORM_CHOICES: usize = 12;

/// Canonical platforms matching `query`, drawn strictly from the same registry
/// `canonical_platform_for_alias` resolves against.
pub(crate) fn platform_choices(query: &str) -> Vec<(&'static str, &'static str)> {
    let needle = query.trim().to_lowercase();
    archivefs_core::platform::canonical_ids()
        .into_iter()
        .map(|id| (id, archivefs_core::platform::display_name_for(id)))
        .filter(|(id, display_name)| {
            needle.is_empty()
                || display_name.to_lowercase().contains(&needle)
                || id.to_lowercase().contains(&needle)
        })
        .take(MAX_PLATFORM_CHOICES)
        .collect()
}

/// How many canonical platforms match `query`, so the picker can say "showing
/// 12 of 30" honestly rather than implying the 12 are all there is.
pub(crate) fn platform_choice_count(query: &str) -> usize {
    let needle = query.trim().to_lowercase();
    archivefs_core::platform::canonical_ids()
        .into_iter()
        .filter(|id| {
            needle.is_empty()
                || archivefs_core::platform::display_name_for(id)
                    .to_lowercase()
                    .contains(&needle)
                || id.to_lowercase().contains(&needle)
        })
        .count()
}

/// A friendly name for a library folder button: its own name first, with the
/// full path shown as muted secondary text. Deep configured paths read as
/// giant buttons otherwise; a beginner recognises "GameCube" faster than
/// "/media/archives/library/GameCube".
fn friendly_folder_label(folder: &Path) -> String {
    folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| folder.display().to_string())
}

fn show_combined_audit_target_picker(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    ui.add_space(6.0);
    widgets::section_header(
        ui,
        "Check which files?",
        Some(
            "Every candidate is read once and compared against all available evidence. Nothing is renamed, moved, or written.",
        ),
    );
    if view.library_folders.is_empty() {
        ui.label(
            egui::RichText::new(
                "No library folders are configured. Choose another folder or one regular file.",
            )
            .color(theme::muted(ui)),
        );
    } else {
        for folder in &view.library_folders {
            if widgets::action_button(
                ui,
                &format!("Use {}", friendly_folder_label(folder)),
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
                && action.is_none()
            {
                action = Some(DatSourcesPageAction::AuditAllEnabled {
                    scan_root: folder.clone(),
                });
            }
            ui.label(
                egui::RichText::new(folder.display().to_string())
                    .color(theme::muted(ui))
                    .small(),
            );
        }
    }
    if widgets::action_button(
        ui,
        "Choose another folder…",
        widgets::ActionStyle::Quiet,
        true,
    )
    .clicked()
        && action.is_none()
        && let Some(path) = rfd::FileDialog::new().pick_folder()
    {
        action = Some(DatSourcesPageAction::AuditAllEnabled { scan_root: path });
    }
    if widgets::action_button(ui, "Choose one file…", widgets::ActionStyle::Quiet, true).clicked()
        && action.is_none()
        && let Some(path) = rfd::FileDialog::new()
            .set_title("Choose one file to identify")
            .pick_file()
    {
        action = Some(DatSourcesPageAction::AuditAllEnabled { scan_root: path });
    }
    action
}

fn show_audit_target_picker(
    ui: &mut egui::Ui,
    row: &DatSourceRowView,
    view: &DatSourcesPageView,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    ui.add_space(6.0);
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Check which files?",
            Some(
                "Choose a folder, or one regular file for a small evidence check. Nothing is \
                 renamed, moved, or written.",
            ),
        );
        if view.library_folders.is_empty() {
            ui.label(
                egui::RichText::new(
                    "No library source folders are configured, so there is nothing to offer here. \
                     Choose a folder instead.",
                )
                .color(theme::muted(ui)),
            );
        }
        for folder in &view.library_folders {
            // A two-line button: the friendly name first, the full path muted
            // underneath. Clicking anywhere on it starts the audit.
            let friendly = friendly_folder_label(folder);
            let full = folder.display().to_string();
            let clicked = egui::Frame::new()
                .fill(theme::card_fill(ui))
                .stroke(theme::border(ui))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Library folder: {friendly}")).strong(),
                        );
                        ui.label(egui::RichText::new(full).color(theme::muted(ui)).small());
                    });
                })
                .response
                .interact(egui::Sense::click());
            if clicked.clicked() && action.is_none() {
                action = Some(DatSourcesPageAction::Audit {
                    id: row.id.clone(),
                    scan_root: folder.clone(),
                });
            }
        }
        if widgets::action_button(
            ui,
            "Choose another folder…",
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
            && action.is_none()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Choose a folder to check")
                .pick_folder()
        {
            action = Some(DatSourcesPageAction::Audit {
                id: row.id.clone(),
                scan_root: path,
            });
        }
        if widgets::action_button(ui, "Choose one file…", widgets::ActionStyle::Quiet, true)
            .clicked()
            && action.is_none()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Choose one file to check")
                .pick_file()
        {
            action = Some(DatSourcesPageAction::Audit {
                id: row.id.clone(),
                scan_root: path,
            });
        }
    });
    action
}

fn show_inspect(ui: &mut egui::Ui, row: &DatSourceRowView) {
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Source details", None);
        let mut rows: Vec<(&str, String)> = vec![
            ("ID", row.id.clone()),
            ("Kind", row.kind_label.to_string()),
            ("Path", row.path.clone()),
            (
                "Enabled",
                if row.enabled { "yes" } else { "no" }.to_string(),
            ),
            ("Health", row.health_state.label().to_string()),
        ];
        if let Some(platform) = &row.platform_display {
            rows.push(("Platform", platform.clone()));
        }
        if !row.formats.is_empty() {
            rows.push(("Formats", row.formats.join(", ")));
        }
        if let Some(count) = row.entry_count {
            rows.push(("Catalogue entries", count.to_string()));
        }
        if let Some(count) = row.rom_count {
            rows.push(("Catalogue ROMs", count.to_string()));
        }
        if let Some(when) = &row.last_validated {
            rows.push(("Last checked", when.clone()));
        }
        for (label, value) in rows {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{label}:")).color(theme::muted(ui)));
                ui.label(value);
            });
        }
        if !row.health_state.is_checked() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "This source has not been checked yet, so nothing above describes its \
                     contents. Use Validate to read it.",
                )
                .color(theme::muted(ui))
                .small(),
            );
        }

        let Some(detail) = &row.detail else {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Run Validate to see the individual DAT files, their formats, and anything \
                     the parser had to say.",
                )
                .color(theme::muted(ui))
                .small(),
            );
            return;
        };

        ui.add_space(8.0);
        widgets::section_header(ui, "DAT files read", None);
        for file in &detail.files {
            ui.horizontal_top(|ui| {
                widgets::status_badge(
                    ui,
                    file.status,
                    if file.status == "Failed" {
                        widgets::StatusTone::Blocked
                    } else if file.status == "OK" {
                        widgets::StatusTone::Success
                    } else {
                        widgets::StatusTone::Warning
                    },
                );
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&file.file_name).strong());
                    ui.label(
                        egui::RichText::new(&file.detail)
                            .color(theme::muted(ui))
                            .small(),
                    );
                    for error in file.errors.iter().take(20) {
                        ui.label(
                            egui::RichText::new(format!("error: {error}"))
                                .color(widgets::StatusTone::Blocked.color(ui))
                                .small(),
                        );
                    }
                    for warning in file.warnings.iter().take(20) {
                        ui.label(
                            egui::RichText::new(format!("warning: {warning}"))
                                .color(widgets::StatusTone::Warning.color(ui))
                                .small(),
                        );
                    }
                    for note in file.notes.iter().take(20) {
                        ui.label(
                            egui::RichText::new(format!("note: {note}"))
                                .color(theme::muted(ui))
                                .small(),
                        );
                    }
                });
            });
        }

        if !detail.duplicate_identities.is_empty() {
            ui.add_space(6.0);
            widgets::section_header(
                ui,
                "Conflicting catalogue identities",
                Some(
                    "More than one file claims to be the same catalogue. Both are still read; \
                     EmuWiz does not pick one for you.",
                ),
            );
            for line in &detail.duplicate_identities {
                ui.label(line);
            }
        }

        if !detail.skipped.is_empty() {
            ui.add_space(6.0);
            widgets::section_header(
                ui,
                "Files not used",
                Some("Looked at and left alone, so nothing is missed silently."),
            );
            for line in detail.skipped.iter().take(50) {
                ui.label(egui::RichText::new(line).small());
            }
        }

        if detail.truncated {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Partial listing",
                "This folder holds more DAT files than one check reads. Split it, or register the \
                 subfolders separately, for a complete picture.",
                widgets::StatusTone::Warning,
            );
        }
    });
}

/// The policy annotation of an audit: which candidates the user's preferences
/// prefer for each multi-candidate file, and what the policy could not decide.
///
/// This is display of the core's resolution - nothing here ranks, and the
/// verdicts themselves are never touched.
fn show_audit_policy_notes(ui: &mut egui::Ui, policy: &AuditPolicyView) {
    if policy.notes.is_empty() {
        return;
    }
    widgets::section_header(
        ui,
        "Policy preference",
        Some(
            "Your DAT matching preferences, applied to the files whose hash matched several \
             catalogue entries. Nothing is renamed or changed; this is the preferred order only.",
        ),
    );
    ui.label(
        egui::RichText::new(format!(
            "Sources consulted: {}",
            if policy.source_ordering.is_empty() {
                "none".to_string()
            } else {
                policy.source_ordering.join(" → ")
            }
        ))
        .color(theme::muted(ui))
        .small(),
    );
    for note in &policy.notes {
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            ui.label(egui::RichText::new(note.file_name.clone()).strong());
            ui.label(
                egui::RichText::new(note.verdict_label.clone())
                    .color(theme::muted(ui))
                    .small(),
            );
        });
        for line in &note.ranked {
            ui.label(egui::RichText::new(line.clone()).small().monospace());
        }
        for line in &note.explanations {
            ui.label(
                egui::RichText::new(format!("• {line}"))
                    .color(theme::muted(ui))
                    .small(),
            );
        }
        if let Some(winner) = &note.winner {
            ui.label(
                egui::RichText::new(format!("Preferred: {winner}"))
                    .color(theme::SUCCESS)
                    .small(),
            );
        }
        if note.ambiguous {
            ui.label(
                egui::RichText::new(format!(
                    "Ambiguous: {}",
                    note.ambiguity_reason
                        .as_deref()
                        .unwrap_or("the policy cannot decide")
                ))
                .color(theme::WARNING)
                .small(),
            );
        }
    }
}

/// The DAT Matching Policy section: preference editors plus the Effective
/// Policy Summary. Every value drawn here comes from the core resolver; the
/// drawing code only turns actions into edits.
fn show_dat_policy_section(
    ui: &mut egui::Ui,
    view: &DatPolicyView,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    ui.add_space(10.0);
    widgets::section_header(
        ui,
        "DAT matching policy",
        Some(
            "How EmuWiz prefers one verified candidate over another. Nothing here renames, \
             moves, deletes or rewrites a file.",
        ),
    );

    widgets::card(ui, |ui| {
        if !view.editable {
            ui.label(
                egui::RichText::new(
                    "The registry could not be read, so the policy cannot be edited until that \
                     is fixed.",
                )
                .color(theme::muted(ui)),
            );
            return;
        }

        // Scope selector: which platform the preferences and summary apply to.
        ui.horizontal(|ui| {
            ui.label("Applies to:");
            egui::ComboBox::from_id_salt("dat-policy-scope")
                .selected_text(&view.scope_label)
                .show_ui(ui, |ui| {
                    for option in &view.scopes_available {
                        if ui
                            .selectable_label(option.id == view.scope, &option.label)
                            .clicked()
                        {
                            action = Some(DatSourcesPageAction::SelectPolicyScope {
                                scope: option.id.clone(),
                            });
                        }
                    }
                });
        });
        ui.label(
            egui::RichText::new(match view.scope {
                None => "Editing: Global defaults".to_string(),
                Some(_) => format!("Editing: {} settings", view.scope_label),
            })
            .color(theme::muted(ui))
            .small(),
        );
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label("Show:");
            egui::ComboBox::from_id_salt("dat-policy-content")
                .selected_text(view.content_selection.label())
                .show_ui(ui, |ui| {
                    for policy in ContentSelectionPolicy::ALL {
                        if ui
                            .selectable_label(view.content_selection == policy, policy.label())
                            .clicked()
                        {
                            action = Some(DatSourcesPageAction::SetContentSelection {
                                scope: view.scope.clone(),
                                policy,
                            });
                        }
                    }
                });
        });
        ui.label(
            egui::RichText::new(GAMES_ONLY_EXPLANATION)
                .color(theme::muted(ui))
                .small(),
        );
        ui.add_space(10.0);

        if let Some(policy_action) = show_region_preference_editor(ui, view) {
            action = Some(policy_action);
        }
        ui.add_space(10.0);
        if let Some(policy_action) = show_language_preference_editor(ui, view) {
            action = Some(policy_action);
        }
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label("Revision policy:");
            egui::ComboBox::from_id_salt("dat-policy-revision")
                .selected_text(view.revision_policy.label())
                .show_ui(ui, |ui| {
                    for policy in RevisionPolicy::ALL {
                        if ui
                            .selectable_label(view.revision_policy == policy, policy.label())
                            .clicked()
                        {
                            action = Some(DatSourcesPageAction::SetRevisionPolicy {
                                scope: view.scope.clone(),
                                policy,
                            });
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Parent/clone handling:");
            egui::ComboBox::from_id_salt("dat-policy-clone")
                .selected_text(view.clone_policy.label())
                .show_ui(ui, |ui| {
                    for policy in ClonePolicy::ALL {
                        if ui
                            .selectable_label(view.clone_policy == policy, policy.label())
                            .clicked()
                        {
                            action = Some(DatSourcesPageAction::SetClonePolicy {
                                scope: view.scope.clone(),
                                policy,
                            });
                        }
                    }
                });
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(SAFE_PROMISE)
                .color(theme::muted(ui))
                .small(),
        );

        if !view.problems.is_empty() {
            ui.add_space(8.0);
            widgets::banner(
                ui,
                "Policy file kept as written",
                &format!(
                    "{} These values are preserved but not applied until fixed by hand or by a \
                     newer build.",
                    view.problems.join(" ")
                ),
                widgets::StatusTone::Warning,
            );
        }
    });

    ui.add_space(10.0);
    show_effective_policy_summary(ui, view);

    action
}

/// One ordered preference list with move/remove, plus an "Add" affordance.
fn show_preference_rows(
    ui: &mut egui::Ui,
    rows: &[PolicyPreferenceRowView],
    move_action: impl Fn(usize, i32) -> DatSourcesPageAction,
    remove_action: impl Fn(usize) -> DatSourcesPageAction,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    for row in rows {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{}.", row.position)).monospace());
            ui.label(&row.label);
            if row.position > 1
                && ui
                    .add_enabled(true, egui::Button::new("↑").small())
                    .on_hover_text("Move up")
                    .clicked()
            {
                action = Some(move_action(row.position - 1, -1));
            }
            if row.position < rows.len()
                && ui
                    .add_enabled(true, egui::Button::new("↓").small())
                    .on_hover_text("Move down")
                    .clicked()
            {
                action = Some(move_action(row.position - 1, 1));
            }
            if ui
                .add_enabled(true, egui::Button::new("Remove").small())
                .on_hover_text("Remove from the preference list")
                .clicked()
            {
                action = Some(remove_action(row.position - 1));
            }
        });
    }
    action
}

fn show_region_preference_editor(
    ui: &mut egui::Ui,
    view: &DatPolicyView,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Preferred regions").strong());
        if view.region_preferences.is_empty() {
            ui.label(
                egui::RichText::new("Any region — no preference")
                    .color(theme::muted(ui))
                    .small(),
            );
        }
    });
    ui.label(
        egui::RichText::new(
            "When several regions of a game are verified, prefer them in this order.",
        )
        .color(theme::muted(ui))
        .small(),
    );
    if let Some(policy_action) = show_preference_rows(
        ui,
        &view.region_preferences,
        |index, delta| DatSourcesPageAction::MoveRegion {
            scope: view.scope.clone(),
            index,
            delta,
        },
        |index| DatSourcesPageAction::RemoveRegion {
            scope: view.scope.clone(),
            index,
        },
    ) {
        action = Some(policy_action);
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Add:").color(theme::muted(ui)));
        for region in RegionId::ALL {
            let present = view
                .region_preferences
                .iter()
                .any(|row| row.value == region.as_str());
            if !present && ui.add(egui::Button::new(region.label()).small()).clicked() {
                action = Some(DatSourcesPageAction::AddRegion {
                    scope: view.scope.clone(),
                    region,
                });
            }
        }
        // "Any region" clears the ordering back to no preference.
        if !view.region_preferences.is_empty()
            && ui
                .add(egui::Button::new("Any region").small())
                .on_hover_text("Clear the region preference so every region is treated as equal.")
                .clicked()
        {
            action = Some(DatSourcesPageAction::ClearRegion {
                scope: view.scope.clone(),
            });
        }
    });
    action
}

fn show_language_preference_editor(
    ui: &mut egui::Ui,
    view: &DatPolicyView,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Preferred languages").strong());
        if view.language_preferences.is_empty() {
            ui.label(
                egui::RichText::new("Any language — no preference")
                    .color(theme::muted(ui))
                    .small(),
            );
        }
    });
    ui.label(
        egui::RichText::new(
            "When several languages of a game are verified, prefer them in this order. \
             Multi-language matches any entry with more than one language tag; Original matches \
             the release region's own language.",
        )
        .color(theme::muted(ui))
        .small(),
    );
    if let Some(policy_action) = show_preference_rows(
        ui,
        &view.language_preferences,
        |index, delta| DatSourcesPageAction::MoveLanguage {
            scope: view.scope.clone(),
            index,
            delta,
        },
        |index| DatSourcesPageAction::RemoveLanguage {
            scope: view.scope.clone(),
            index,
        },
    ) {
        action = Some(policy_action);
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Add:").color(theme::muted(ui)));
        let present: Vec<&str> = view
            .language_preferences
            .iter()
            .map(|row| row.value.as_str())
            .collect();
        egui::ComboBox::from_id_salt("dat-policy-add-language")
            .selected_text("Choose a language…")
            .width(220.0)
            .show_ui(ui, |ui| {
                if !present.contains(&"multi")
                    && ui
                        .selectable_label(false, LanguagePreference::MultiLanguage.label())
                        .clicked()
                {
                    action = Some(DatSourcesPageAction::AddLanguage {
                        scope: view.scope.clone(),
                        preference: LanguagePreference::MultiLanguage,
                    });
                }
                if !present.contains(&"original")
                    && ui
                        .selectable_label(false, LanguagePreference::OriginalLanguage.label())
                        .clicked()
                {
                    action = Some(DatSourcesPageAction::AddLanguage {
                        scope: view.scope.clone(),
                        preference: LanguagePreference::OriginalLanguage,
                    });
                }
                for language in LanguageId::ALL {
                    let present = present.contains(&language.as_str());
                    if !present && ui.selectable_label(false, language.label()).clicked() {
                        action = Some(DatSourcesPageAction::AddLanguage {
                            scope: view.scope.clone(),
                            preference: LanguagePreference::Language(language),
                        });
                    }
                }
            });
        // "Any language" clears the ordering back to no preference.
        if !view.language_preferences.is_empty()
            && ui
                .add(egui::Button::new("Any language").small())
                .on_hover_text(
                    "Clear the language preference so every language is treated as equal.",
                )
                .clicked()
        {
            action = Some(DatSourcesPageAction::ClearLanguage {
                scope: view.scope.clone(),
            });
        }
    });
    action
}

/// The Effective Policy Summary: the resolved policy for the current scope,
/// where each value came from, and the source consultation order.
fn show_effective_policy_summary(ui: &mut egui::Ui, view: &DatPolicyView) {
    widgets::section_header(
        ui,
        "Effective policy",
        Some(
            "What will actually be applied, after any platform overrides. Resolved by the same \
             core the CLI uses.",
        ),
    );
    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new(format!("Platform: {}", view.effective.platform)).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Sources consulted:").strong());
            if view.effective.source_ordering.is_empty() {
                ui.label(
                    egui::RichText::new("none enabled for this scope")
                        .color(theme::muted(ui))
                        .small(),
                );
            } else {
                ui.vertical(|ui| {
                    for source in &view.effective.source_ordering {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}. {} (priority {})",
                                source.consulted_position, source.display_name, source.priority
                            ))
                            .small(),
                        );
                    }
                });
            }
        });
        ui.add_space(4.0);
        summary_row(ui, "Region preference", &view.effective.region);
        summary_row(ui, "Language preference", &view.effective.language);
        summary_row(ui, "Revision rule", &view.effective.revision);
        summary_row(ui, "Clone rule", &view.effective.clone);
        summary_row(ui, "Show", &view.effective.content);
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new("Where each value comes from").strong());
        for (field, scope) in &view.effective.source_of {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(field).small());
                ui.label(egui::RichText::new(scope).color(theme::muted(ui)).small());
            });
        }
    });
}

fn summary_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{label}:")).strong());
        ui.label(value);
    });
}

/// A human-facing summary of what a rename transaction did, leading with
/// the action and result rather than a raw transaction ID - e.g.
/// `Renamed "old.zip" -> "new.zip"` for a single entry, or `Renamed 12
/// files` for many. Built once, purely from the same
/// `TransactionEntry::original_basename`/`proposed_basename` the journal
/// already records - never a new persisted field, never a change to the
/// journal format.
fn rename_transaction_human_summary(entries: &[TransactionEntry]) -> String {
    match entries {
        [] => "No files".to_string(),
        [only] => format!(
            "Renamed \"{}\" -> \"{}\"",
            only.original_basename, only.proposed_basename
        ),
        many => format!("Renamed {} files", many.len()),
    }
}

/// The human-facing state word for a rename transaction's primary summary
/// line - deliberately coarser than [`TransactionState::label`] (which
/// keeps its full technical granularity for Technical details): any
/// state that is not a settled `Applied` or `RolledBack` means something
/// is unresolved and needs a person to look at it, not one of five
/// different in-progress/failure words a normal user has no use for on
/// the primary line.
fn recovery_human_state_label(state: TransactionState) -> &'static str {
    match state {
        TransactionState::Applied => "Applied",
        TransactionState::RolledBack => "Rolled back",
        TransactionState::Planned
        | TransactionState::Applying
        | TransactionState::ApplyFailed
        | TransactionState::RollingBack
        | TransactionState::RollbackFailed => "Needs attention",
    }
}

/// The tone of a proposal-state badge.
fn plan_state_tone(state: ProposalState) -> widgets::StatusTone {
    match state {
        ProposalState::Suggested => widgets::StatusTone::Success,
        ProposalState::AlreadyCanonical => widgets::StatusTone::Info,
        ProposalState::Ambiguous => widgets::StatusTone::Warning,
        ProposalState::Conflict => widgets::StatusTone::Warning,
        ProposalState::Unsupported => widgets::StatusTone::Info,
        ProposalState::Blocked | ProposalState::UnclassifiedContent => widgets::StatusTone::Blocked,
        ProposalState::ExcludedByContentPolicy => widgets::StatusTone::Info,
    }
}

/// Whether a recovery transaction's audited folder (`transaction_scan_root`,
/// recorded verbatim from
/// [`archivefs_core::dat::rename_apply::RenameTransaction::source_scan_root`])
/// is the same folder as - or an ancestor/descendant of - `current_root`.
///
/// Plain path-component comparison only: no `canonicalize`, no symlink
/// resolution, no guessing across differently-spelled-but-equivalent paths.
/// An empty or otherwise unreadable scan root is never trusted to mean
/// "unrelated" - the caller must not guess relevance it cannot prove.
fn transaction_targets_root(transaction_scan_root: &str, current_root: &str) -> bool {
    if transaction_scan_root.trim().is_empty() {
        return true;
    }
    let transaction_path = Path::new(transaction_scan_root);
    let current_path = Path::new(current_root);
    transaction_path == current_path
        || current_path.starts_with(transaction_path)
        || transaction_path.starts_with(current_path)
}

/// Renders one card per recovery/crash-recovery transaction. Shared by the
/// advanced planner (every transaction, always shown directly) and Quick
/// Rename (split into a directly-shown "blocking" subset and a collapsed
/// "settled/other-folder" subset - see `show_quick_rename_page`).
fn show_recovery_transactions(
    ui: &mut egui::Ui,
    recoveries: &[RecoveryTransactionView],
    rollback_running: bool,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    widgets::card(ui, |ui| {
        for recovery in recoveries {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&recovery.human_summary).strong());
                ui.label(
                    egui::RichText::new(format!(
                        "({})",
                        recovery_human_state_label(recovery.state)
                    ))
                    .color(theme::muted(ui)),
                );
            });
            // The raw transaction ID and exact applied/total counts are
            // developer-facing detail, not meaningful to a normal user
            // deciding whether to roll back - moved behind Technical
            // details, same as History & Logs already does for its own
            // transaction IDs. `transaction_id` is unique per real
            // transaction, so it alone is a safe, stable per-row salt.
            widgets::technical_details(
                ui,
                ("rename_recovery_technical_detail", &recovery.transaction_id),
                |ui| {
                    widgets::copyable_value(ui, "Transaction ID", &recovery.transaction_id);
                    ui.label(format!(
                        "State: {} ({} applied of {})",
                        recovery.state.label(),
                        recovery.applied_count,
                        recovery.total_count
                    ));
                },
            );
            if let Some(resolution) = recovery.resolution {
                // Already resolved: `state` still truthfully says
                // "interrupted" (never rewritten - see
                // `RecoveryResolution`'s own doc), so this must never be
                // presented as settled/Applied. The user is not asked to
                // decide again, but rollback stays offered on exactly the
                // same terms as it would for an unresolved interrupted
                // transaction (matching `RenameTransaction::is_rollbackable`,
                // which depends only on `state.needs_recovery()` here, never
                // on `applied_count` or on whether a resolution exists) -
                // acknowledging the prompt must never quietly take away the
                // ability to undo it.
                ui.label(
                    egui::RichText::new(format!(
                        "Interrupted - {}",
                        resolution.label().to_ascii_lowercase()
                    ))
                    .color(theme::muted(ui))
                    .small(),
                );
                ui.add_space(4.0);
                if widgets::action_button(
                    ui,
                    "Roll back transaction",
                    widgets::ActionStyle::Destructive,
                    !rollback_running,
                )
                .clicked()
                {
                    action = Some(DatSourcesPageAction::RecoveryChoice {
                        id: recovery.transaction_id.clone(),
                        choice: RecoveryChoice::RollBack,
                    });
                }
            } else {
                let (explanation, rollback_label) = if recovery.state == TransactionState::Applied {
                    (
                        "A completed rename transaction is still applied and can be rolled \
                             back.",
                        "Roll back transaction",
                    )
                } else {
                    (
                        "An interrupted rename transaction was found. EmuWiz will never \
                             resume it automatically.",
                        "Roll back completed steps",
                    )
                };
                ui.label(
                    egui::RichText::new(explanation)
                        .color(theme::muted(ui))
                        .small(),
                );
                ui.horizontal(|ui| {
                    if widgets::action_button(
                        ui,
                        rollback_label,
                        widgets::ActionStyle::Destructive,
                        !rollback_running,
                    )
                    .clicked()
                    {
                        action = Some(DatSourcesPageAction::RecoveryChoice {
                            id: recovery.transaction_id.clone(),
                            choice: RecoveryChoice::RollBack,
                        });
                    }
                    if widgets::action_button(
                        ui,
                        "Leave untouched",
                        widgets::ActionStyle::Quiet,
                        !rollback_running,
                    )
                    .clicked()
                    {
                        action = Some(DatSourcesPageAction::RecoveryChoice {
                            id: recovery.transaction_id.clone(),
                            choice: RecoveryChoice::LeaveUntouched,
                        });
                    }
                });
            }
            ui.add_space(6.0);
        }
    });
    action
}

/// The gated apply and crash-recovery section.
///
/// This is the only place the advanced planner offers a rename: an
/// explicitly approved, actionable batch is built by the core, shown
/// read-only, confirmed (with a typed phrase for large batches), and
/// executed by the core executor on a worker thread. The GUI never calls
/// `std::fs::rename`. Quick Rename's own simple confirmation/success cards
/// (`show_quick_rename_confirmation`, `show_quick_rename_success`) render
/// the exact same [`RenameApplyView`] data through friendlier UI instead of
/// calling this - see `show_quick_rename_page`.
fn show_rename_apply_section(
    ui: &mut egui::Ui,
    apply: &RenameApplyView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;

    // Transactions found on disk first: a settled `Applied` batch is offered
    // for rollback, and an interrupted batch is surfaced for crash recovery -
    // never auto-resumed.
    if !apply.recovery.is_empty() {
        ui.add_space(10.0);
        widgets::section_header(ui, "Rename transactions", None);
        if let Some(recovery_action) =
            show_recovery_transactions(ui, &apply.recovery, apply.rollback_running)
        {
            action = Some(recovery_action);
        }
        if let Some(error) = &apply.recovery_resolution_error {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Your choice could not be saved",
                error,
                widgets::StatusTone::Blocked,
            );
        }
    }

    if action.is_none()
        && let Some(review_action) = show_rename_apply_review_and_outcome(ui, apply, ui_state)
    {
        action = Some(review_action);
    }
    action
}

/// The review/confirm/outcome/rollback body of the apply section, without
/// the recovery-transaction cards above it - split out so Quick Rename's
/// simple path can show its own recovery split (see `show_quick_rename_page`)
/// while still reaching this exact technical rendering for its "Review
/// changes" (advanced) route, unchanged from what the advanced planner has
/// always shown.
fn show_rename_apply_review_and_outcome(
    ui: &mut egui::Ui,
    apply: &RenameApplyView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    if let Some(review) = &apply.review {
        ui.add_space(10.0);
        widgets::section_header(
            ui,
            "Review approved renames",
            Some("Nothing is renamed until you confirm. The renames stay inside the trusted root."),
        );
        widgets::card(ui, |ui| {
            if let Some(root) = &review.trusted_root {
                ui.label(
                    egui::RichText::new(format!("Trusted root: {root}"))
                        .color(theme::muted(ui))
                        .small(),
                );
            }
            ui.label(
                egui::RichText::new(format!("{} rename(s) to apply", review.rows.len())).strong(),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .id_salt("dat-apply-review-rows")
                .show(ui, |ui| {
                    for row in &review.rows {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&row.current_basename).monospace());
                            ui.label(egui::RichText::new("→").color(theme::muted(ui)));
                            ui.label(egui::RichText::new(&row.proposed_basename).monospace());
                        });
                    }
                });
            ui.add_space(6.0);
            if let Some(phrase) = &review.required_phrase {
                ui.label(
                    egui::RichText::new(format!("Type {phrase} to confirm:"))
                        .color(theme::WARNING)
                        .small(),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut ui_state.plan_typed_confirmation)
                        .hint_text(phrase)
                        .id_salt("dat-apply-typed-confirmation"),
                );
                ui.add_space(4.0);
            }
            if apply.apply_running {
                ui.label(egui::RichText::new("Applying…").color(theme::muted(ui)));
            } else {
                let typed = ui_state.plan_typed_confirmation.clone();
                let can_confirm = match &review.required_phrase {
                    Some(phrase) => typed.trim() == *phrase,
                    None => true,
                };
                ui.horizontal(|ui| {
                    if widgets::action_button(
                        ui,
                        "Apply approved renames",
                        widgets::ActionStyle::Destructive,
                        can_confirm,
                    )
                    .clicked()
                    {
                        action = Some(DatSourcesPageAction::ConfirmApply {
                            typed: typed.clone(),
                        });
                    }
                    if apply.subset_available
                        && widgets::action_button(
                            ui,
                            "Apply only the independently safe subset",
                            widgets::ActionStyle::Secondary,
                            can_confirm,
                        )
                        .clicked()
                    {
                        action = Some(DatSourcesPageAction::ConfirmApplySafeSubset { typed });
                    }
                    if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        action = Some(DatSourcesPageAction::CancelApplyReview);
                    }
                });
            }
        });
    }

    if let Some(error) = &apply.apply_error {
        ui.add_space(8.0);
        widgets::banner(ui, "Apply did not run", error, widgets::StatusTone::Blocked);
    }

    if let Some(outcome) = &apply.outcome {
        ui.add_space(10.0);
        widgets::section_header(
            ui,
            "Rename transaction",
            Some(&format!(
                "requested {} · applied {} · skipped {} · failed {} · {}",
                outcome.requested,
                outcome.applied,
                outcome.skipped,
                outcome.failed,
                outcome.state.label()
            )),
        );
        widgets::card(ui, |ui| {
            ui.label(
                egui::RichText::new(format!("Transaction: {}", outcome.transaction_id))
                    .color(theme::muted(ui))
                    .small(),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .id_salt("dat-apply-outcome-rows")
                .show(ui, |ui| {
                    for row in &outcome.rows {
                        ui.horizontal(|ui| {
                            widgets::status_badge(ui, row.state.label(), apply_row_tone(row.state));
                            ui.label(egui::RichText::new(&row.current_basename).monospace());
                            ui.label(egui::RichText::new("→").color(theme::muted(ui)));
                            ui.label(egui::RichText::new(&row.proposed_basename).monospace());
                        });
                        if let Some(reason) = &row.failure_reason {
                            ui.label(egui::RichText::new(reason).color(theme::WARNING).small());
                        }
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    "Roll back transaction",
                    widgets::ActionStyle::Destructive,
                    !apply.rollback_running && outcome.applied > 0,
                )
                .clicked()
                {
                    action = Some(DatSourcesPageAction::RollbackTransaction {
                        id: outcome.transaction_id.clone(),
                    });
                }
                if widgets::action_button(ui, "Dismiss", widgets::ActionStyle::Quiet, true)
                    .clicked()
                {
                    action = Some(DatSourcesPageAction::ClearApplyOutcome);
                }
            });
        });
    }

    if let Some(result) = &apply.rollback_result {
        ui.add_space(8.0);
        widgets::banner(
            ui,
            result.label,
            &result.detail,
            match result.label {
                "Fully rolled back" => widgets::StatusTone::Success,
                _ => widgets::StatusTone::Warning,
            },
        );
    }

    if let Some(error) = &apply.rollback_error {
        ui.add_space(8.0);
        widgets::banner(
            ui,
            "Rollback could not run",
            error,
            widgets::StatusTone::Blocked,
        );
    }

    action
}

fn apply_row_tone(state: EntryState) -> widgets::StatusTone {
    match state {
        EntryState::Applied | EntryState::RolledBack => widgets::StatusTone::Success,
        EntryState::Skipped => widgets::StatusTone::Info,
        EntryState::ApplyFailed | EntryState::RollbackFailed => widgets::StatusTone::Blocked,
        _ => widgets::StatusTone::Info,
    }
}

/// Whether a skip/failure reason names a collision specifically (the
/// destination already existing, a case-only collision, or two proposals
/// landing on the same target) rather than some other preflight refusal
/// (source vanished, became a symlink, moved outside the trusted root, …).
/// Display-only bucketing for Quick Rename's success summary - it reads an
/// already-decided [`PreflightFailure::reason`] string, never re-derives or
/// weakens the refusal itself.
fn is_collision_reason(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    // Matches exactly the three `PreflightFailure` reasons that name a real
    // destination collision - `DestinationExists` ("now exists"),
    // `DestinationCaseCollision` ("only by case"), and `ConflictingBatchTarget`
    // ("target the same destination", its literal wording). Deliberately
    // narrow: every other preflight refusal (source vanished, became a
    // symlink, left the trusted root, a stale generation, …) must keep
    // counting as an ordinary skip, not a collision.
    lower.contains("now exists")
        || lower.contains("only by case")
        || lower.contains("target the same destination")
}

/// Quick Rename's compact confirmation card - the "simple confirmation"
/// step for its normal path. Renders exactly the [`ApplyReviewView`] the
/// advanced planner's `show_rename_apply_review_and_outcome` also shows
/// (same `build_transaction`, same typed-phrase safety gate for large
/// batches via `required_phrase`, same safe-subset offer after a hard
/// conflict), just without planner terminology, a trusted-root path, or a
/// full old->new file listing. Confirming issues the exact same
/// [`DatSourcesPageAction::ConfirmApply`] / `ConfirmApplySafeSubset` the
/// advanced path uses; nothing here calls `std::fs::rename`.
fn show_quick_rename_confirmation(
    ui: &mut egui::Ui,
    review: &ApplyReviewView,
    unsupported: usize,
    ambiguous: usize,
    conflicts: usize,
    apply: &RenameApplyView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    let count = review.rows.len();
    ui.add_space(10.0);
    widgets::card(ui, |ui| {
        ui.label(
            egui::RichText::new(format!(
                "Ready to rename {count} verified file{}.",
                if count == 1 { "" } else { "s" }
            ))
            .strong(),
        );
        ui.add_space(4.0);
        if unsupported > 0 {
            ui.label(format!(
                "{unsupported} unsupported file{} will remain unchanged.",
                if unsupported == 1 { "" } else { "s" }
            ));
        }
        ui.label(format!("{ambiguous} ambiguous."));
        ui.label(format!(
            "{conflicts} conflict{}.",
            if conflicts == 1 { "" } else { "s" }
        ));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("A recovery journal will be created.")
                .color(theme::muted(ui))
                .small(),
        );
        ui.add_space(8.0);
        if let Some(phrase) = &review.required_phrase {
            ui.label(egui::RichText::new(format!("Type {phrase} to confirm:")).small());
            ui.add(
                egui::TextEdit::singleline(&mut ui_state.plan_typed_confirmation)
                    .hint_text(phrase)
                    .id_salt("quick-rename-typed-confirmation"),
            );
            ui.add_space(4.0);
        }
        if apply.apply_running {
            ui.label(egui::RichText::new("Renaming…").color(theme::muted(ui)));
        } else {
            let typed = ui_state.plan_typed_confirmation.clone();
            let can_confirm = match &review.required_phrase {
                Some(phrase) => typed.trim() == *phrase,
                None => true,
            };
            ui.horizontal(|ui| {
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked()
                {
                    action = Some(DatSourcesPageAction::CancelApplyReview);
                }
                if widgets::action_button(
                    ui,
                    "Rename files",
                    widgets::ActionStyle::Destructive,
                    can_confirm,
                )
                .clicked()
                {
                    action = Some(DatSourcesPageAction::ConfirmApply {
                        typed: typed.clone(),
                    });
                }
                if apply.subset_available
                    && widgets::action_button(
                        ui,
                        "Rename the safe subset only",
                        widgets::ActionStyle::Secondary,
                        can_confirm,
                    )
                    .clicked()
                {
                    action = Some(DatSourcesPageAction::ConfirmApplySafeSubset { typed });
                }
            });
        }
    });
    if let Some(error) = &apply.apply_error {
        ui.add_space(8.0);
        widgets::banner(
            ui,
            "Rename did not run",
            error,
            widgets::StatusTone::Blocked,
        );
    }
    action
}

/// Quick Rename's compact success card - shown once its own (non-advanced)
/// confirmation has been applied. Reads the same [`ApplyOutcomeView`] the
/// advanced planner's technical "Rename transaction" card shows; the
/// itemized per-file rows and rollback control are unchanged, just tucked
/// behind "View details" instead of always on screen.
fn show_quick_rename_success(
    ui: &mut egui::Ui,
    outcome: &ApplyOutcomeView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    let collisions = outcome
        .rows
        .iter()
        .filter(|row| {
            row.state == EntryState::Skipped
                && row
                    .failure_reason
                    .as_deref()
                    .is_some_and(is_collision_reason)
        })
        .count();
    ui.add_space(10.0);
    widgets::card(ui, |ui| {
        ui.label(
            egui::RichText::new("Quick Rename complete")
                .strong()
                .size(18.0),
        );
        ui.add_space(6.0);
        ui.label(format!(
            "{} file{} renamed",
            outcome.applied,
            if outcome.applied == 1 { "" } else { "s" }
        ));
        ui.label(format!("{} left unchanged", outcome.skipped));
        ui.label(format!("{} failed", outcome.failed));
        ui.label(format!(
            "{collisions} collision{}",
            if collisions == 1 { "" } else { "s" }
        ));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Recovery journal saved.")
                .color(theme::muted(ui))
                .small(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if widgets::action_button(
                ui,
                "Rename another library",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                action = Some(DatSourcesPageAction::ResetQuickRenameSession);
            }
            if widgets::action_button(ui, "Done", widgets::ActionStyle::Secondary, true).clicked() {
                action = Some(DatSourcesPageAction::ClearApplyOutcome);
            }
            let label = if ui_state.quick_success_details_open {
                "Hide details"
            } else {
                "View details"
            };
            if widgets::action_button(ui, label, widgets::ActionStyle::Secondary, true).clicked() {
                ui_state.quick_success_details_open = !ui_state.quick_success_details_open;
            }
        });
        if ui_state.quick_success_details_open {
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .id_salt("quick-rename-outcome-rows")
                .show(ui, |ui| {
                    for row in &outcome.rows {
                        ui.horizontal(|ui| {
                            widgets::status_badge(ui, row.state.label(), apply_row_tone(row.state));
                            ui.label(egui::RichText::new(&row.current_basename).monospace());
                            ui.label(egui::RichText::new("→").color(theme::muted(ui)));
                            ui.label(egui::RichText::new(&row.proposed_basename).monospace());
                        });
                        if let Some(reason) = &row.failure_reason {
                            ui.label(egui::RichText::new(reason).color(theme::WARNING).small());
                        }
                    }
                });
            ui.add_space(6.0);
            if outcome.applied > 0
                && widgets::action_button(
                    ui,
                    "Roll back these renames",
                    widgets::ActionStyle::Destructive,
                    true,
                )
                .clicked()
            {
                action = Some(DatSourcesPageAction::RollbackTransaction {
                    id: outcome.transaction_id.clone(),
                });
            }
        }
    });
    action
}

/// The read-only rename-planning section.
///
/// Everything here displays the core plan; the only user actions are review
/// decisions (session-only, never touching a file) and copying a proposed
/// name. There is no Rename/Apply/Execute/Move/Delete control of any kind.
fn show_rename_plan_section(
    ui: &mut egui::Ui,
    plan: &RenamePlanView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    ui.add_space(10.0);
    widgets::section_header(ui, "Rename planning", None);

    widgets::banner(ui, "Planning only", SAFE_PROMISE, widgets::StatusTone::Info);
    ui.label(
        egui::RichText::new(PLAN_ONLY_PROMISE)
            .color(theme::muted(ui))
            .small(),
    );

    widgets::card(ui, |ui| {
        ui.label(
            egui::RichText::new(format!(
                "Source '{}' · {} · {} of {} audited files verified",
                plan.source_display_name,
                plan.scan_root_short,
                plan.verified_total,
                plan.audited_total
            ))
            .color(theme::muted(ui))
            .small(),
        );
        if let Some(platform) = &plan.platform_display {
            ui.label(
                egui::RichText::new(format!("Platform: {platform}"))
                    .color(theme::muted(ui))
                    .small(),
            );
        }
        if plan.truncated {
            ui.add_space(4.0);
            widgets::banner(
                ui,
                "Partial plan",
                "The audit hit a ceiling, so this plan covers part of the folder.",
                widgets::StatusTone::Warning,
            );
        }
        if let Some(error) = &plan.error {
            ui.add_space(4.0);
            widgets::banner(ui, "Plan not produced", error, widgets::StatusTone::Warning);
        }

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} suggested · {} already canonical · {} ambiguous · {} conflicts · {} \
                     unsupported · {} blocked · {} not selected · {} unknown",
                    plan.counts.suggested,
                    plan.counts.already_canonical,
                    plan.counts.ambiguous,
                    plan.counts.conflicts,
                    plan.counts.unsupported,
                    plan.counts.blocked,
                    plan.counts.excluded_by_content_policy,
                    plan.counts.unclassified_content
                ))
                .small(),
            );
        });

        // Filter row: which states are shown. Filters only change what is
        // drawn; they never decide anything about the plan.
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Show:").color(theme::muted(ui)));
            for filter in RenamePlanFilter::ALL {
                let selected = ui_state.plan_filter == filter;
                if ui.selectable_label(selected, filter.label()).clicked() {
                    ui_state.plan_filter = filter;
                    ui_state.plan_page = 0;
                }
            }
        });
    });

    ui.add_space(8.0);
    widgets::card(ui, |ui| {
        let visible: Vec<&RenamePlanRowView> = plan
            .rows
            .iter()
            .filter(|row| ui_state.plan_filter.matches(row))
            .collect();
        if visible.is_empty() {
            ui.label(
                egui::RichText::new("No proposals match this filter.").color(theme::muted(ui)),
            );
            return;
        }
        let (start, end, page_count) = rename_plan_page_bounds(visible.len(), ui_state.plan_page);
        ui_state.plan_page = ui_state.plan_page.min(page_count - 1);
        if page_count > 1 {
            ui.horizontal(|ui| {
                let at_first_page = ui_state.plan_page == 0;
                if ui
                    .add_enabled(!at_first_page, egui::Button::new("← Previous page"))
                    .clicked()
                {
                    ui_state.plan_page -= 1;
                }
                ui.label(format!(
                    "Page {} of {page_count} · showing {}-{} of {}",
                    ui_state.plan_page + 1,
                    start + 1,
                    end,
                    visible.len()
                ));
                let at_last_page = ui_state.plan_page + 1 >= page_count;
                if ui
                    .add_enabled(!at_last_page, egui::Button::new("Next page →"))
                    .clicked()
                {
                    ui_state.plan_page += 1;
                }
            });
            ui.add_space(4.0);
        }
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .id_salt("dat-rename-plan-rows")
            .show(ui, |ui| {
                for row in &visible[start..end] {
                    show_rename_plan_row(ui, row, &mut action);
                    ui.add_space(4.0);
                }
            });
    });

    if !plan.rows.is_empty() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let approved_count = plan
                .rows
                .iter()
                .filter(|row| {
                    row.state == ProposalState::Suggested
                        && row.decision == Some(ReviewDecision::AcceptedForReview)
                })
                .count();
            if approved_count > 0 {
                if widgets::action_button(
                    ui,
                    format!("Apply approved renames ({approved_count})"),
                    widgets::ActionStyle::Destructive,
                    true,
                )
                .on_hover_text("Review and confirm before anything is renamed")
                .clicked()
                {
                    action = Some(DatSourcesPageAction::BeginApplyReview);
                }
                ui.add_space(8.0);
            }
            let actionable_count = plan
                .rows
                .iter()
                .filter(|row| row.state == ProposalState::Suggested)
                .count();
            if widgets::action_button(
                ui,
                format!("Select all verified actionable ({actionable_count})"),
                widgets::ActionStyle::Secondary,
                actionable_count > 0,
            )
            .on_hover_text(
                "Accept every Suggested proposal for review, across every page and filter. \
                 Unmatched, ambiguous, unsupported, conflicting, and blocked rows are never \
                 selected.",
            )
            .clicked()
            {
                action = Some(DatSourcesPageAction::SelectAllActionable);
            }
            ui.add_space(8.0);
            if widgets::action_button(
                ui,
                "Clear selection",
                widgets::ActionStyle::Quiet,
                !plan.rows.is_empty(),
            )
            .clicked()
            {
                action = Some(DatSourcesPageAction::ClearReviewDecisions);
            }
        });
    }

    action
}

fn show_rename_plan_row(
    ui: &mut egui::Ui,
    row: &RenamePlanRowView,
    action: &mut Option<DatSourcesPageAction>,
) {
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                widgets::status_badge(ui, row.state.label(), plan_state_tone(row.state));
                if row.state == ProposalState::Suggested {
                    ui.label(
                        egui::RichText::new(row.current_basename.clone())
                            .monospace()
                            .strong(),
                    );
                } else {
                    ui.label(egui::RichText::new(row.current_basename.clone()).monospace());
                }
            });
            match &row.proposed_basename {
                Some(proposed) => {
                    ui.label(
                        egui::RichText::new(format!("→ {proposed}"))
                            .monospace()
                            .color(theme::muted(ui)),
                    );
                    if row.extension_preserved {
                        ui.label(
                            egui::RichText::new("extension preserved")
                                .color(theme::muted(ui))
                                .small(),
                        );
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("no proposed name")
                            .color(theme::muted(ui))
                            .small(),
                    );
                }
            }
            if let Some(platform) = &row.platform_display {
                ui.label(
                    egui::RichText::new(format!("{platform} · {}", row.source_display_name))
                        .color(theme::muted(ui))
                        .small(),
                );
            } else {
                ui.label(
                    egui::RichText::new(row.source_display_name.clone())
                        .color(theme::muted(ui))
                        .small(),
                );
            }
            if let Some(game) = &row.game_name {
                ui.label(
                    egui::RichText::new(format!(
                        "Matched: {game}{} · {}",
                        row.rom_name
                            .as_deref()
                            .filter(|rom| *rom != game)
                            .map(|rom| format!(" ({rom})"))
                            .unwrap_or_default(),
                        row.verdict_label
                    ))
                    .color(theme::muted(ui))
                    .small(),
                );
            }
            show_content_technical_details(
                ui,
                ("rename_plan_row_technical_details", &row.source_path),
                &row.content,
            );
            if !row.explanations.is_empty() {
                ui.add_space(2.0);
                for line in &row.explanations {
                    ui.label(
                        egui::RichText::new(format!("• {line}"))
                            .color(theme::muted(ui))
                            .small(),
                    );
                }
            }
            if let Some(reason) = &row.ambiguity_reason {
                ui.label(
                    egui::RichText::new(format!("Ambiguous: {reason}"))
                        .color(theme::WARNING)
                        .small(),
                );
            }
            if let Some(detail) = &row.collision_detail {
                ui.label(
                    egui::RichText::new(format!("Conflict: {detail}"))
                        .color(theme::WARNING)
                        .small(),
                );
            }
            for blocker in &row.blockers {
                ui.label(
                    egui::RichText::new(format!("Blocked: {blocker}"))
                        .color(theme::DANGER)
                        .small(),
                );
            }
            for note in &row.sanitisation_notes {
                ui.label(
                    egui::RichText::new(note.clone())
                        .color(theme::muted(ui))
                        .small(),
                );
            }
            if let Some(decision) = row.decision {
                ui.label(
                    egui::RichText::new(format!("Your decision: {}", decision.label()))
                        .color(theme::SUCCESS)
                        .small(),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            if let Some(proposed) = &row.proposed_basename
                && ui
                    .add(egui::Button::new("Copy name").small())
                    .on_hover_text("Copy the proposed filename to the clipboard")
                    .clicked()
            {
                ui.ctx().copy_text(proposed.clone());
            }
            if ui
                .add(egui::Button::new("Needs review").small())
                .on_hover_text("Mark this proposal as needing manual review")
                .clicked()
            {
                *action = Some(DatSourcesPageAction::SetReviewDecision {
                    path: row.source_path.to_string_lossy().into_owned(),
                    decision: Some(ReviewDecision::NeedsManualReview),
                });
            }
            if ui
                .add(egui::Button::new("Ignore").small())
                .on_hover_text("Ignore this proposal for now")
                .clicked()
            {
                *action = Some(DatSourcesPageAction::SetReviewDecision {
                    path: row.source_path.to_string_lossy().into_owned(),
                    decision: Some(ReviewDecision::Ignored),
                });
            }
            if ui
                .add(egui::Button::new("Accept").small())
                .on_hover_text("Keep this proposal for a future review/apply stage")
                .clicked()
            {
                *action = Some(DatSourcesPageAction::SetReviewDecision {
                    path: row.source_path.to_string_lossy().into_owned(),
                    decision: Some(ReviewDecision::AcceptedForReview),
                });
            }
            if row.decision.is_some()
                && ui
                    .add(egui::Button::new("Clear").small())
                    .on_hover_text("Clear your decision; nothing on disk changes")
                    .clicked()
            {
                *action = Some(DatSourcesPageAction::SetReviewDecision {
                    path: row.source_path.to_string_lossy().into_owned(),
                    decision: None,
                });
            }
        });
    });
}

/// Renders unhashed (name-only) audit files grouped by reason, so a run with
/// thousands of symlink refusals shows one exact-count summary instead of
/// thousands of repeated lines. Raw details stay available via "Show all".
fn show_unhashed_groups(ui: &mut egui::Ui, unhashed: &[(String, String)]) {
    const EXAMPLES: usize = 10;
    let mut groups: std::collections::BTreeMap<&str, Vec<&(String, String)>> =
        std::collections::BTreeMap::new();
    for file in unhashed.iter() {
        groups.entry(file.1.as_str()).or_default().push(file);
    }
    // Most common reason first.
    let mut order: Vec<&str> = groups.keys().copied().collect();
    order.sort_by_key(|detail| std::cmp::Reverse(groups[*detail].len()));
    for detail in order {
        let files = &groups[detail];
        let count = files.len();
        let heading = if detail.contains("symlink") {
            format!("{count} symlinks could not be hashed")
        } else {
            format!("{count} files could not be hashed")
        };
        ui.label(egui::RichText::new(heading).strong());
        ui.label(egui::RichText::new(detail).color(theme::muted(ui)).small());
        ui.label(
            egui::RichText::new(format!(
                "Example{}: {}",
                if count == 1 { "" } else { "s" },
                files
                    .iter()
                    .take(EXAMPLES)
                    .map(|file| file.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .color(theme::muted(ui))
            .small(),
        );
        if count > EXAMPLES {
            egui::CollapsingHeader::new(format!("Show all {count}"))
                .id_salt(("dat-audit-unhashed", detail))
                .default_open(false)
                .show(ui, |ui| {
                    for file in files {
                        ui.label(egui::RichText::new(&file.0).monospace().small());
                    }
                });
        }
    }
    ui.label(
        egui::RichText::new(
            "These files were compared by name only, so their matches are not verified by \
             content.",
        )
        .color(theme::muted(ui))
        .small(),
    );
}

/// The compact completion summary: a progress bar, the state badge, the
/// verified/total/percent line, and missing/extra counts up front; revision,
/// provider, and any caveat are one disclosure away rather than always on
/// screen. Never recomputes anything - every number here already lives on
/// `completion`.
fn show_dat_completion(ui: &mut egui::Ui, completion: &DatCompletionView) {
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::status_badge(ui, completion.state.label(), completion.state.tone());
            let headline = match completion.percent {
                Some(percent) => format!(
                    "Verified {} / {} · {}%",
                    format_count(completion.verified as u64),
                    format_count(completion.total as u64),
                    format_percent(percent)
                ),
                // `total == 0`: no denominator to report a count or percent
                // against - see `dat_completion_view`'s `Unknown` branch.
                None => "Not enough information to measure completion".to_string(),
            };
            ui.label(egui::RichText::new(headline).strong());
            if completion.no_intro_complete_badge {
                widgets::status_badge(
                    ui,
                    "Complete against selected No-Intro DAT",
                    widgets::StatusTone::Success,
                );
            }
        });
        if let Some(percent) = completion.percent {
            ui.add(
                egui::ProgressBar::new((percent / 100.0).clamp(0.0, 1.0) as f32).show_percentage(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(missing) = completion.missing {
                ui.label(
                    egui::RichText::new(format!("Missing: {}", format_count(missing as u64)))
                        .color(theme::muted(ui)),
                );
            }
            if completion.extra_local_files > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "Extra (not in this catalogue): {}",
                        format_count(completion.extra_local_files as u64)
                    ))
                    .color(theme::muted(ui)),
                );
            }
        });
        ui.label(
            egui::RichText::new(format!("Against: {}", completion.source_title))
                .color(theme::muted(ui))
                .small(),
        );
        if let Some(caveat) = &completion.caveat {
            ui.add_space(4.0);
            widgets::banner(
                ui,
                "This may understate completion",
                caveat,
                widgets::StatusTone::Warning,
            );
        }
        widgets::technical_details(
            ui,
            ("dat_completion_details", &completion.source_title),
            |ui| {
                if let Some(provider) = &completion.provider {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Provider:").color(theme::muted(ui)));
                        ui.label(provider);
                    });
                }
                if let Some(revision) = &completion.revision {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Revision/snapshot:").color(theme::muted(ui)));
                        widgets::copyable_value(ui, "Revision", revision);
                    });
                }
                if completion.provider.is_none() && completion.revision.is_none() {
                    ui.label(
                        egui::RichText::new(
                            "This DAT's header did not carry a provider or revision string.",
                        )
                        .color(theme::muted(ui))
                        .small(),
                    );
                }
            },
        );
    });
}

fn format_percent(percent: f64) -> String {
    format!("{percent:.2}")
}

/// `12438` -> `"12,438"`. Grouped by thousands for a count that can
/// realistically run into five or six digits (a large No-Intro catalogue),
/// where an ungrouped number is hard to read at a glance.
fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.chars().rev().enumerate() {
        if position > 0 && position % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped.chars().rev().collect()
}

fn show_audit_result(ui: &mut egui::Ui, audit: &AuditResultView) {
    widgets::section_header(ui, "Audit result", Some(&audit.headline));
    widgets::card(ui, |ui| {
        ui.label(
            egui::RichText::new(format!(
                "Source '{}' ({}) · checked {} · {} files read",
                audit.source_display_name,
                audit.source_id,
                audit.scan_root_short,
                audit.files_scanned
            ))
            .color(theme::muted(ui)),
        );
        ui.label(
            egui::RichText::new(format!(
                "Catalogue: {} ({} entries) from {}",
                audit.catalogue_names.join(", "),
                audit.catalogue_entries,
                audit.dat_path
            ))
            .color(theme::muted(ui))
            .small(),
        );
        ui.label(
            egui::RichText::new(format!(
                "Show: {} · full catalogue {} · games {} · compilations {} · required multidisc parts {} · non-game {} · unknown {}",
                audit.content_selection.label(),
                audit.content_summary.total,
                audit.content_summary.games,
                audit.content_summary.game_compilations,
                audit.content_summary.required_multidisc_parts,
                audit.content_summary.non_game,
                audit.content_summary.unknown,
            ))
            .color(theme::muted(ui))
            .small(),
        );
        if let Some(elapsed) = audit.elapsed_seconds {
            ui.label(
                egui::RichText::new(format!("Completed in {}", format_elapsed(elapsed)))
                    .color(theme::muted(ui))
                    .small(),
            );
        }

        if let Some(completion) = &audit.completion {
            ui.add_space(8.0);
            show_dat_completion(ui, completion);
        }
        ui.add_space(6.0);

        for category in &audit.categories {
            ui.horizontal_top(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:>6}", category.count))
                        .monospace()
                        .strong(),
                );
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(category.label).strong());
                    ui.label(
                        egui::RichText::new(category.meaning)
                            .color(theme::muted(ui))
                            .small(),
                    );
                });
            });
        }

        if audit.truncated {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Partial result",
                "The folder held more files than one audit run reads, so this covers part of it. \
                 Audit a smaller folder for a complete answer.",
                widgets::StatusTone::Warning,
            );
        }
        if !audit.unreadable_catalogues.is_empty() {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "Some catalogues were not read",
                &audit.unreadable_catalogues.join("\n"),
                widgets::StatusTone::Warning,
            );
        }
        if !audit.unhashed.is_empty() {
            ui.add_space(6.0);
            widgets::section_header(
                ui,
                "Compared by name only",
                Some(
                    "These files could not be read for hashing, so any match below rests on the \
                     name alone.",
                ),
            );
            show_unhashed_groups(ui, &audit.unhashed);
        }

        if let Some(policy) = &audit.policy {
            ui.add_space(6.0);
            show_audit_policy_notes(ui, policy);
        }
    });

    ui.add_space(8.0);
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Files", None);
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .id_salt("dat-audit-entries")
            .show(ui, |ui| {
                for entry in &audit.entries {
                    ui.horizontal_top(|ui| {
                        ui.label(egui::RichText::new(entry.verdict).monospace().small());
                        ui.vertical(|ui| {
                            ui.label(&entry.file_name);
                            if !entry.detail.is_empty() {
                                ui.label(
                                    egui::RichText::new(&entry.detail)
                                        .color(theme::muted(ui))
                                        .small(),
                                );
                            }
                            if !entry.evidence_sources.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Verified by: {}",
                                        entry.evidence_sources.join(" · ")
                                    ))
                                    .color(theme::muted(ui))
                                    .small(),
                                );
                            }
                            for (content_index, content) in entry.content.iter().enumerate() {
                                show_content_technical_details(
                                    ui,
                                    (
                                        "audit_entry_technical_details",
                                        &entry.file_name,
                                        content_index,
                                    ),
                                    content,
                                );
                            }
                        });
                    });
                }
            });
        if audit.entries_truncated > 0 {
            ui.label(
                egui::RichText::new(format!(
                    "{} more files are counted in the summary above but not listed here.",
                    audit.entries_truncated
                ))
                .color(theme::muted(ui))
                .small(),
            );
        }
    });

    if !audit.archives.is_empty() {
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            widgets::section_header(
                ui,
                "Archive members",
                Some(
                    "Read-only ZIP/7z/LHA evidence. A complete ZIP/7z with one exact game member, or a WHDLoad LHA with one exact slave match, can rename only its outer container; member paths are never changed.",
                ),
            );
            for archive in &audit.archives {
                ui.label(egui::RichText::new(&archive.archive_name).strong());
                ui.label(
                    egui::RichText::new(&archive.completion)
                        .color(theme::muted(ui))
                        .small(),
                );
                for member in &archive.members {
                    ui.horizontal_top(|ui| {
                        ui.label(
                            egui::RichText::new(format!("#{:04}", member.index))
                                .monospace()
                                .small(),
                        );
                        ui.vertical(|ui| {
                            ui.label(&member.name);
                            let identity = member
                                .verdict
                                .as_deref()
                                .map(|verdict| format!(" · DAT: {verdict}"))
                                .unwrap_or_default();
                            ui.label(
                                egui::RichText::new(format!("{}{identity}", member.status))
                                    .color(theme::muted(ui))
                                    .small(),
                            );
                            if !member.detail.is_empty() {
                                ui.label(
                                    egui::RichText::new(&member.detail)
                                        .color(theme::muted(ui))
                                        .small(),
                                );
                            }
                            if !member.evidence_sources.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Verified by: {}",
                                        member.evidence_sources.join(" · ")
                                    ))
                                    .color(theme::muted(ui))
                                    .small(),
                                );
                            }
                        });
                    });
                }
            }
        });
    }
}

/// Root cause of the widespread duplicate-widget-ID warning on DAT
/// Sources: this `CollapsingHeader` used to have no `id_salt` at all, and
/// this function is called once per row from two of the busiest loops on
/// the page - every rename-plan row (`show_rename_plan_row`, one call per
/// file in a plan that can hold hundreds) and every audit-entry content
/// item (the "Files" list under an audit result). With no salt, every one
/// of those calls - across every row, both loops, and however many rows
/// happen to be on screen at once - resolved to the exact same literal-
/// text-derived ID, hashing to the exact same widget ID, which is why one
/// specific number kept recurring "everywhere": it was not one collision,
/// it was every row's identical disclosure colliding with every other
/// row's. `id_salt` is now required from the caller, scoped to that row's
/// own stable identity.
fn show_content_technical_details(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    content: &ContentTechnicalView,
) {
    egui::CollapsingHeader::new("Technical classification details")
        .id_salt(id_salt)
        .default_open(false)
        .show(ui, |ui| {
            ui.label(format!("Classification: {}", content.classification));
            ui.label(format!("Confidence: {}", content.confidence));
            ui.label(format!("Classifier: {}", content.classifier_version));
            for evidence in &content.evidence {
                ui.label(format!("Evidence: {evidence}"));
            }
            if content.original_metadata.is_empty() {
                ui.label("Original metadata: none supplied by this DAT export");
            } else {
                for (field, value) in &content.original_metadata {
                    ui.label(format!("Original {field}: {value}"));
                }
            }
        });
}

fn show_kept_but_not_understood(ui: &mut egui::Ui, view: &DatSourcesPageView) {
    widgets::section_header(
        ui,
        "Kept but not recognised",
        Some(
            "These parts of your registry file name something this build does not know about. \
             They are preserved exactly as written, and saving from this page does not remove \
             them.",
        ),
    );
    widgets::card(ui, |ui| {
        for problem in &view.load_problems {
            ui.horizontal_top(|ui| {
                widgets::status_badge(ui, "Ignored", widgets::StatusTone::Warning);
                ui.add(egui::Label::new(problem).wrap());
            });
        }
        for row in &view.unresolved {
            ui.horizontal_top(|ui| {
                widgets::status_badge(ui, "Kept", widgets::StatusTone::Info);
                ui.add(egui::Label::new(&row.explanation).wrap());
            });
        }
    });
}

#[cfg(test)]
mod tests;
