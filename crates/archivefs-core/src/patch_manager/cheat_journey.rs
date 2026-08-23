//! One explicit, read-first RetroArch cheat journey built from the shared
//! catalogue, preview, transaction, history, and rollback primitives.
//!
//! This module intentionally does not discover profiles or contact providers.
//! Its caller supplies the already selected authoritative game, local
//! catalogue snapshot, and selected profile destination. That keeps identity,
//! candidate choice, preview, apply, and undo separate and makes every write
//! contingent on an exact previously approved preview.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::cheat_candidates::{
    CheatCandidate, CheatCandidateArchive, CheatCandidateOptions, build_cheat_candidates,
};
use super::cheat_catalogue::{CheatCatalogueFormat, CheatCatalogueSnapshot};
use super::cheat_install_plan::{
    CheatDestinationRequest, CheatInstallPlanError, CheatInstallPreviewRequest, CheatSelection,
    ResolvedCheatDestination, build_cheat_install_preview, load_candidate_document,
    match_strength_for_candidate, resolve_cheat_destination, stage_generated_cheat_file,
};
use super::destination_safety::DestinationState;
use super::shared_preview::PreviewDestinationState;
use super::shared_transaction::{
    SharedApplyOptions, SharedApplyResult, SharedRollbackConfirmation, SharedRollbackOptions,
    SharedRollbackPreview, SharedRollbackResult, build_shared_transaction_plan,
    execute_shared_apply, execute_shared_rollback, preview_shared_rollback,
};

/// Identity state before candidate discovery. Only [`Verified`](Self::Verified)
/// is allowed to progress to selection or installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheatJourneyIdentityState {
    Verified,
    Unknown,
    Conflicting,
}

/// The independent proof used to authoritatively identify the selected game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheatJourneyIdentityEvidenceKind {
    CanonicalLibraryRecord,
    ContentHash,
    ProductCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyIdentityEvidence {
    pub kind: CheatJourneyIdentityEvidenceKind,
    pub value: String,
}

/// One caller-selected game. `identity_key` is recorded in transaction
/// history; `evidence` is retained for callers to display alongside matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyGameIdentity {
    pub state: CheatJourneyIdentityState,
    pub selected_archive: PathBuf,
    pub identity_key: String,
    pub archive: CheatCandidateArchive,
    pub evidence: Vec<CheatJourneyIdentityEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyCandidate {
    pub candidate: CheatCandidate,
    pub provider: String,
    pub format: CheatCatalogueFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyCandidateList {
    pub candidates: Vec<CheatJourneyCandidate>,
    pub total_matched: usize,
    pub truncated: bool,
    pub records_scanned: usize,
    pub scan_limit_reached: bool,
}

/// Read-only result of matching one authoritative game to a local catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyDiscovery {
    pub game: CheatJourneyGameIdentity,
    pub provider: String,
    pub candidates: CheatJourneyCandidateList,
    /// Entries excluded by catalogue parsing. They never appear as selectable
    /// candidates, while valid neighbours remain available.
    pub excluded_candidate_count: usize,
}

/// A candidate selected by its exact catalogue-relative path, with its picker
/// deliberately initialized with no codes selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneySelection {
    pub game: CheatJourneyGameIdentity,
    pub candidate: CheatJourneyCandidate,
    pub candidate_digest: String,
    pub cheat_selection: CheatSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheatJourneyDestinationFingerprint {
    Missing,
    Existing { sha256: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheatJourneyPreviewAction {
    InstallNew,
    AlreadyInstalled,
    ReplaceExisting,
}

/// A fully deterministic, write-free approval artifact. It contains the
/// rendered bytes to show the caller, but does not create a staging file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyPreview {
    pub preview_id: String,
    pub game: CheatJourneyGameIdentity,
    pub candidate: CheatJourneyCandidate,
    pub candidate_digest: String,
    pub selection: CheatSelection,
    pub destination_request: CheatDestinationRequest,
    pub destination: ResolvedCheatDestination,
    pub destination_fingerprint: CheatJourneyDestinationFingerprint,
    pub action: CheatJourneyPreviewAction,
    pub rendered_contents: String,
    pub rendered_digest: String,
    pub profile_id: String,
    pub source_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyApplyApproval {
    pub preview_id: String,
    pub approved: bool,
    pub replacement_approved: bool,
}

#[derive(Debug, Clone)]
pub struct CheatJourneyApplyOptions {
    /// Private EmuWiz-owned staging root. This is first written during apply,
    /// never while preparing a preview.
    pub staging_root: PathBuf,
    pub operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

#[derive(Debug)]
pub struct CheatJourneyApplyResult {
    pub preview_id: String,
    pub transaction_id: String,
    pub result: SharedApplyResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyUndoPreview {
    pub transaction_id: String,
    pub preview: SharedRollbackPreview,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyUndoConfirmation {
    pub preview_id: String,
    pub approved: bool,
}

#[derive(Debug, Clone)]
pub struct CheatJourneyUndoOptions {
    pub confirmation: CheatJourneyUndoConfirmation,
    pub rollback_operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheatJourneyErrorKind {
    IdentityUnknown,
    IdentityConflicting,
    IdentityInvalid,
    CatalogueNotReadOnly,
    CandidateNotFound,
    CandidateNotSelectable,
    CandidateChanged,
    SelectionInvalid,
    DestinationUnavailable,
    PreviewChanged,
    ApprovalRequired,
    ApprovalMismatch,
    ApplyFailed,
    UndoApprovalMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatJourneyError {
    pub kind: CheatJourneyErrorKind,
    pub detail: String,
}

impl std::fmt::Display for CheatJourneyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for CheatJourneyError {}

/// Match candidates without touching the filesystem or a provider. Unknown
/// and conflicting identities fail before the catalogue is consulted.
pub fn discover_cheat_journey(
    game: &CheatJourneyGameIdentity,
    snapshot: &CheatCatalogueSnapshot,
    options: &CheatCandidateOptions,
) -> Result<CheatJourneyDiscovery, CheatJourneyError> {
    validate_game(game)?;
    if !snapshot.read_only {
        return Err(error(
            CheatJourneyErrorKind::CatalogueNotReadOnly,
            "a cheat journey requires a read-only catalogue snapshot",
        ));
    }
    let matches = build_cheat_candidates(snapshot, &game.archive, options);
    let candidates = matches
        .candidates
        .into_iter()
        .map(|candidate| CheatJourneyCandidate {
            format: format_for_candidate(snapshot, &candidate),
            provider: snapshot.source_name.clone(),
            candidate,
        })
        .collect();
    Ok(CheatJourneyDiscovery {
        game: game.clone(),
        provider: snapshot.source_name.clone(),
        candidates: CheatJourneyCandidateList {
            candidates,
            total_matched: matches.total_matched,
            truncated: matches.truncated,
            records_scanned: matches.records_scanned,
            scan_limit_reached: matches.scan_limit_reached,
        },
        excluded_candidate_count: snapshot.excluded_entries.len(),
    })
}

/// Opens exactly the candidate the caller named and returns a picker with no
/// code selected. Ambiguous candidates are accepted only through this explicit
/// path; no candidate is automatically selected by this module.
pub fn select_cheat_journey_candidate(
    discovery: &CheatJourneyDiscovery,
    catalogue_root: &Path,
    catalogue_relative_path: &str,
) -> Result<CheatJourneySelection, CheatJourneyError> {
    validate_game(&discovery.game)?;
    let candidate = discovery
        .candidates
        .candidates
        .iter()
        .find(|value| value.candidate.catalogue_relative_path == catalogue_relative_path)
        .cloned()
        .ok_or_else(|| {
            error(
                CheatJourneyErrorKind::CandidateNotFound,
                "candidate is not in this discovery",
            )
        })?;
    if !candidate.candidate.manually_selectable {
        return Err(error(
            CheatJourneyErrorKind::CandidateNotSelectable,
            "the selected candidate is blocked by compatibility or parsing checks",
        ));
    }
    if candidate.format != CheatCatalogueFormat::RetroarchChtDirectory {
        return Err(error(
            CheatJourneyErrorKind::CandidateNotSelectable,
            "this catalogue format is discoverable but has no RetroArch CHT materialization path",
        ));
    }
    let loaded = load_candidate_document(
        catalogue_root,
        &candidate.candidate.catalogue_relative_path,
        candidate.candidate.source_file_hash.as_deref(),
    )
    .map_err(candidate_error)?;
    Ok(CheatJourneySelection {
        game: discovery.game.clone(),
        candidate,
        candidate_digest: loaded.digest,
        cheat_selection: CheatSelection::from_document(&loaded.document),
    })
}

/// Create a deterministic preview without staging or writing any file.
pub fn preview_cheat_journey(
    selection: &CheatJourneySelection,
    catalogue_root: &Path,
    destination_request: CheatDestinationRequest,
    profile_id: &str,
    source_mode: &str,
) -> Result<CheatJourneyPreview, CheatJourneyError> {
    validate_game(&selection.game)?;
    let (loaded, rendered) = reload_and_render(selection, catalogue_root)?;
    let destination = resolve_cheat_destination(&destination_request).map_err(destination_error)?;
    let fingerprint = destination_fingerprint(&destination).map_err(destination_error)?;
    let rendered_digest = digest(rendered.as_bytes());
    let action = match &fingerprint {
        CheatJourneyDestinationFingerprint::Missing => CheatJourneyPreviewAction::InstallNew,
        CheatJourneyDestinationFingerprint::Existing { sha256 } if sha256 == &rendered_digest => {
            CheatJourneyPreviewAction::AlreadyInstalled
        }
        CheatJourneyDestinationFingerprint::Existing { .. } => {
            CheatJourneyPreviewAction::ReplaceExisting
        }
    };
    let mut preview = CheatJourneyPreview {
        preview_id: String::new(),
        game: selection.game.clone(),
        candidate: selection.candidate.clone(),
        candidate_digest: loaded.digest,
        selection: selection.cheat_selection.clone(),
        destination_request,
        destination,
        destination_fingerprint: fingerprint,
        action,
        rendered_contents: rendered,
        rendered_digest,
        profile_id: profile_id.to_owned(),
        source_mode: source_mode.to_owned(),
    };
    preview.preview_id = preview_digest(&preview);
    Ok(preview)
}

/// Materialize and apply exactly one approved preview. Existing transaction
/// machinery rechecks sources and destinations under its lock, writes a
/// journal, and performs byte-exact rollback support.
pub fn apply_cheat_journey(
    preview: &CheatJourneyPreview,
    catalogue_root: &Path,
    approval: &CheatJourneyApplyApproval,
    options: &CheatJourneyApplyOptions,
) -> Result<CheatJourneyApplyResult, CheatJourneyError> {
    if !approval.approved {
        return Err(error(
            CheatJourneyErrorKind::ApprovalRequired,
            "explicit preview approval is required",
        ));
    }
    if approval.preview_id != preview.preview_id || preview.preview_id != preview_digest(preview) {
        return Err(error(
            CheatJourneyErrorKind::ApprovalMismatch,
            "approval does not match the exact preview",
        ));
    }
    validate_game(&preview.game)?;
    let selection = CheatJourneySelection {
        game: preview.game.clone(),
        candidate: preview.candidate.clone(),
        candidate_digest: preview.candidate_digest.clone(),
        cheat_selection: preview.selection.clone(),
    };
    let (loaded, rendered) = reload_and_render(&selection, catalogue_root)?;
    if loaded.digest != preview.candidate_digest
        || rendered != preview.rendered_contents
        || digest(rendered.as_bytes()) != preview.rendered_digest
    {
        return Err(error(
            CheatJourneyErrorKind::PreviewChanged,
            "candidate or selected cheat contents changed; create a new preview",
        ));
    }
    let destination =
        resolve_cheat_destination(&preview.destination_request).map_err(destination_error)?;
    let fingerprint = destination_fingerprint(&destination).map_err(destination_error)?;
    if destination.path != preview.destination.path
        || fingerprint != preview.destination_fingerprint
    {
        return Err(error(
            CheatJourneyErrorKind::PreviewChanged,
            "destination changed since preview; create a new preview",
        ));
    }
    let file_stem = destination.file_name.strip_suffix(".cht").ok_or_else(|| {
        error(
            CheatJourneyErrorKind::PreviewChanged,
            "preview destination has an invalid file name",
        )
    })?;
    let entries = preview
        .selection
        .resolve(&loaded.document)
        .map_err(selection_error)?;
    let staged = stage_generated_cheat_file(&options.staging_root, file_stem, &entries, &[])
        .map_err(selection_error)?;
    if staged.contents != preview.rendered_contents || staged.digest != preview.rendered_digest {
        return Err(error(
            CheatJourneyErrorKind::PreviewChanged,
            "staged output differs from the approved preview",
        ));
    }
    let install_preview = build_cheat_install_preview(&CheatInstallPreviewRequest {
        selected_archive: preview.game.selected_archive.clone(),
        platform: preview.game.archive.platform.clone(),
        verified_identity: preview.game.identity_key.clone(),
        destination,
        profile_cheat_root: preview.destination_request.profile_cheat_root.clone(),
        staged: staged.clone(),
        match_strength: match_strength_for_candidate(&preview.candidate.candidate)
            .map_err(selection_error)?,
    })
    .map_err(selection_error)?;
    let entry = install_preview.report.entries.first().ok_or_else(|| {
        error(
            CheatJourneyErrorKind::PreviewChanged,
            "apply preview contains no destination entry",
        )
    })?;
    if !shared_destination_matches_preview(
        entry.destination_state,
        entry.existing_destination_digest.as_deref(),
        preview,
    ) {
        return Err(error(
            CheatJourneyErrorKind::PreviewChanged,
            "destination changed while preparing apply; create a new preview",
        ));
    }
    let plan = build_shared_transaction_plan(
        &install_preview.report,
        &preview.profile_id,
        &preview.source_mode,
        &staged.staging_root,
    )
    .map_err(|failure| error(CheatJourneyErrorKind::ApplyFailed, failure.detail))?;
    let result = execute_shared_apply(
        &plan,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(super::shared_transaction::SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: true,
                replacement_approved: approval.replacement_approved,
            }),
            operation_id: options.operation_id.clone(),
            timestamp_unix_seconds: options.timestamp_unix_seconds,
            current_context: plan.context.clone(),
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
    );
    Ok(CheatJourneyApplyResult {
        preview_id: preview.preview_id.clone(),
        transaction_id: result.journal.operation_id.clone(),
        result,
    })
}

/// Read-only exact-undo inspection for one transaction journal.
pub fn preview_cheat_journey_undo(
    transaction_id: &str,
    journal_path: &Path,
    destination_root: &Path,
    backup_root: &Path,
) -> CheatJourneyUndoPreview {
    CheatJourneyUndoPreview {
        transaction_id: transaction_id.to_owned(),
        preview: preview_shared_rollback(journal_path, destination_root, backup_root),
    }
}

/// Execute a separately approved undo. Repeating it is safe: the shared
/// rollback records `AlreadyRolledBack` and makes no destination change.
pub fn undo_cheat_journey(
    preview: &CheatJourneyUndoPreview,
    options: &CheatJourneyUndoOptions,
) -> Result<SharedRollbackResult, CheatJourneyError> {
    if !options.confirmation.approved
        || options.confirmation.preview_id != preview.preview.preview_id
    {
        return Err(error(
            CheatJourneyErrorKind::UndoApprovalMismatch,
            "undo approval does not match the exact undo preview",
        ));
    }
    Ok(execute_shared_rollback(
        &preview.preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: options.rollback_operation_id.clone(),
            timestamp_unix_seconds: options.timestamp_unix_seconds,
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
    ))
}

fn validate_game(game: &CheatJourneyGameIdentity) -> Result<(), CheatJourneyError> {
    match game.state {
        CheatJourneyIdentityState::Unknown => {
            return Err(error(
                CheatJourneyErrorKind::IdentityUnknown,
                "unknown game identity cannot authorize cheat installation",
            ));
        }
        CheatJourneyIdentityState::Conflicting => {
            return Err(error(
                CheatJourneyErrorKind::IdentityConflicting,
                "conflicting game identity cannot authorize cheat installation",
            ));
        }
        CheatJourneyIdentityState::Verified => {}
    }
    if game.selected_archive.as_os_str().is_empty()
        || game.identity_key.trim().is_empty()
        || game.archive.display_name.trim().is_empty()
        || game.evidence.is_empty()
        || game
            .evidence
            .iter()
            .any(|evidence| evidence.value.trim().is_empty())
    {
        return Err(error(
            CheatJourneyErrorKind::IdentityInvalid,
            "a verified game needs an archive, identity key, display name, and non-empty identity evidence",
        ));
    }
    Ok(())
}

fn reload_and_render(
    selection: &CheatJourneySelection,
    catalogue_root: &Path,
) -> Result<(super::cheat_install_plan::LoadedCandidate, String), CheatJourneyError> {
    let loaded = load_candidate_document(
        catalogue_root,
        &selection.candidate.candidate.catalogue_relative_path,
        Some(&selection.candidate_digest),
    )
    .map_err(candidate_error)?;
    let entries = selection
        .cheat_selection
        .resolve(&loaded.document)
        .map_err(selection_error)?;
    let rendered = super::cht_document::render_cht_file(
        &entries,
        &[super::cheat_install_plan::GENERATED_FILE_PROVENANCE.to_string()],
    );
    Ok((loaded, rendered))
}

fn destination_fingerprint(
    destination: &ResolvedCheatDestination,
) -> Result<CheatJourneyDestinationFingerprint, CheatInstallPlanError> {
    match destination.state {
        DestinationState::Absent => Ok(CheatJourneyDestinationFingerprint::Missing),
        DestinationState::RegularFile => {
            let file =
                fs::File::open(&destination.path).map_err(|failure| CheatInstallPlanError {
                    kind: super::cheat_install_plan::CheatInstallPlanErrorKind::DestinationUnsafe,
                    path: Some(destination.path.clone()),
                    detail: format!("destination could not be read for preview: {failure}"),
                })?;
            let maximum = super::shared_transaction::SHARED_MAX_SOURCE_BYTES as usize;
            let mut bytes = Vec::with_capacity(maximum.min(8192));
            file.take((maximum + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|failure| CheatInstallPlanError {
                    kind: super::cheat_install_plan::CheatInstallPlanErrorKind::DestinationUnsafe,
                    path: Some(destination.path.clone()),
                    detail: format!("destination could not be read for preview: {failure}"),
                })?;
            if bytes.len() > maximum {
                return Err(CheatInstallPlanError {
                    kind: super::cheat_install_plan::CheatInstallPlanErrorKind::DestinationUnsafe,
                    path: Some(destination.path.clone()),
                    detail: "destination exceeds the bounded transaction preview size".to_string(),
                });
            }
            Ok(CheatJourneyDestinationFingerprint::Existing {
                sha256: digest(&bytes),
            })
        }
        _ => Err(CheatInstallPlanError {
            kind: super::cheat_install_plan::CheatInstallPlanErrorKind::DestinationUnsafe,
            path: Some(destination.path.clone()),
            detail: "destination is not a regular file or an absent path".to_string(),
        }),
    }
}

fn shared_destination_matches_preview(
    state: PreviewDestinationState,
    digest_value: Option<&str>,
    preview: &CheatJourneyPreview,
) -> bool {
    match (&preview.destination_fingerprint, state) {
        (CheatJourneyDestinationFingerprint::Missing, PreviewDestinationState::Missing) => true,
        (
            CheatJourneyDestinationFingerprint::Existing { sha256 },
            PreviewDestinationState::RegularFileIdentical,
        )
        | (
            CheatJourneyDestinationFingerprint::Existing { sha256 },
            PreviewDestinationState::RegularFileDifferent,
        ) => digest_value == Some(sha256.as_str()),
        _ => false,
    }
}

fn format_for_candidate(
    snapshot: &CheatCatalogueSnapshot,
    candidate: &CheatCandidate,
) -> CheatCatalogueFormat {
    snapshot
        .games
        .iter()
        .find(|record| {
            let full = &record.source_file_path.display;
            let root = &snapshot.source_root.display;
            let relative = full
                .strip_prefix(root.as_str())
                .map(|rest| rest.trim_start_matches(['/', '\\']).to_string())
                .filter(|rest| !rest.is_empty())
                .unwrap_or_else(|| full.clone());
            relative == candidate.catalogue_relative_path
        })
        .map_or(CheatCatalogueFormat::RetroarchChtDirectory, |record| {
            record.format
        })
}

fn preview_digest(preview: &CheatJourneyPreview) -> String {
    let mut bytes = Vec::new();
    for value in [
        preview.game.identity_key.as_str(),
        &preview.game.selected_archive.to_string_lossy(),
        preview.candidate.candidate.catalogue_relative_path.as_str(),
        preview.candidate_digest.as_str(),
        preview.destination.path.to_string_lossy().as_ref(),
        preview.rendered_digest.as_str(),
        preview.profile_id.as_str(),
        preview.source_mode.as_str(),
    ] {
        bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    for entry in &preview.selection.entries {
        bytes.extend_from_slice(&entry.source_index.to_be_bytes());
        bytes.push(u8::from(entry.selected));
        bytes.push(u8::from(entry.enabled));
        bytes.push(u8::from(entry.selectable));
    }
    bytes.push(preview.candidate.candidate.classification as u8);
    bytes.push(preview.candidate.candidate.manually_selectable as u8);
    bytes.push(preview.candidate.format as u8);
    match &preview.destination_fingerprint {
        CheatJourneyDestinationFingerprint::Missing => bytes.push(0),
        CheatJourneyDestinationFingerprint::Existing { sha256 } => {
            bytes.push(1);
            bytes.extend_from_slice(sha256.as_bytes());
        }
    }
    digest(&bytes)
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn candidate_error(failure: CheatInstallPlanError) -> CheatJourneyError {
    error(CheatJourneyErrorKind::CandidateChanged, failure.detail)
}

fn selection_error(failure: CheatInstallPlanError) -> CheatJourneyError {
    error(CheatJourneyErrorKind::SelectionInvalid, failure.detail)
}

fn destination_error(failure: CheatInstallPlanError) -> CheatJourneyError {
    error(
        CheatJourneyErrorKind::DestinationUnavailable,
        failure.detail,
    )
}

fn error(kind: CheatJourneyErrorKind, detail: impl Into<String>) -> CheatJourneyError {
    CheatJourneyError {
        kind,
        detail: detail.into(),
    }
}
