//! GUI-v2 History & Undo: what EmuWiz did, in plain words.
//!
//! Three things stay strictly apart:
//!
//! - **Current state** belongs in the workflow that owns it. Nothing here says
//!   a file is healthy or broken *now*.
//! - **History** records what happened. A historical success is never proof
//!   that today's files are fine, and a historical failure is never a current
//!   blocker.
//! - **Undo** is a new, explicit operation that uses an existing receipt. This
//!   module adds no undo engine: the executing path is the existing backend
//!   (duplicate-quarantine undo in place; every other family hands off to the
//!   workflow that owns its rollback).
//!
//! The model ([`HistoryEntry`]) is built from the existing journal records
//! without touching the filesystem, so painting is cheap and pure. The only
//! filesystem reads are the read-only, bounded undo-safety check
//! ([`check_undo_safety`]), which runs when the person presses "Preview undo",
//! never during paint, and never mutates anything.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use archivefs_core::dat::rename_apply::model::{
    EntryState, ObjectIdentity, ObjectKind, RenameTransaction, TransactionEntry,
    TransactionOperation, TransactionState,
};
use eframe::egui::{self, RichText};

use super::backend::DuplicateRepairRecord;
use super::library::Library;
use super::routes::{Route, Section};

/// Entries examined per transaction for target/game evidence and for the
/// preview's safety check. Larger receipts say how many were not examined.
pub(super) const MAX_EVIDENCE_ENTRIES: usize = 200;
/// Rows painted before "Show more". Presentation only.
pub(super) const PAGE_SIZE: usize = 50;
/// Per-entry rows shown under Advanced.
const MAX_ADVANCED_ENTRIES: usize = 20;

/// Where a receipt came from. This, plus the recorded operation kinds, is the
/// only basis for grouping; nothing is classified from message text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Source {
    PbpExport,
    DuplicateQuarantine,
    PlayingLibrary,
    CanonicalOrganisation,
    MameReconstruction,
    MameInternalRepair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Family {
    VerificationRename,
    Repair,
    Organisation,
    Mame,
}

impl Family {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::VerificationRename => "Verification / Rename",
            Self::Repair => "Repair",
            Self::Organisation => "Organisation",
            Self::Mame => "MAME",
        }
    }
}

/// How an operation ended, from the recorded transaction state only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Completed,
    /// The operation stopped and changed nothing.
    Failed,
    /// The operation stopped after changing some, but not all, items.
    StoppedPartway {
        applied: usize,
        total: usize,
    },
    /// Undone: the changes were reversed.
    Undone,
    /// Interrupted, or an undo that did not finish; recovery state is recorded.
    NeedsAttention,
}

impl Outcome {
    pub(super) fn label(self) -> String {
        match self {
            Self::Completed => "Completed".into(),
            Self::Failed => "Failed".into(),
            Self::StoppedPartway { applied, total } => {
                format!("Stopped partway ({applied} of {total} done)")
            }
            Self::Undone => "Undone".into(),
            Self::NeedsAttention => "Needs attention".into(),
        }
    }
}

/// Whether an entry can be undone, and why not. Derived from the receipt only;
/// whether it is still *safe now* is answered by the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum UndoStatus {
    Available,
    AlreadyUndone,
    Unavailable(String),
    NeedsReview(String),
}

impl UndoStatus {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Available => "Undo available",
            Self::AlreadyUndone => "Already undone",
            Self::Unavailable(_) => "Undo unavailable",
            Self::NeedsReview(_) => "Undo needs review",
        }
    }
}

/// A catalogue game that exactly equals a path recorded in the receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GameRef {
    pub id: i64,
    pub title: String,
    pub platform: String,
}

/// Where a person should go to work with this operation now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Handoff {
    pub label: &'static str,
    pub route: Route,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FailureFacts {
    pub attempted: String,
    pub changed: String,
    pub cleanup: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct HistoryEntry {
    pub transaction_id: String,
    pub source: Source,
    pub family: Family,
    pub summary: String,
    pub outcome: Outcome,
    /// Unix seconds, only when the receipt records one.
    pub when: Option<u64>,
    pub undone_when: Option<u64>,
    /// Neutral target context (file/set/folder), never a guessed game.
    pub target: Option<String>,
    /// Games whose catalogue path exactly equals a recorded receipt path.
    pub games: Vec<GameRef>,
    pub undo: UndoStatus,
    pub handoff: Handoff,
    pub failure: Option<FailureFacts>,
    pub advanced: Vec<(String, String)>,
    pub transaction: RenameTransaction,
    pub journal_dir: Option<PathBuf>,
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

fn nonzero(unix: u64) -> Option<u64> {
    (unix > 0).then_some(unix)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn applied_count(transaction: &RenameTransaction) -> usize {
    transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .count()
}

fn is_link_operation(operation: &TransactionOperation) -> bool {
    matches!(
        operation,
        TransactionOperation::CreateSymlink { .. }
            | TransactionOperation::CreateHardlink { .. }
            | TransactionOperation::CreateCopy { .. }
    )
}

fn outcome_of(transaction: &RenameTransaction, source: Source) -> Outcome {
    match transaction.state {
        TransactionState::Applied | TransactionState::Finalised => Outcome::Completed,
        TransactionState::RolledBack
            if source == Source::DuplicateQuarantine && !transaction.is_fully_rolled_back() =>
        {
            Outcome::NeedsAttention
        }
        TransactionState::RolledBack => Outcome::Undone,
        TransactionState::ApplyFailed => {
            let applied = applied_count(transaction);
            if applied == 0 {
                Outcome::Failed
            } else {
                Outcome::StoppedPartway {
                    applied,
                    total: transaction.entries.len(),
                }
            }
        }
        TransactionState::Planned
        | TransactionState::Applying
        | TransactionState::RollingBack
        | TransactionState::Finalising
        | TransactionState::RollbackFailed => Outcome::NeedsAttention,
    }
}

fn undo_status_of(transaction: &RenameTransaction, source: Source) -> UndoStatus {
    match transaction.state {
        TransactionState::Applied if source == Source::PbpExport => UndoStatus::Unavailable(
            "The derived PBP and receipt are journaled; this Blue journey does not offer Undo. Review recovery records for interrupted publication.".into(),
        ),
        TransactionState::RolledBack
            if source == Source::DuplicateQuarantine && !transaction.is_fully_rolled_back() =>
        {
            UndoStatus::NeedsReview(
                "The Undo receipt contradicts its entry states; manual review is required.".into(),
            )
        }
        TransactionState::RolledBack => UndoStatus::AlreadyUndone,
        TransactionState::Finalised => UndoStatus::Unavailable(
            "This replacement was finalised: its original was deleted on your confirmation, so it can no longer be undone."
                .into(),
        ),
        TransactionState::Finalising => UndoStatus::NeedsReview(
            "Finalising this replacement was interrupted. Undo is unavailable; finish or cancel it from the MAME rebuild page."
                .into(),
        ),
        TransactionState::Applied if applied_count(transaction) > 0 => {
            // Quarantine undo runs in this page; the others run in the
            // workflow that owns their rollback.
            let _ = source;
            UndoStatus::Available
        }
        TransactionState::Applied => UndoStatus::Unavailable(
            "The receipt records no applied changes, so there is nothing to undo.".into(),
        ),
        TransactionState::ApplyFailed
            if applied_count(transaction) > 0
                && source == Source::PlayingLibrary
                && transaction
                    .entries
                    .iter()
                    .filter(|entry| entry.state == EntryState::Applied)
                    .all(|entry| entry.created_link_identity().is_some()) =>
        {
            UndoStatus::Available
        }
        TransactionState::ApplyFailed if applied_count(transaction) == 0 => {
            UndoStatus::Unavailable("Nothing was changed, so there is nothing to undo.".into())
        }
        TransactionState::RollbackFailed | TransactionState::RollingBack
            if source == Source::DuplicateQuarantine
                && transaction
                    .entries
                    .iter()
                    .any(super::duplicate_undo::needs_restore) =>
        {
            UndoStatus::Available
        }
        TransactionState::RollbackFailed => UndoStatus::NeedsReview(
            "An earlier undo attempt did not finish. Its recovery state is recorded.".into(),
        ),
        TransactionState::Planned
        | TransactionState::Applying
        | TransactionState::ApplyFailed
        | TransactionState::RollingBack => UndoStatus::NeedsReview(
            "This operation did not finish normally. Review its recovery state before undoing."
                .into(),
        ),
    }
}

fn summary_of(transaction: &RenameTransaction, source: Source) -> String {
    let count = transaction.entries.len();
    match source {
        Source::PbpExport => "PS1 PBP playing-copy export".into(),
        Source::DuplicateQuarantine => format!(
            "Moved {} to quarantine",
            plural(count, "redundant file", "redundant files")
        ),
        Source::PlayingLibrary => format!(
            "Built a Playing Library ({})",
            plural(count, "link", "links")
        ),
        Source::MameInternalRepair => {
            format!("MAME internal repair ({} member(s))", count)
        }
        Source::MameReconstruction => {
            match transaction
                .unknown
                .get("mame_parent")
                .and_then(serde_json::Value::as_str)
            {
                Some(parent) => format!("Rebuilt MAME set {parent}"),
                None => "Rebuilt a MAME set".into(),
            }
        }
        Source::CanonicalOrganisation => {
            let links = transaction
                .entries
                .iter()
                .any(|entry| is_link_operation(&entry.operation));
            if links {
                format!(
                    "Organised {} as links or copies",
                    plural(count, "file", "files")
                )
            } else if transaction
                .entries
                .iter()
                .all(|entry| entry.source_path.parent() == entry.destination_path.parent())
            {
                format!("Renamed {}", plural(count, "file", "files"))
            } else {
                format!("Moved {}", plural(count, "file", "files"))
            }
        }
    }
}

fn family_of(transaction: &RenameTransaction, source: Source) -> Family {
    match source {
        Source::PbpExport => Family::Repair,
        Source::DuplicateQuarantine => Family::Repair,
        Source::PlayingLibrary => Family::Organisation,
        Source::MameReconstruction | Source::MameInternalRepair => Family::Mame,
        Source::CanonicalOrganisation => {
            if transaction
                .entries
                .iter()
                .any(|entry| is_link_operation(&entry.operation))
            {
                Family::Organisation
            } else {
                Family::VerificationRename
            }
        }
    }
}

fn target_of(transaction: &RenameTransaction, source: Source) -> Option<String> {
    let first = transaction.entries.first()?;
    Some(match source {
        Source::PlayingLibrary => transaction.source_scan_root.clone(),
        Source::DuplicateQuarantine => transaction.source_scan_root.clone(),
        _ => {
            let name = file_name(&first.destination_path);
            match transaction.entries.len() {
                1 => name,
                more => format!("{name} and {} more", more - 1),
            }
        }
    })
}

/// The route that owns this family's current workflow.
fn handoff_for(source: Source, family: Family) -> Handoff {
    match (source, family) {
        (Source::DuplicateQuarantine, _) => Handoff {
            label: "Open Duplicates",
            route: Route::Section(Section::Duplicates),
        },
        (Source::MameReconstruction | Source::MameInternalRepair, _) => Handoff {
            label: "Open MAME workflow",
            route: Route::MameWorkflow,
        },
        (_, Family::VerificationRename) => Handoff {
            label: "Open DATs & Verification",
            route: Route::Section(Section::DatVerification),
        },
        _ => Handoff {
            label: "Open Organisation",
            route: Route::Section(Section::Build),
        },
    }
}

fn failure_facts(transaction: &RenameTransaction, source: Source) -> Option<FailureFacts> {
    let outcome = outcome_of(transaction, source);
    if !matches!(
        outcome,
        Outcome::Failed | Outcome::StoppedPartway { .. } | Outcome::NeedsAttention
    ) {
        return None;
    }
    let total = transaction.entries.len();
    let applied = applied_count(transaction);
    let rolled_back = transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::RolledBack)
        .count();
    let attempted = format!("It tried to change {}.", plural(total, "item", "items"));
    let changed = match outcome {
        Outcome::Failed if rolled_back > 0 => {
            format!("{rolled_back} of {total} were changed and then reversed; no change remains.")
        }
        Outcome::Failed => "No file was changed; your originals are as they were.".to_string(),
        _ if applied == 0 && rolled_back == 0 => {
            "The receipt records no completed change.".to_string()
        }
        _ => format!(
            "{} of {} changed before it stopped.",
            applied + rolled_back,
            total
        ),
    };
    let cleanup = if rolled_back > 0 {
        format!(
            "{} reversed. Whether anything else was cleaned up is not recorded.",
            plural(rolled_back, "change", "changes")
        )
    } else {
        "Cleanup is not recorded for this operation.".into()
    };
    let reason = transaction
        .entries
        .iter()
        .find_map(|entry| entry.failure_reason.clone())
        .or_else(|| {
            transaction
                .entries
                .iter()
                .flat_map(|entry| entry.preflight_failures.iter().cloned())
                .next()
        });
    Some(FailureFacts {
        attempted,
        changed,
        cleanup,
        reason,
    })
}

fn advanced_of(
    transaction: &RenameTransaction,
    journal_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Transaction ID".into(), transaction.transaction_id.clone()),
        ("Recorded state".into(), transaction.state.label().into()),
        (
            "Created (unix)".into(),
            transaction.created_at_unix.to_string(),
        ),
        (
            "Plan generation".into(),
            transaction.plan_generation.to_string(),
        ),
        (
            "Working folder".into(),
            transaction.source_scan_root.clone(),
        ),
    ];
    if let Some(dir) = journal_dir {
        rows.push(("Journal folder".into(), dir.display().to_string()));
    }
    if let Some(version) = &transaction.classifier_version {
        rows.push(("Classifier version".into(), version.clone()));
    }
    for (key, value) in &transaction.unknown {
        rows.push((format!("Provenance: {key}"), value.to_string()));
    }
    if let Some(resolution) = &transaction.recovery_resolution {
        rows.push(("Recovery resolution".into(), format!("{resolution:?}")));
    }
    for (index, entry) in transaction
        .entries
        .iter()
        .take(MAX_ADVANCED_ENTRIES)
        .enumerate()
    {
        rows.push((
            format!("Item {} from", index + 1),
            entry.source_path.display().to_string(),
        ));
        rows.push((
            format!("Item {} to", index + 1),
            entry.destination_path.display().to_string(),
        ));
        rows.push((
            format!("Item {} state", index + 1),
            entry.state.label().into(),
        ));
        if let Some(freshness) = &entry.identity.freshness {
            let hash: String = freshness
                .sha256
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            rows.push((format!("Item {} SHA-256", index + 1), hash));
        }
        if let Some(reason) = &entry.failure_reason {
            rows.push((format!("Item {} error", index + 1), reason.clone()));
        }
        for failure in &entry.preflight_failures {
            rows.push((format!("Item {} preflight", index + 1), failure.clone()));
        }
        if let Some(at) = entry.applied_at_unix {
            rows.push((format!("Item {} applied (unix)", index + 1), at.to_string()));
        }
        if let Some(at) = entry.rolled_back_at_unix {
            rows.push((format!("Item {} undone (unix)", index + 1), at.to_string()));
        }
    }
    if transaction.entries.len() > MAX_ADVANCED_ENTRIES {
        rows.push((
            "More items".into(),
            format!(
                "{} further item(s) are in the journal",
                transaction.entries.len() - MAX_ADVANCED_ENTRIES
            ),
        ));
    }
    rows
}

impl HistoryEntry {
    /// A record shown read-only: its original time, workflow, outcome and
    /// summary under an unmistakable status, with ordinary Undo disabled.
    /// Built from what was read; nothing is read or written here.
    pub(super) fn from_evidence(
        item: &archivefs_core::dat::rename_apply::seal::JournalEvidence,
    ) -> Self {
        let placeholder;
        let (transaction, source) = match &item.transaction {
            Some(transaction) => (transaction, source_of(transaction)),
            None => {
                placeholder = RenameTransaction {
                    transaction_id: item.file_id.clone(),
                    plan_generation: 0,
                    classifier_version: None,
                    created_at_unix: 0,
                    source_scan_root: String::new(),
                    state: TransactionState::Planned,
                    entries: Vec::new(),
                    created_directories: Vec::new(),
                    recovery_resolution: None,
                    recovery_resolved_at_unix: None,
                    unknown: Default::default(),
                };
                (&placeholder, Source::CanonicalOrganisation)
            }
        };
        let mut entry = Self::from_transaction(transaction, source, None);
        let recorded = if item.transaction.is_some() {
            entry.summary.clone()
        } else {
            format!("File {}", file_name(&item.file))
        };
        entry.summary = format!("{} · {recorded}", item.status.title());
        // A key of its own: never merged with, or selected as, another record
        // that shares its transaction id.
        entry.transaction_id = format!("evidence:{}", item.file.display());
        entry.undo = UndoStatus::Unavailable(item.status.explanation());
        if item.transaction.is_none() {
            entry.outcome = Outcome::NeedsAttention;
            entry.target = Some(item.file.display().to_string());
        }
        entry.journal_dir = None;
        entry
            .advanced
            .insert(0, ("Record status".into(), item.status.title().into()));
        entry
            .advanced
            .insert(1, ("Record file".into(), item.file.display().to_string()));
        entry.advanced.insert(
            2,
            (
                "Record folder".into(),
                if item.legacy_namespace {
                    "Legacy (written before sealed Undo records)".into()
                } else {
                    "Sealed records folder".into()
                },
            ),
        );
        entry
    }

    pub(super) fn from_transaction(
        transaction: &RenameTransaction,
        source: Source,
        journal_dir: Option<&Path>,
    ) -> Self {
        let family = family_of(transaction, source);
        let when = transaction
            .entries
            .iter()
            .filter_map(|entry| entry.applied_at_unix)
            .max()
            .and_then(nonzero)
            .or_else(|| nonzero(transaction.created_at_unix));
        let undone_when =
            if source == Source::DuplicateQuarantine && !transaction.is_fully_rolled_back() {
                None
            } else {
                transaction
                    .entries
                    .iter()
                    .filter_map(|entry| entry.rolled_back_at_unix)
                    .max()
                    .and_then(nonzero)
            };
        let coordinated = journal_dir
            .map(|dir| {
                archivefs_core::launch::es_de_publish::operation::is_coordinated(dir, transaction)
            })
            .unwrap_or_else(|| {
                Ok(transaction
                    .unknown
                    .keys()
                    .any(|k| k.starts_with("retrodeck_")))
            });
        let owned = matches!(coordinated, Ok(true))
            || transaction
                .unknown
                .keys()
                .any(|key| key.starts_with("retrodeck_"));
        let ownership_unavailable = coordinated.is_err();
        let combined_phase = if owned {
            journal_dir.and_then(|dir| {
                archivefs_core::launch::es_de_publish::operation::inspect(
                    dir,
                    &transaction.transaction_id,
                )
                .ok()
                .filter(|outcome| outcome.transaction == *transaction)
                .map(|outcome| outcome.phase)
            })
        } else {
            None
        };
        let undo = match coordinated {
            Ok(false)=>undo_status_of(transaction,source),
            Ok(true)=>UndoStatus::NeedsReview("Use Build Playing Library › RetroDECK History and Recovery for coordinated metadata and link rollback. Child Undo is blocked.".into()),
            Err(error)=>UndoStatus::NeedsReview(format!("Receipt ownership cannot be checked: {error}. Preserve evidence; independent Undo is blocked.")),
        };
        Self {
            transaction_id: transaction.transaction_id.clone(),
            source,
            family,
            summary: if owned {
                "RetroDECK projection: links and game list".into()
            } else {
                summary_of(transaction, source)
            },
            outcome: if owned {
                match combined_phase {
                    Some(archivefs_core::launch::es_de_publish::operation::Phase::Completed) => {
                        Outcome::Completed
                    }
                    Some(archivefs_core::launch::es_de_publish::operation::Phase::Restored) => {
                        Outcome::Undone
                    }
                    _ => Outcome::NeedsAttention,
                }
            } else if ownership_unavailable {
                Outcome::NeedsAttention
            } else {
                outcome_of(transaction, source)
            },
            when,
            undone_when,
            target: target_of(transaction, source),
            games: Vec::new(),
            undo,
            handoff: handoff_for(source, family),
            failure: failure_facts(transaction, source),
            advanced: advanced_of(transaction, journal_dir),
            transaction: transaction.clone(),
            journal_dir: journal_dir.map(Path::to_path_buf),
        }
    }

    /// Paths this receipt records, bounded. Used only for exact catalogue
    /// matching.
    fn recorded_paths(&self) -> impl Iterator<Item = &Path> {
        self.transaction
            .entries
            .iter()
            .take(MAX_EVIDENCE_ENTRIES)
            .flat_map(|entry| {
                [
                    entry.source_path.as_path(),
                    entry.destination_path.as_path(),
                ]
            })
    }
}

/// Every receipt the page knows, as borrowed from the existing stores.
pub(super) struct HistorySources<'a> {
    pub quarantine: &'a [DuplicateRepairRecord],
    pub playing_library: &'a [RenameTransaction],
    pub organisation: &'a [RenameTransaction],
    pub mame: &'a [RenameTransaction],
    /// Legacy, unsealed, changed or unreadable records: shown, never undone.
    pub evidence: &'a [archivefs_core::dat::rename_apply::seal::JournalEvidence],
}

/// Which workflow a sealed receipt belongs to, from what it recorded.
pub(super) fn source_of(transaction: &RenameTransaction) -> Source {
    use archivefs_core::dat::rename_apply::model::TransactionOperation;
    if transaction.source_scan_root == "verified-ps1-pbp-export-v1" {
        Source::PbpExport
    } else if transaction.entries.iter().any(|entry| {
        entry.destination_path.components().any(|component| {
            component.as_os_str() == archivefs_core::repair::QUARANTINE_DIRECTORY_NAME
        })
    }) {
        Source::DuplicateQuarantine
    } else if transaction
        .unknown
        .contains_key("mame_internal_repair_plan_digest")
    {
        Source::MameInternalRepair
    } else if transaction
        .unknown
        .get("workflow")
        .and_then(serde_json::Value::as_str)
        == Some(archivefs_core::dat::mame_merged_reconstruction::MAME_RECONSTRUCTION_WORKFLOW)
    {
        Source::MameReconstruction
    } else if transaction
        .entries
        .iter()
        .any(|entry| matches!(entry.operation, TransactionOperation::CreateSymlink { .. }))
    {
        Source::PlayingLibrary
    } else {
        Source::CanonicalOrganisation
    }
}

/// Builds the entries newest first, then attaches exact game associations.
/// Reads no filesystem. A game is associated only when its catalogue path is
/// byte-for-byte equal to a path in the receipt; resemblance is never used.
pub(super) fn build_entries(
    sources: &HistorySources<'_>,
    library: Option<&Library>,
) -> Vec<HistoryEntry> {
    let mut entries: Vec<HistoryEntry> = Vec::new();
    for record in sources.quarantine {
        entries.push(HistoryEntry::from_transaction(
            &record.transaction,
            Source::DuplicateQuarantine,
            Some(&record.journal_dir),
        ));
    }
    for transaction in sources.playing_library {
        entries.push(HistoryEntry::from_transaction(
            transaction,
            Source::PlayingLibrary,
            None,
        ));
    }
    for transaction in sources.organisation {
        entries.push(HistoryEntry::from_transaction(
            transaction,
            source_of(transaction),
            None,
        ));
    }
    for transaction in sources.mame {
        entries.push(HistoryEntry::from_transaction(
            transaction,
            Source::MameReconstruction,
            None,
        ));
    }
    for item in sources.evidence {
        entries.push(HistoryEntry::from_evidence(item));
    }
    // Newest first; an entry with no recorded time sorts last, never first.
    entries.sort_by(|a, b| b.when.cmp(&a.when));
    if let Some(library) = library
        && !entries.is_empty()
    {
        attach_games(&mut entries, library);
    }
    entries
}

fn attach_games(entries: &mut [HistoryEntry], library: &Library) {
    let wanted: HashSet<PathBuf> = entries
        .iter()
        .flat_map(|entry| entry.recorded_paths().map(Path::to_path_buf))
        .collect();
    if wanted.is_empty() {
        return;
    }
    let mut by_path: HashMap<&Path, GameRef> = HashMap::new();
    for game in &library.games {
        if wanted.contains(game.archive.absolute_path.as_path()) {
            by_path.insert(
                game.archive.absolute_path.as_path(),
                GameRef {
                    id: game.archive.id,
                    title: game.title.clone(),
                    platform: game.platform.clone(),
                },
            );
        }
    }
    for entry in entries {
        let mut seen = BTreeMap::new();
        for path in entry.recorded_paths() {
            if let Some(game) = by_path.get(path) {
                seen.entry(game.id).or_insert_with(|| game.clone());
            }
        }
        entry.games = seen.into_values().collect();
    }
}

// ----------------------------------------------------------------- filters

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum StatusFilter {
    #[default]
    All,
    UndoAvailable,
    Completed,
    Failed,
}

impl StatusFilter {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::UndoAvailable => "Undo available",
            Self::Completed => "Completed",
            Self::Failed => "Failed or needs attention",
        }
    }
}

/// Keeps the immutable catalogue snapshot alive without dumping the library.
#[derive(Clone)]
struct LibrarySnapshot(std::sync::Arc<Library>);

impl std::fmt::Debug for LibrarySnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("LibrarySnapshot")
            .field(&std::sync::Arc::as_ptr(&self.0))
            .finish()
    }
}

/// Presentation state only. Nothing here is written to any journal.
#[derive(Clone, Debug, Default)]
pub(super) struct HistoryViewState {
    pub status: StatusFilter,
    pub family: Option<Family>,
    pub search: String,
    pub shown: usize,
    /// Cached model and the cheap fingerprint it was built from.
    pub entries: Vec<HistoryEntry>,
    pub built_for: Option<u64>,
    /// Retain the snapshot so allocation reuse cannot disguise a library reload.
    built_library: Option<LibrarySnapshot>,
    /// Safety previews the person asked for, by transaction id.
    pub previews: BTreeMap<String, UndoPreview>,
    pub confirm_undo: Option<String>,
    /// Session-only links to the existing Activity results, by receipt id.
    pub undo_attempts: BTreeMap<String, u64>,
}

/// Why the row list is empty, so each cause gets its own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EmptyCause {
    NoHistory,
    NoUndoable,
    NoFailed,
    NoMatches,
    NoGameHistory,
}

/// The entries to show, in order, and why the list is empty if it is.
pub(super) fn filter_entries<'a>(
    entries: &'a [HistoryEntry],
    state: &HistoryViewState,
    game: Option<i64>,
) -> Result<Vec<&'a HistoryEntry>, EmptyCause> {
    if entries.is_empty() {
        return Err(EmptyCause::NoHistory);
    }
    let scoped: Vec<&HistoryEntry> = entries
        .iter()
        .filter(|entry| game.is_none_or(|id| entry.games.iter().any(|g| g.id == id)))
        .collect();
    if scoped.is_empty() {
        return Err(EmptyCause::NoGameHistory);
    }
    let needle = state.search.trim().to_lowercase();
    let narrowed: Vec<&HistoryEntry> = scoped
        .into_iter()
        .filter(|entry| match state.status {
            StatusFilter::All => true,
            StatusFilter::UndoAvailable => entry.undo == UndoStatus::Available,
            StatusFilter::Completed => entry.outcome == Outcome::Completed,
            StatusFilter::Failed => matches!(
                entry.outcome,
                Outcome::Failed | Outcome::StoppedPartway { .. } | Outcome::NeedsAttention
            ),
        })
        .filter(|entry| state.family.is_none_or(|family| entry.family == family))
        .filter(|entry| {
            needle.is_empty()
                || entry.summary.to_lowercase().contains(&needle)
                || entry
                    .target
                    .as_deref()
                    .is_some_and(|t| t.to_lowercase().contains(&needle))
                || entry
                    .games
                    .iter()
                    .any(|g| g.title.to_lowercase().contains(&needle))
                || entry.family.label().to_lowercase().contains(&needle)
        })
        .collect();
    if narrowed.is_empty() {
        return Err(match (state.status, state.family, needle.is_empty()) {
            (StatusFilter::UndoAvailable, None, true) => EmptyCause::NoUndoable,
            (StatusFilter::Failed, None, true) => EmptyCause::NoFailed,
            _ => EmptyCause::NoMatches,
        });
    }
    Ok(narrowed)
}

pub(super) fn empty_copy(cause: EmptyCause) -> (&'static str, &'static str) {
    match cause {
        EmptyCause::NoHistory => (
            "No history yet",
            "When EmuWiz renames, repairs, rebuilds or organises something, the record appears here.",
        ),
        EmptyCause::NoUndoable => (
            "No undoable operations",
            "Nothing recorded can currently be undone. Undone or unfinished-but-empty operations are not offered.",
        ),
        EmptyCause::NoFailed => (
            "No failed operations",
            "Every recorded operation completed or was undone.",
        ),
        EmptyCause::NoMatches => (
            "No entries match these filters",
            "Clear the search or the filters to see the rest of your history.",
        ),
        EmptyCause::NoGameHistory => (
            "No history for this game",
            "No receipt records a path that exactly matches this game. Other history may still exist; show all history to see it.",
        ),
    }
}

// --------------------------------------------------------- undo preview

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Blocker {
    OutputChanged(String),
    OutputMissing(String),
    OriginalPathOccupied(String),
    WorkflowOwned(String),
}

impl Blocker {
    pub(super) fn describe(&self) -> String {
        match self {
            Self::WorkflowOwned(reason) => reason.clone(),
            Self::OutputChanged(name) => {
                format!("The output has changed since the operation: {name}.")
            }
            Self::OutputMissing(name) => {
                format!("The destination no longer exists: {name}.")
            }
            Self::OriginalPathOccupied(name) => {
                format!("Something now exists where the original would be restored: {name}.")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UndoPreview {
    pub operation: String,
    pub target: Option<String>,
    pub will_change: Vec<String>,
    pub receipt: String,
    pub checked: usize,
    pub not_checked: usize,
    pub blockers: Vec<Blocker>,
    pub warnings: Vec<String>,
}

impl UndoPreview {
    pub(super) fn safe(&self) -> bool {
        self.blockers.is_empty()
    }

    /// The plain reason Undo is unavailable, if the preview found one.
    pub(super) fn unavailable_because(&self) -> Option<String> {
        let first = self.blockers.first()?;
        Some(
            match first {
                Blocker::WorkflowOwned(_) => "Undo requires the workflow that owns this operation.",
                Blocker::OutputChanged(_) => "Undo is unavailable because the output has changed.",
                Blocker::OutputMissing(_) => {
                    "Undo is unavailable because the destination no longer exists."
                }
                Blocker::OriginalPathOccupied(_) => {
                    "Undo is unavailable because something now exists at the original location."
                }
            }
            .into(),
        )
    }
}

fn identity_still_matches(expected: &ObjectIdentity, path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let kind = if metadata.file_type().is_symlink() {
        ObjectKind::Symlink
    } else if metadata.file_type().is_file() {
        ObjectKind::RegularFile
    } else {
        ObjectKind::Other
    };
    if kind != expected.kind || metadata.len() != expected.size_bytes {
        return false;
    }
    use std::os::unix::fs::MetadataExt;
    metadata.mtime() == expected.modified_unix
        && metadata.ino() == expected.ino
        && metadata.dev() == expected.dev
}

fn check_entry(entry: &TransactionEntry, blockers: &mut Vec<Blocker>) {
    let name = file_name(&entry.destination_path);
    match &entry.operation {
        TransactionOperation::RenameMove | TransactionOperation::ReplaceExisting { .. } => {
            if std::fs::symlink_metadata(&entry.destination_path).is_err() {
                blockers.push(Blocker::OutputMissing(name));
                return;
            }
            if !identity_still_matches(&entry.identity, &entry.destination_path) {
                blockers.push(Blocker::OutputChanged(name));
                return;
            }
            if matches!(entry.operation, TransactionOperation::RenameMove)
                && std::fs::symlink_metadata(&entry.source_path).is_ok()
            {
                blockers.push(Blocker::OriginalPathOccupied(file_name(&entry.source_path)));
            }
        }
        TransactionOperation::CreateSymlink {
            expected_target, ..
        } => match std::fs::read_link(&entry.destination_path) {
            Err(_) => blockers.push(Blocker::OutputMissing(name)),
            Ok(target) if &target != expected_target => {
                blockers.push(Blocker::OutputChanged(name));
            }
            Ok(_) => {
                let owned = entry.created_link_identity();
                let current = archivefs_core::dat::rename_apply::identity::capture_identity(
                    &entry.destination_path,
                )
                .ok();
                if !matches!((owned, current), (Some(expected), Some(actual))
                    if archivefs_core::dat::rename_apply::identity::identity_matches(&expected, &actual))
                {
                    blockers.push(Blocker::OutputChanged(name));
                }
            }
        },
        TransactionOperation::CreateCopy { .. } => {
            match archivefs_core::dat::rename_apply::capture_identity(&entry.destination_path) {
                Ok(actual)
                    if archivefs_core::dat::rename_apply::copy::destination_matches(
                        entry, &actual,
                    ) => {}
                Ok(_) => blockers.push(Blocker::OutputChanged(name)),
                Err(_) => blockers.push(Blocker::OutputMissing(name)),
            }
        }
        TransactionOperation::CreateHardlink { .. } => {
            if std::fs::symlink_metadata(&entry.destination_path).is_err() {
                blockers.push(Blocker::OutputMissing(name));
            }
        }
    }
}

/// A read-only, bounded look at whether undo still looks safe. It compares
/// recorded sizes, times and identities with what is on disk right now and
/// changes nothing. The backend repeats the full verification when undo runs,
/// and its refusal reason is shown if it declines.
pub(super) fn check_undo_safety(entry: &HistoryEntry) -> UndoPreview {
    let transaction = &entry.transaction;
    let mut blockers = Vec::new();
    let known_owned = transaction
        .unknown
        .keys()
        .any(|key| key.starts_with("retrodeck_"));
    let ownership = entry
        .journal_dir
        .as_deref()
        .map(|dir| {
            archivefs_core::launch::es_de_publish::operation::is_coordinated(dir, transaction)
        })
        .unwrap_or(Ok(known_owned));
    match ownership {
        Ok(true) => blockers.push(Blocker::WorkflowOwned(
            "Use RetroDECK History and Recovery for coordinated rollback.".into(),
        )),
        Err(error) => blockers.push(Blocker::WorkflowOwned(format!(
            "Receipt ownership is unavailable; preserve evidence and review before Undo: {error}"
        ))),
        Ok(false) => {}
    }
    let mut checked = 0;
    for item in transaction
        .entries
        .iter()
        .filter(|item| {
            if entry.source == Source::DuplicateQuarantine {
                super::duplicate_undo::needs_restore(item)
            } else {
                item.state == EntryState::Applied
            }
        })
        .take(MAX_EVIDENCE_ENTRIES)
    {
        check_entry(item, &mut blockers);
        checked += 1;
    }
    let applied = if entry.source == Source::DuplicateQuarantine {
        transaction
            .entries
            .iter()
            .filter(|item| super::duplicate_undo::needs_restore(item))
            .count()
    } else {
        applied_count(transaction)
    };
    let not_checked = applied.saturating_sub(checked);
    let mut warnings = Vec::new();
    if not_checked > 0 {
        warnings.push(format!(
            "{} not checked here; they are fully re-verified when you undo.",
            plural(not_checked, "item was", "items were")
        ));
    }
    if !matches!(entry.undo, UndoStatus::Available) {
        warnings.push(format!("Receipt status: {}.", entry.undo.label()));
    }
    let will_change = match entry.source {
        Source::PbpExport => vec!["The original discs are retained. This journey does not offer Undo of the derived PBP.".into()],
        Source::DuplicateQuarantine => vec![format!(
            "Restore {} from quarantine to {}.",
            plural(applied, "file", "files"),
            "their original locations"
        )],
        Source::PlayingLibrary => vec![format!(
            "Remove {} the Playing Library created. Your original games are not touched.",
            plural(applied, "link", "links")
        )],
        Source::MameInternalRepair => vec!["Move the created MAME copies into their private recovery staging slots. Original source files are retained.".into()],
        Source::MameReconstruction => vec![
            if super::mame_reconstruction::is_replacement(transaction) {
                "Put the original archive back. The rebuilt set is kept in EmuWiz's recovery folder; your source archives are not touched."
            } else {
                "Remove the reconstructed MAME set. Your source archives are not touched."
            }
            .into(),
        ],
        Source::CanonicalOrganisation => vec![format!(
            "Put {} back at the original name or location.",
            plural(applied, "file", "files")
        )],
    };
    UndoPreview {
        operation: entry.summary.clone(),
        target: entry.target.clone(),
        will_change,
        receipt: format!("Journal receipt {}", entry.transaction_id),
        checked,
        not_checked,
        blockers,
        warnings,
    }
}

// ------------------------------------------------------------- formatting

/// `2026-10-07 14:03 UTC` from Unix seconds. UTC is stated because the
/// receipt records no time zone.
pub(super) fn format_time(unix: u64) -> String {
    let secs = i64::try_from(unix).unwrap_or(i64::MAX);
    match time::OffsetDateTime::from_unix_timestamp(secs) {
        Ok(moment) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02} UTC",
            moment.year(),
            u8::from(moment.month()),
            moment.day(),
            moment.hour(),
            moment.minute()
        ),
        Err(_) => "an unrecorded time".into(),
    }
}

/// A stable fingerprint of what the page would show, with no filesystem access.
pub(super) fn fingerprint(sources: &HistorySources<'_>, library_games: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    library_games.hash(&mut hasher);
    (
        sources.quarantine.len(),
        sources.playing_library.len(),
        sources.organisation.len(),
        sources.mame.len(),
        sources.evidence.len(),
    )
        .hash(&mut hasher);
    for item in sources.evidence {
        item.file.hash(&mut hasher);
        item.status.title().hash(&mut hasher);
    }
    let mut add = |transaction: &RenameTransaction| {
        transaction.transaction_id.hash(&mut hasher);
        std::mem::discriminant(&transaction.state).hash(&mut hasher);
        transaction.entries.len().hash(&mut hasher);
    };
    for record in sources.quarantine {
        add(&record.transaction);
    }
    for transaction in sources
        .playing_library
        .iter()
        .chain(sources.organisation)
        .chain(sources.mame)
    {
        add(transaction);
    }
    hasher.finish()
}

// ------------------------------------------------------------------ page

impl super::App {
    pub(super) fn history(&mut self, ui: &mut egui::Ui) {
        self.organisation
            .r2
            .show(ui, self.canonical_organisation.r2_selection(), true);
        self.refresh_history_model();
        let game_path = self
            .router
            .current
            .game()
            .and_then(|id| self.library.game(id))
            .map(|g| g.archive.absolute_path.clone());
        if self
            .discovery_import_history
            .show(ui, None, false, game_path.as_deref())
        {
            self.load(false);
            if let Some(w) = &mut self.native_workflows {
                w.refresh_after_discovery_undo(ui.ctx().clone());
            }
        }
        let has_conversion_records =
            super::wiiu_disc::show_conversion_history(ui, game_path.as_deref());
        let scoped_game = match &self.router.current {
            Route::Task {
                section: Section::History,
                game,
            } => Some(*game),
            _ => None,
        };
        let trainer_scope_available = scoped_game.is_none() || game_path.is_some();
        let has_trainer_history = trainer_scope_available
            && self
                .native_workflows
                .get_or_insert_with(|| {
                    super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
                })
                .prepare_scummvm_trainer_history(ui.ctx(), game_path.as_deref());
        let has_bezel_history = self
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(ui.ctx().clone()))
            .prepare_bezel_history(ui.ctx(), scoped_game.is_none());
        let has_cheat_history = self
            .native_workflows
            .as_ref()
            .is_some_and(super::native_workflows::NativeWorkflows::has_cheat_history);
        let has_undo_activity = self
            .native_workflows
            .as_ref()
            .is_some_and(super::native_workflows::NativeWorkflows::has_undo_activity);
        // Resolve completed loads before deciding whether history is empty.
        // Starting a load also makes its pending outcome distinct from empty.
        self.duckstation_history.poll();
        self.duckstation_history.ensure_fresh(ui.ctx(), false);

        ui.label("This is a record of what EmuWiz did earlier. It is history, not the current state of your files. Browsing it changes nothing.");

        let mut go: Option<Route> = None;
        let mut view = std::mem::take(&mut self.history_view);

        if let Some(game_id) = scoped_game {
            ui.horizontal_wrapped(|ui| {
                let title = self
                    .library
                    .game(game_id)
                    .map_or_else(|| "this game".to_string(), |game| game.title.clone());
                ui.strong(format!("Showing only history recorded for {title}"));
                if ui.button("Show all history").clicked() {
                    go = Some(Route::Section(Section::History));
                }
            });
        }

        if !view.entries.is_empty() {
            show_filters(ui, &mut view);
        }

        let result = filter_entries(&view.entries, &view, scoped_game);
        match result {
            Err(EmptyCause::NoGameHistory) if has_trainer_history || has_bezel_history => {}
            Err(EmptyCause::NoHistory)
                if has_cheat_history
                    || has_undo_activity
                    || has_bezel_history
                    || has_trainer_history
                    || self.duckstation_history.is_loading()
                    || !self.duckstation_history.operations().is_empty()
                    || self.discovery_import_history.has_records()
                    || self.organisation.r2.has_state()
                    || has_conversion_records => {}
            Err(cause) => {
                let (title, body) = empty_copy(cause);
                super::imagery::empty_state(
                    ui,
                    &mut self.imagery,
                    super::imagery::EmptyArt::Mascot,
                    title,
                    body,
                    None,
                );
                if cause == EmptyCause::NoMatches && ui.button("Clear filters").clicked() {
                    view.status = StatusFilter::All;
                    view.family = None;
                    view.search.clear();
                }
            }
            Ok(rows) => {
                let limit = view.shown.max(PAGE_SIZE);
                let total = rows.len();
                let mut preview_request: Option<String> = None;
                let mut undo_request: Option<String> = None;
                let mut shown = view.shown;
                // The conversion, import and trainer sections share this page, so
                // the rows must keep enough height to stay readable.
                egui::ScrollArea::vertical()
                    .id_salt("v2_history_rows")
                    .min_scrolled_height(ROWS_MIN_HEIGHT)
                    .show(ui, |ui| {
                        for entry in rows.iter().take(limit) {
                            show_row(
                                ui,
                                entry,
                                &view,
                                &self.activity,
                                self.playing_library_job.is_some(),
                                HistoryRowRequests {
                                    go: &mut go,
                                    preview: &mut preview_request,
                                    undo: &mut undo_request,
                                },
                            );
                        }
                        if total > limit {
                            ui.label(format!("Showing {limit} of {total}."));
                            if ui.button("Show more").clicked() {
                                shown = limit + PAGE_SIZE;
                            }
                        }
                    });
                view.shown = shown;
                if let Some(id) = preview_request
                    && let Some(entry) = view.entries.iter().find(|e| e.transaction_id == id)
                {
                    // The one place the filesystem is read: on the click,
                    // read-only, bounded - never during paint.
                    let preview = check_undo_safety(entry);
                    view.previews.insert(id.clone(), preview);
                    view.confirm_undo = None;
                }
                if let Some(id) = undo_request {
                    view.confirm_undo = Some(id);
                }
            }
        }

        // DuckStation cheat operations: one row each, deep-linked to the
        // Cheats workflow that owns the single Undo.
        let mut refresh = false;
        if let Some(route) = super::history_duckstation::show(
            ui,
            self.duckstation_history.operations(),
            &self.library,
            self.duckstation_history.is_loading(),
            &mut refresh,
        ) {
            go = Some(self.open_duckstation_undo(route));
        }
        if refresh {
            self.duckstation_history.ensure_fresh(ui.ctx(), true);
        }

        if trainer_scope_available && let Some(workflows) = self.native_workflows.as_mut() {
            workflows.show_scummvm_trainer_history(ui, game_path.as_deref());
        }
        if scoped_game.is_none() {
            let open_cheats = self
                .native_workflows
                .as_ref()
                .is_some_and(|workflows| workflows.show_cheat_history(ui));
            if open_cheats {
                go = Some(Route::Section(Section::Mods));
            }
            // Gold records carry no game, so like cheat activity this is global.
            if let Some(workflows) = self.native_workflows.as_ref() {
                workflows.show_undo_activity(ui);
            }
        }

        if let Some(workflows) = self.native_workflows.as_mut() {
            workflows.show_bezel_history(ui, scoped_game.is_none());
        }

        // Keep recovery navigation after the existing receipts so opening
        // History preserves the space available for their outcomes and Undo.
        if crate::ui::components::action_button(
            ui,
            "Review recovery records",
            crate::ui::components::ActionStyle::Secondary,
            true,
        )
        .on_hover_text("Open the journal recovery view to review interrupted changes and unreadable-record warnings. Opening the page does not approve Undo.")
        .clicked()
        {
            go = Some(self.open_recovery_history());
        }

        if let Some(id) = view.confirm_undo.clone() {
            let repair_index = self
                .repair_history
                .iter()
                .position(|record| record.transaction.transaction_id == id);
            let library_index = self.playing_library_history.iter().position(|transaction| {
                transaction.transaction_id == id
                    && transaction
                        .unknown
                        .get("playing_library_profile")
                        .and_then(serde_json::Value::as_str)
                        == Some("generic")
            });
            if repair_index.is_some() || library_index.is_some() {
                let mut confirmed = false;
                let mut cancelled = false;
                egui::Window::new("Confirm undo")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(if library_index.is_some() {
                        "EmuWiz will re-verify each recorded link object before moving it to a hidden Undo tombstone. Replaced or changed links will be refused; original ROMs remain untouched."
                    } else {
                        "EmuWiz will re-verify the quarantined files and restore them to their original locations. If anything changed unexpectedly it will refuse and tell you why."
                    });
                    if ui.add_enabled(
                        self.playing_library_job.is_none(),
                        egui::Button::new(if library_index.is_some() { "Undo these links" } else { "Undo this repair" }),
                    ).clicked() {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
                if confirmed {
                    if self.playing_library_job.is_some() {
                        view.confirm_undo = None;
                        self.history_view = view;
                        return;
                    }
                    view.confirm_undo = None;
                    self.undo_confirm = None;
                    self.history_view = view;
                    if repair_index.is_some() {
                        self.undo_history_entry(&id);
                    } else if let Some(index) = library_index
                        && let Some(transaction) = self.playing_library_history.get(index).cloned()
                    {
                        self.playing_library
                            .load_generic_history_receipt(transaction);
                        self.start_playing_library_job(super::PlayingLibraryJobKind::Rollback);
                    }
                    return;
                }
                if cancelled {
                    view.confirm_undo = None;
                }
            }
        }
        self.history_view = view;
        if let Some(route) = go {
            self.go(route);
        }
    }

    /// Recovery stays with the existing native journal controller and its
    /// separate confirmation. History grants no permission to mutate files.
    fn open_recovery_history(&mut self) -> Route {
        self.organisation.view = super::organisation::OrganisationView::RepairHistory;
        Route::Section(Section::Build)
    }

    /// The deep link from a DuckStation History row to the Cheats workflow that
    /// owns its single Undo: lands on that game's Cheats tab.
    pub(super) fn open_duckstation_undo(&mut self, route: Route) -> Route {
        self.mods.select_cheats();
        route
    }

    /// Rebuilds the cached model only when the receipts or library changed.
    /// Pure: no filesystem access.
    pub(super) fn refresh_history_model(&mut self) {
        let sources = HistorySources {
            quarantine: &self.repair_history,
            playing_library: &self.playing_library_history,
            organisation: &self.canonical_organisation_history,
            mame: &self.organisation.mame_reconstruction.history,
            evidence: &self.journal_evidence,
        };
        let key = fingerprint(&sources, self.library.games.len());
        if self.history_view.built_for == Some(key)
            && self
                .history_view
                .built_library
                .as_ref()
                .is_some_and(|snapshot| std::sync::Arc::ptr_eq(&snapshot.0, &self.library))
        {
            return;
        }
        let entries = build_entries(&sources, Some(&self.library));
        let previous = std::mem::replace(&mut self.history_view.entries, entries);
        self.history_view.built_library = Some(LibrarySnapshot(self.library.clone()));
        self.history_view.built_for = Some(key);
        let previous: HashMap<_, _> = previous
            .iter()
            .map(|entry| {
                (
                    (entry.source, entry.transaction_id.as_str()),
                    &entry.transaction,
                )
            })
            .collect();
        let valid: HashSet<&str> = self
            .history_view
            .entries
            .iter()
            .filter(|entry| matches!(entry.undo, UndoStatus::Available))
            .filter(|entry| {
                previous
                    .get(&(entry.source, entry.transaction_id.as_str()))
                    .is_some_and(|old| *old == &entry.transaction)
            })
            .map(|entry| entry.transaction_id.as_str())
            .collect();
        self.history_view
            .previews
            .retain(|id, _| valid.contains(id.as_str()));
        if self
            .history_view
            .confirm_undo
            .as_ref()
            .is_some_and(|id| !valid.contains(id.as_str()))
        {
            self.history_view.confirm_undo = None;
        }
    }
}

fn show_filters(ui: &mut egui::Ui, view: &mut HistoryViewState) {
    ui.horizontal_wrapped(|ui| {
        for status in [
            StatusFilter::All,
            StatusFilter::UndoAvailable,
            StatusFilter::Completed,
            StatusFilter::Failed,
        ] {
            if ui
                .selectable_label(view.status == status, status.label())
                .clicked()
            {
                view.status = status;
            }
        }
    });
    let present: Vec<Family> = {
        let mut families: Vec<Family> = view.entries.iter().map(|entry| entry.family).collect();
        families.sort();
        families.dedup();
        families
    };
    if present.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(view.family.is_none(), "Every kind")
                .clicked()
            {
                view.family = None;
            }
            for family in present {
                if ui
                    .selectable_label(view.family == Some(family), family.label())
                    .clicked()
                {
                    view.family = Some(family);
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label("Search:");
        ui.add(
            egui::TextEdit::singleline(&mut view.search)
                .hint_text("operation, file or game")
                .id_salt("v2_history_search"),
        );
    });
}

const ROWS_MIN_HEIGHT: f32 = 360.0;

struct HistoryRowRequests<'a> {
    go: &'a mut Option<Route>,
    preview: &'a mut Option<String>,
    undo: &'a mut Option<String>,
}

fn show_row(
    ui: &mut egui::Ui,
    entry: &HistoryEntry,
    view: &HistoryViewState,
    activity: &super::activity::Activity,
    playing_library_busy: bool,
    requests: HistoryRowRequests<'_>,
) {
    ui.push_id(("history-row", &entry.transaction_id), |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(&entry.summary).strong().size(16.0));
            let when = entry
                .when
                .map(format_time)
                .unwrap_or_else(|| "Time not recorded".into());
            ui.label(format!("{} · {}", entry.outcome.label(), when));
            if let Some(game) = entry.games.first() {
                let extra = entry.games.len().saturating_sub(1);
                ui.label(if extra == 0 {
                    format!("Game: {} · {}", game.title, game.platform)
                } else {
                    format!("Game: {} and {extra} more", game.title)
                });
            }
            if let Some(target) = &entry.target {
                ui.label(format!("Where: {target}"));
            }
            if let Some(at) = entry.undone_when {
                ui.label(format!("Undone {}", format_time(at)));
            }

            // The receipt describes the original operation. A refused Undo can
            // leave it unchanged, so show the attempt separately using Activity's
            // actual outcome and reason rather than inferring success from it.
            if entry.source == Source::DuplicateQuarantine
                && let Some(job) = view
                    .undo_attempts
                    .get(&entry.transaction_id)
                    .and_then(|id| activity.jobs.get(id))
            {
                ui.strong(format!("Latest Undo attempt · {}", job.status_label()));
                ui.label(&job.summary);
                if !job.technical.is_empty() {
                    ui.label(&job.technical);
                }
            }

            // Undo status: current backend truth, with the reason when not.
            let preview = view.previews.get(&entry.transaction_id);
            match (&entry.undo, preview) {
                (UndoStatus::Available, Some(preview)) if !preview.safe() => {
                    ui.strong("Undo unavailable");
                    ui.label(
                        preview
                            .unavailable_because()
                            .unwrap_or_else(|| "The safety check found a problem.".into()),
                    );
                }
                (status @ UndoStatus::Available, _) => {
                    ui.strong(status.label());
                }
                (status @ UndoStatus::AlreadyUndone, _) => {
                    ui.label(status.label());
                }
                (
                    status @ (UndoStatus::Unavailable(reason) | UndoStatus::NeedsReview(reason)),
                    _,
                ) => {
                    ui.strong(status.label());
                    ui.label(reason);
                }
            }

            if let Some(failure) = &entry.failure {
                ui.label(&failure.attempted);
                ui.label(&failure.changed);
                ui.label(&failure.cleanup);
                if let Some(reason) = &failure.reason {
                    ui.label(format!("Reason recorded: {reason}"));
                }
            }

            // One primary action per row.
            ui.horizontal_wrapped(|ui| match (&entry.undo, preview) {
                (UndoStatus::Available, None) => {
                    if ui.button(RichText::new("Preview undo").strong()).clicked() {
                        *requests.preview = Some(entry.transaction_id.clone());
                    }
                }
                // The preview panel below carries the next action.
                (UndoStatus::Available, Some(_)) | (UndoStatus::AlreadyUndone, _) => {}
                _ if entry.source == Source::PbpExport => {
                    ui.label("Use Review recovery records below to inspect the journal.");
                }
                _ => {
                    if ui
                        .button(RichText::new(entry.handoff.label).strong())
                        .clicked()
                    {
                        *requests.go = Some(entry.handoff.route.clone());
                    }
                }
            });

            if let Some(preview) = preview {
                let undo_busy = playing_library_busy
                    || view
                        .undo_attempts
                        .values()
                        .any(|id| activity.jobs.get(id).is_some_and(|job| job.active()));
                show_preview(ui, entry, preview, undo_busy, requests.go, requests.undo);
            }

            ui.collapsing("Advanced details", |ui| {
                egui::Grid::new(("history-advanced", &entry.transaction_id))
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (label, value) in &entry.advanced {
                            ui.label(label);
                            ui.monospace(value);
                            ui.end_row();
                        }
                    });
            });
        });
    });
}

fn show_preview(
    ui: &mut egui::Ui,
    entry: &HistoryEntry,
    preview: &UndoPreview,
    undo_busy: bool,
    go: &mut Option<Route>,
    undo_request: &mut Option<String>,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong("Undo preview - nothing has been changed");
        ui.label(format!("Original operation: {}", preview.operation));
        if let Some(target) = &preview.target {
            ui.label(format!("Affected: {target}"));
        }
        for line in &preview.will_change {
            if entry.source == Source::PbpExport {
                ui.label(format!("Recovery note: {line}"));
            } else {
                ui.label(format!("Undo would: {line}"));
            }
        }
        ui.label(format!("Uses: {}", preview.receipt));
        if preview.safe() && entry.source == Source::PbpExport {
            ui.label(format!("Recorded evidence: {} looked unchanged. This page does not offer PBP Undo.", plural(preview.checked, "item", "items")));
        } else if preview.safe() {
            ui.label(format!(
                "Safety check: {} looked unchanged. EmuWiz re-verifies everything again when you undo.",
                plural(preview.checked, "item", "items")
            ));
        } else {
            for blocker in &preview.blockers {
                ui.label(format!("Blocked: {}", blocker.describe()));
            }
        }
        if entry.source == Source::PbpExport {
            ui.label("This read-only check covers recorded metadata and presence. Review the journal and retained recovery stage before any separate recovery action.");
        } else {
            ui.label("This preview checks recorded metadata, link targets or presence only. The owning workflow repeats its full verification before undo changes any files.");
        }
        for warning in &preview.warnings {
            ui.label(format!("Note: {warning}"));
        }
        if preview.safe() && matches!(entry.undo, UndoStatus::Available) {
            match entry.source {
                Source::DuplicateQuarantine | Source::PlayingLibrary
                    if entry.source == Source::DuplicateQuarantine
                        || entry.transaction.unknown.get("playing_library_profile").and_then(serde_json::Value::as_str) == Some("generic") => {
                    if crate::ui::components::action_button(
                        ui,
                        RichText::new("Undo").strong(),
                        crate::ui::components::ActionStyle::Undo,
                        !undo_busy,
                    )
                    .on_disabled_hover_text("Another Undo is still running.")
                    .clicked()
                    {
                        *undo_request = Some(entry.transaction_id.clone());
                    }
                }
                _ => {
                    ui.label("Undo for this kind of operation is run from the workflow that owns it.");
                    if ui.button(RichText::new(entry.handoff.label).strong()).clicked() {
                        *go = Some(entry.handoff.route.clone());
                    }
                }
            }
        } else if ui.button(entry.handoff.label).clicked() {
            *go = Some(entry.handoff.route.clone());
        }
    });
}

#[cfg(test)]
pub(super) mod tests;
