//! Verified, deliberately narrow CUE/BIN to CHD conversion.
//!
//! Conversion is not considered successful when `chdman` exits successfully.
//! The staged output must independently produce the same canonical optical
//! fingerprint as the source before the existing journaled Repair engine is
//! allowed to finalize it.
//! The admitted original CUE text and output CHD metadata must also satisfy
//! the same narrow layout contract; payload identity alone cannot prove gaps
//! or additional indexes survived conversion.

use crate::optical_preservation as layout_contract;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::rename_apply::journal::{journal_path, read_journal, write_journal};
use crate::dat::rename_apply::model::{
    ObjectIdentity, RenameTransaction, RollbackResult, TransactionState, TransactionSummary,
};
use crate::dat::sources::now_unix;
use crate::optical_fingerprint::{
    CanonicalOpticalFingerprint, OpticalFingerprintComparison, compare_optical_fingerprints,
    fingerprint_chd, fingerprint_cue_layout,
};
use crate::repair::execute::{
    RepairApplyExecution, RepairExecutionError, RepairExecutionOptions, RepairReverifyOutcome,
    RepairTransactionResult, apply_repair_transaction, build_repair_transaction,
    reverify_transaction, rollback_repair_transaction,
};
use crate::repair::plan::{RepairPlan, RepairPlanId, build_repair_plan};
use crate::repair::proposal::{
    RepairAction, RepairEvidence, RepairEvidenceKind, RepairProposal, RepairProposalId, SafetyState,
};
use crate::safe_read::TrustedRoots;

const CHDMAN_TIMEOUT: Duration = Duration::from_secs(30);

/// Staging directory name prefix used while `chdman` writes its output.
/// Named so the operation-receipt projection layer (`crate::operation`) can
/// recognise a disc-conversion output transaction from its journal alone,
/// without duplicating this literal or depending on conversion internals
/// beyond this one stable string.
pub const DISC_CONVERSION_STAGING_PREFIX: &str = ".emuwiz-chd-";

/// Quarantine sub-directory name used when the user explicitly requests
/// source replacement (`ChdConversionSourceMode::QuarantineSource`). Named
/// for the same reason as [`DISC_CONVERSION_STAGING_PREFIX`]: a stable,
/// single-source-of-truth literal the projection layer can match on.
pub const DISC_CONVERSION_QUARANTINE_SUBDIR: &str = "optical-conversion";

/// Marks the start of the specific, human-readable preservation reason that a
/// refused layout carries (stored INDEX 00 pregap vs requested PREGAP vs
/// ambiguous). It runs to the closing `]` at the end of the message.
pub const LAYOUT_PRESERVATION_DETAIL_MARKER: &str = "[layout preservation: ";

/// The specific preservation reason inside a refused-source message, if any.
pub fn layout_preservation_detail(message: &str) -> Option<&str> {
    let (_, rest) = message.split_once(LAYOUT_PRESERVATION_DETAIL_MARKER)?;
    Some(rest.strip_suffix(']').unwrap_or(rest))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChdConversionSourceMode {
    KeepSource,
    QuarantineSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChdConversionError {
    InvalidSource(String),
    InvalidTarget(String),
    ChdmanUnavailable(String),
    ProcessFailed(String),
    VerificationFailed(String),
    StaleSource(PathBuf),
    StaleOutput(PathBuf),
    Transaction(RepairExecutionError),
}

impl std::fmt::Display for ChdConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSource(s) => write!(f, "unsupported CUE/BIN source: {s}"),
            Self::InvalidTarget(s) => write!(f, "unsafe CHD target: {s}"),
            Self::ChdmanUnavailable(s) => write!(f, "chdman is unavailable: {s}"),
            Self::ProcessFailed(s) => write!(f, "chdman conversion failed: {s}"),
            Self::VerificationFailed(s) => write!(f, "CHD verification failed: {s}"),
            Self::StaleSource(p) => write!(f, "source changed during conversion: {}", p.display()),
            Self::StaleOutput(p) => write!(f, "staged output changed: {}", p.display()),
            Self::Transaction(e) => write!(f, "conversion transaction failed: {e}"),
        }
    }
}

impl std::error::Error for ChdConversionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChdConversionPlan {
    pub cue_path: PathBuf,
    pub bin_path: PathBuf,
    pub target_path: PathBuf,
    pub source_fingerprint: CanonicalOpticalFingerprint,
    /// Ordered source facts admitted by the conversion layout contract.
    /// The original CUE text and BIN remain bound by full-file source hashes.
    pub source_layout: crate::ingestion::cue_bin::CueLayout,
    pub cue_identity: crate::dat::rename_apply::model::ObjectIdentity,
    pub bin_identity: crate::dat::rename_apply::model::ObjectIdentity,
    pub chdman_path: PathBuf,
    pub source_mode: ChdConversionSourceMode,
}

#[derive(Debug)]
pub struct ChdConversionTransaction {
    pub output: RepairTransactionResult,
    pub source_quarantine: Option<RepairTransactionResult>,
    staged_path: PathBuf,
}

// Optical-only pairing in the journal's existing forward-compatible extension
// bag. The two ordinary rename journals remain authoritative for move/recovery
// state; this records ordering and the reviewed originals, not another engine.
const CHD_RECOVERY_KEY: &str = "chd_conversion_recovery";
const CHD_OUTPUT_ID_KEY: &str = "chd_conversion_output_id";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ChdRecoveryLink {
    version: u8,
    originals: Vec<ChdOriginal>,
    source_quarantine_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ChdOriginal {
    path: PathBuf,
    identity: ObjectIdentity,
}

fn recovery_link(transaction: &RenameTransaction) -> Result<Option<ChdRecoveryLink>, String> {
    let Some(value) = transaction.unknown.get(CHD_RECOVERY_KEY) else {
        return Ok(None);
    };
    let link: ChdRecoveryLink = serde_json::from_value(value.clone())
        .map_err(|error| format!("unreadable CHD recovery link: {error}"))?;
    if link.version != 1
        || link.originals.len() != 2
        || link.originals[0].path == link.originals[1].path
        || link
            .originals
            .iter()
            .any(|original| original.identity.freshness.is_none())
        || link.source_quarantine_id.as_ref().is_some_and(|id| {
            id == &transaction.transaction_id || journal_path(Path::new("."), id).is_none()
        })
    {
        return Err("invalid CHD recovery link; manual recovery required".into());
    }
    Ok(Some(link))
}

fn transaction_snapshot(transaction: &RenameTransaction) -> RepairTransactionResult {
    RepairTransactionResult {
        transaction: transaction.clone(),
        summary: TransactionSummary::from_transaction(transaction),
        reverify: reverify_transaction(transaction),
    }
}

/// Recognises output journals, including legacy ones that lack a safe pairing.
/// GUI journal Undo must not bypass the original-source restoration guard.
pub fn is_chd_conversion_output(transaction: &RenameTransaction) -> bool {
    transaction.unknown.contains_key(CHD_RECOVERY_KEY)
        || transaction
            .entries
            .iter()
            .any(|entry| is_staged_chd_path(&entry.source_path))
}

fn is_staged_chd_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "output.chd")
        && path.parent().and_then(Path::file_name).is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(DISC_CONVERSION_STAGING_PREFIX)
        })
}

fn read_transaction(journal_dir: &Path, id: &str) -> Result<RenameTransaction, String> {
    let path = journal_path(journal_dir, id).ok_or("unsafe conversion journal id")?;
    let transaction = read_journal(&path).map_err(|error| error.to_string())?;
    if transaction.transaction_id != id {
        return Err("conversion journal identity mismatch".into());
    }
    Ok(transaction)
}

fn linked_source(
    journal_dir: &Path,
    output: &RenameTransaction,
    link: &ChdRecoveryLink,
) -> Result<Option<RepairTransactionResult>, String> {
    let Some(id) = &link.source_quarantine_id else {
        return Ok(None);
    };
    let source = read_transaction(journal_dir, id)?;
    if source
        .unknown
        .get(CHD_OUTPUT_ID_KEY)
        .and_then(serde_json::Value::as_str)
        != Some(output.transaction_id.as_str())
        || source.entries.len() != link.originals.len()
        || link.originals.iter().any(|original| {
            !source.entries.iter().any(|entry| {
                entry.source_path == original.path && entry.identity == original.identity
            })
        })
    {
        return Err("contradictory CHD source journal pairing; manual recovery required".into());
    }
    Ok(Some(transaction_snapshot(&source)))
}

/// Reloads a conversion from its paired ordinary journals after restart. This
/// only reads evidence; explicit Undo uses the existing shared rollback rules.
/// Missing/corrupt/legacy pairing is refused rather than guessing a source set.
pub fn load_chd_conversion_transaction(
    journal_dir: &Path,
    output_id: &str,
) -> Result<ChdConversionTransaction, String> {
    let output = read_transaction(journal_dir, output_id)?;
    if output.entries.len() != 1 || !is_staged_chd_path(&output.entries[0].source_path) {
        return Err("not a single-output CHD conversion journal".into());
    }
    let link = recovery_link(&output)?.ok_or(
        "legacy CHD journal has no original-source recovery proof; manual recovery required",
    )?;
    let source_quarantine = linked_source(journal_dir, &output, &link)?;
    let staged_path = output.entries[0].source_path.clone();
    Ok(ChdConversionTransaction {
        output: transaction_snapshot(&output),
        source_quarantine,
        staged_path,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChdConversionResult {
    pub target_path: PathBuf,
    pub source_fingerprint: CanonicalOpticalFingerprint,
    pub output_fingerprint: CanonicalOpticalFingerprint,
    pub source_mode: ChdConversionSourceMode,
    /// True only when both originals were moved and re-verified in quarantine.
    pub source_quarantined: bool,
    /// What became of the originals. A verified, published CHD is kept even
    /// when the requested source quarantine could not be completed.
    pub source_cleanup: ChdSourceCleanup,
    pub transaction_id: String,
}

/// The outcome of the source-replacement step that follows a verified,
/// published conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChdSourceCleanup {
    /// `KeepSource`: the originals were never going to move.
    NotRequested,
    /// Both originals were moved to quarantine and re-verified there.
    Quarantined,
    /// The conversion succeeded and the CHD is published, but the originals
    /// could not be quarantined as one unit. Partial success: it needs the
    /// user's attention and is never reported as a completed replacement.
    QuarantineFailed {
        detail: String,
        /// Whether every original is proven back at (or never left) its
        /// reviewed path. When false, the unresolved quarantine transaction
        /// is retained in [`ChdConversionTransaction::source_quarantine`].
        sources_restored: bool,
    },
}

impl ChdConversionResult {
    /// Whether the conversion is published but its requested source cleanup
    /// is not, so it must not be presented as an ordinary full success.
    pub fn needs_attention(&self) -> bool {
        matches!(
            self.source_cleanup,
            ChdSourceCleanup::QuarantineFailed { .. }
        )
    }
}

/// The explicit policy bound to the currently proven execution lane.  The
/// preview module may describe more formats, but this execution API accepts
/// only this CUE/BIN -> CHD policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChdConversionPolicy {
    pub version: &'static str,
    pub mode: &'static str,
    pub compression: &'static str,
    pub hunk_size: &'static str,
    pub round_trip: &'static str,
}

pub const SAFE_CUE_BIN_TO_CHD_POLICY: ChdConversionPolicy = ChdConversionPolicy {
    version: "cue-bin-to-chd-v1",
    mode: "createcd",
    compression: "cdlz,cdzl,cdfl",
    hunk_size: "8",
    round_trip: "CONTENT_EQUIVALENT",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChdConversionExecutionState {
    Completed,
    ConversionFailed,
    VerificationFailed,
    Stale,
    NeedsCleanup,
}

/// A compact, non-persistent history record produced by safe execution.
/// Large command output is intentionally not retained.
#[derive(Debug, Clone, PartialEq)]
pub struct ChdConversionRecord {
    pub source_cue: PathBuf,
    pub source_bin: PathBuf,
    pub destination: PathBuf,
    pub chdman_path: PathBuf,
    pub chdman_version: Option<String>,
    pub policy: ChdConversionPolicy,
    pub state: ChdConversionExecutionState,
    pub source_fingerprint: CanonicalOpticalFingerprint,
    pub output_fingerprint: CanonicalOpticalFingerprint,
    pub source_logical_bytes: u64,
    pub output_bytes: u64,
    pub savings_bytes: i64,
    pub savings_percent: f64,
    pub warning: Option<String>,
}

/// Execute only the already-proven, byte-preserving-source conversion lane.
/// In particular, this wrapper rejects the older quarantine/source-replace
/// mode so callers cannot accidentally turn Phase 3A into source cleanup.
pub fn execute_safe_chd_conversion(
    plan: &ChdConversionPlan,
    trusted: TrustedRoots,
    journal_dir: &Path,
    staging_root: &Path,
    cancel: &AtomicBool,
) -> Result<(ChdConversionRecord, ChdConversionTransaction), ChdConversionError> {
    if plan.source_mode != ChdConversionSourceMode::KeepSource {
        return Err(ChdConversionError::InvalidTarget(
            "safe execution requires KeepSource; source cleanup is not part of this phase".into(),
        ));
    }
    let (result, transaction) =
        execute_chd_conversion(plan, trusted, journal_dir, staging_root, cancel)?;
    let source_logical_bytes = fs::metadata(&plan.cue_path)
        .map_err(|e| ChdConversionError::VerificationFailed(e.to_string()))?
        .len()
        .saturating_add(
            fs::metadata(&plan.bin_path)
                .map_err(|e| ChdConversionError::VerificationFailed(e.to_string()))?
                .len(),
        );
    let output_bytes = fs::metadata(&plan.target_path)
        .map_err(|e| ChdConversionError::VerificationFailed(e.to_string()))?
        .len();
    let savings_bytes = source_logical_bytes as i128 - output_bytes as i128;
    let savings_percent = if source_logical_bytes == 0 {
        0.0
    } else {
        (savings_bytes as f64 / source_logical_bytes as f64) * 100.0
    };
    Ok((
        ChdConversionRecord {
            source_cue: plan.cue_path.clone(),
            source_bin: plan.bin_path.clone(),
            destination: plan.target_path.clone(),
            chdman_path: plan.chdman_path.clone(),
            chdman_version: None,
            policy: SAFE_CUE_BIN_TO_CHD_POLICY,
            state: ChdConversionExecutionState::Completed,
            source_fingerprint: result.source_fingerprint,
            output_fingerprint: result.output_fingerprint,
            source_logical_bytes,
            output_bytes,
            savings_bytes: savings_bytes.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
            savings_percent,
            warning: Some(
                "The original CUE/BIN remains in place; no disk space has been reclaimed.".into(),
            ),
        },
        transaction,
    ))
}

struct StageGuard {
    directory: PathBuf,
    output: PathBuf,
    retain: bool,
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        if self.retain {
            return;
        }
        let _ = fs::remove_file(&self.output);
        let _ = fs::remove_dir(&self.directory);
    }
}

fn safe_regular(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("must be a regular non-symlink file".into());
    }
    Ok(())
}

fn safe_destination_parent(parent: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("destination path contains a symlink or non-directory component".into());
        }
    }
    Ok(())
}

fn resolve_chdman(explicit: Option<&Path>) -> Result<PathBuf, ChdConversionError> {
    if let Some(path) = explicit {
        safe_regular(path).map_err(ChdConversionError::ChdmanUnavailable)?;
        #[cfg(unix)]
        if std::os::unix::fs::PermissionsExt::mode(
            &fs::metadata(path)
                .map_err(|e| ChdConversionError::ChdmanUnavailable(e.to_string()))?
                .permissions(),
        ) & 0o111
            == 0
        {
            return Err(ChdConversionError::ChdmanUnavailable(
                "executable is not executable".into(),
            ));
        }
        return Ok(path.to_path_buf());
    }
    let preferred = Path::new("/usr/bin/chdman");
    if preferred.exists() {
        return resolve_chdman(Some(preferred));
    }
    Err(ChdConversionError::ChdmanUnavailable(
        "no reviewed /usr/bin/chdman binding was found".into(),
    ))
}

pub fn build_chd_conversion_plan(
    cue_path: &Path,
    target_path: &Path,
    source_mode: ChdConversionSourceMode,
    chdman_path: Option<&Path>,
) -> Result<ChdConversionPlan, ChdConversionError> {
    safe_regular(cue_path).map_err(|e| ChdConversionError::InvalidSource(format!("CUE: {e}")))?;
    if fs::metadata(cue_path)
        .map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?
        .len()
        > layout_contract::MAX_CONVERSION_CUE_BYTES
    {
        return Err(ChdConversionError::InvalidSource(
            "CUE exceeds the conversion size limit".into(),
        ));
    }
    let cue_identity =
        capture_identity(cue_path).map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?;
    let layout = layout_contract::source_layout(cue_path, source_mode)
        .map_err(ChdConversionError::InvalidSource)?;
    let track = layout
        .supported_single_mode1_2048()
        .map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?;
    safe_regular(&track.path)
        .map_err(|e| ChdConversionError::InvalidSource(format!("BIN: {e}")))?;
    let bin_identity = capture_identity(&track.path)
        .map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?;
    let source_fingerprint = fingerprint_cue_layout(&layout)
        .map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?;
    let parent = target_path
        .parent()
        .ok_or_else(|| ChdConversionError::InvalidTarget("target has no parent".into()))?;
    if !parent.is_dir() {
        return Err(ChdConversionError::InvalidTarget(
            "target parent is not an existing directory".into(),
        ));
    }
    safe_destination_parent(parent).map_err(ChdConversionError::InvalidTarget)?;
    if target_path.exists() || fs::symlink_metadata(target_path).is_ok() {
        return Err(ChdConversionError::InvalidTarget(
            "target already exists".into(),
        ));
    }
    if target_path == cue_path || target_path == track.path {
        return Err(ChdConversionError::InvalidTarget(
            "target overlaps source".into(),
        ));
    }
    let plan = ChdConversionPlan {
        cue_path: cue_path.to_path_buf(),
        bin_path: track.path.clone(),
        target_path: target_path.to_path_buf(),
        source_fingerprint,
        source_layout: layout,
        cue_identity,
        bin_identity,
        chdman_path: resolve_chdman(chdman_path)?,
        source_mode,
    };
    revalidate_source(&plan)?;
    Ok(plan)
}

fn proposal(
    id: &str,
    source: &Path,
    destination: &Path,
    reason: String,
    identity: &crate::dat::rename_apply::model::ObjectIdentity,
) -> Result<RepairProposal, ChdConversionError> {
    Ok(RepairProposal {
        id: RepairProposalId::new(id).ok_or_else(|| {
            ChdConversionError::Transaction(RepairExecutionError::Build {
                detail: "unsafe proposal id".into(),
            })
        })?,
        action: RepairAction::MovePath {
            destination: destination.to_path_buf(),
        },
        source_path: source.to_path_buf(),
        reason,
        evidence: vec![RepairEvidence::new(
            RepairEvidenceKind::UserRequestedOrganisation,
            "verified CUE/BIN conversion",
        )],
        expected_source_identity: Some(identity.clone()),
        originating_audit: None,
        safety: SafetyState::Safe,
        blockers: Vec::new(),
        warnings: Vec::new(),
        dat_source_id: None,
        dat_source_display: None,
        game_name: None,
        rom_name: None,
        verdict_label: Some("Verified CUE/BIN to CHD conversion".into()),
        match_confident: true,
        is_outer_archive: false,
        is_outer_archive_verified: false,
        survivor_path: None,
    })
}

fn plan_for_moves(id: &str, moves: Vec<RepairProposal>) -> Result<RepairPlan, ChdConversionError> {
    Ok(build_repair_plan(
        RepairPlanId::new(id).ok_or_else(|| {
            ChdConversionError::Transaction(RepairExecutionError::Build {
                detail: "unsafe plan id".into(),
            })
        })?,
        0,
        now_unix(),
        Some("verified-optical-conversion-v1".into()),
        moves,
    ))
}

fn revalidate_source(plan: &ChdConversionPlan) -> Result<(), ChdConversionError> {
    if capture_identity(&plan.cue_path).ok().as_ref() != Some(&plan.cue_identity) {
        return Err(ChdConversionError::StaleSource(plan.cue_path.clone()));
    }
    if capture_identity(&plan.bin_path).ok().as_ref() != Some(&plan.bin_identity) {
        return Err(ChdConversionError::StaleSource(plan.bin_path.clone()));
    }
    let fresh_layout = layout_contract::source_layout(&plan.cue_path, plan.source_mode)
        .map_err(ChdConversionError::InvalidSource)?;
    if fresh_layout != plan.source_layout
        || fresh_layout.tracks[0].path != plan.bin_path
        || capture_identity(&plan.cue_path).ok().as_ref() != Some(&plan.cue_identity)
    {
        return Err(ChdConversionError::StaleSource(plan.cue_path.clone()));
    }
    Ok(())
}

pub fn execute_chd_conversion(
    plan: &ChdConversionPlan,
    trusted: TrustedRoots,
    journal_dir: &Path,
    quarantine_root: &Path,
    cancel: &AtomicBool,
) -> Result<(ChdConversionResult, ChdConversionTransaction), ChdConversionError> {
    revalidate_source(plan)?;
    let fresh = fingerprint_cue_layout(&plan.source_layout)
        .map_err(|_| ChdConversionError::StaleSource(plan.cue_path.clone()))?;
    if fresh != plan.source_fingerprint {
        return Err(ChdConversionError::StaleSource(plan.cue_path.clone()));
    }
    revalidate_source(plan)?;
    resolve_chdman(Some(&plan.chdman_path))?;
    fs::create_dir_all(journal_dir).map_err(|e| {
        ChdConversionError::Transaction(RepairExecutionError::Build {
            detail: e.to_string(),
        })
    })?;
    let parent = plan
        .target_path
        .parent()
        .ok_or_else(|| ChdConversionError::InvalidTarget("target has no parent".into()))?;
    let staging = (0..100u32)
        .find_map(|attempt| {
            let candidate = parent.join(format!(
                "{DISC_CONVERSION_STAGING_PREFIX}{}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => Some(candidate),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(_) => None,
            }
        })
        .ok_or_else(|| {
            ChdConversionError::InvalidTarget("could not create unique staging directory".into())
        })?;
    let staged = staging.join("output.chd");
    let mut stage_guard = StageGuard {
        directory: staging.clone(),
        output: staged.clone(),
        retain: false,
    };
    let args = [
        "createcd".as_ref(),
        "--input".as_ref(),
        plan.cue_path.as_os_str(),
        "--output".as_ref(),
        staged.as_os_str(),
    ];
    crate::run_command_os_with_timeout(&plan.chdman_path.to_string_lossy(), &args, CHDMAN_TIMEOUT)
        .map_err(|e| ChdConversionError::ProcessFailed(e.to_string()))?;
    revalidate_source(plan)?;
    safe_regular(&staged).map_err(ChdConversionError::VerificationFailed)?;
    let staged_identity = capture_identity(&staged)
        .map_err(|e| ChdConversionError::VerificationFailed(e.to_string()))?;
    layout_contract::verify_output_layout(
        &staged,
        plan.source_fingerprint.structure.logical_sector_count,
    )
    .map_err(ChdConversionError::VerificationFailed)?;
    layout_contract::verify_output_storage(
        &staged,
        plan.source_fingerprint.structure.logical_sector_count,
    )
    .map_err(ChdConversionError::VerificationFailed)?;
    let output_fingerprint = fingerprint_chd(&staged)
        .map_err(|e| ChdConversionError::VerificationFailed(e.to_string()))?;
    if compare_optical_fingerprints(&plan.source_fingerprint, &output_fingerprint)
        != OpticalFingerprintComparison::Equivalent
    {
        return Err(ChdConversionError::VerificationFailed(
            "canonical optical fingerprints differ".into(),
        ));
    }
    revalidate_source(plan)?;
    if capture_identity(&staged).ok().as_ref() != Some(&staged_identity) {
        return Err(ChdConversionError::StaleOutput(staged));
    }
    let output_plan = plan_for_moves(
        &format!("chd-conversion-output-{}", now_unix()),
        vec![RepairProposal {
            id: RepairProposalId::new(format!("chd-output-{}", now_unix())).unwrap(),
            action: RepairAction::MovePath {
                destination: plan.target_path.clone(),
            },
            source_path: staged.clone(),
            reason: "finalize independently fingerprint-verified CHD".into(),
            evidence: vec![RepairEvidence::new(
                RepairEvidenceKind::UserRequestedOrganisation,
                "source/output canonical optical fingerprints match",
            )],
            expected_source_identity: Some(staged_identity.clone()),
            originating_audit: None,
            safety: SafetyState::Safe,
            blockers: Vec::new(),
            warnings: Vec::new(),
            dat_source_id: None,
            dat_source_display: None,
            game_name: None,
            rom_name: None,
            verdict_label: Some("Verified CHD output".into()),
            match_confident: true,
            is_outer_archive: false,
            is_outer_archive_verified: false,
            survivor_path: None,
        }],
    )?;
    let mut output_tx =
        build_repair_transaction(&output_plan).map_err(ChdConversionError::Transaction)?;
    // Saved with the output's first checkpoint, before publication. Even a
    // source-quarantine setup failure leaves restart Undo with original proof.
    output_tx.unknown.insert(
        CHD_RECOVERY_KEY.into(),
        serde_json::to_value(ChdRecoveryLink {
            version: 1,
            originals: vec![
                ChdOriginal {
                    path: plan.cue_path.clone(),
                    identity: plan.cue_identity.clone(),
                },
                ChdOriginal {
                    path: plan.bin_path.clone(),
                    identity: plan.bin_identity.clone(),
                },
            ],
            source_quarantine_id: None,
        })
        .map_err(|error| transaction_failure(error.to_string()))?,
    );
    let options = RepairExecutionOptions {
        trusted: trusted.clone(),
        journal_dir: journal_dir.to_path_buf(),
        audit_cache: crate::dat::sources::audit_cache::AuditCacheConfig::Disabled,
    };
    // From here the journal owns the staged path, including an ambiguous
    // publication failure: retain the staging directory unless a failure
    // path below proves the staged CHD is back in it.
    stage_guard.retain = true;
    let mut output = match confirmed(
        apply_repair_transaction(&mut RepairApplyExecution {
            transaction: &mut output_tx,
            current_generation: 0,
            options: &options,
            cancel,
        }),
        1,
    ) {
        Ok(output) => output,
        Err(detail) => {
            let (_, rollback) = roll_back_unconfirmed(&mut output_tx, journal_dir);
            stage_guard.retain = !staged_output_recovered(&staged, &staged_identity);
            return Err(transaction_failure(format!(
                "CHD publication was not confirmed ({detail}); no source was moved; \
                 publication rollback: {rollback}"
            )));
        }
    };
    // Only a confirmed publication reaches this point. A source quarantine
    // that then fails never withdraws the verified CHD: the originals are put
    // back and the result says the cleanup needs attention.
    let (source_quarantine, source_cleanup) =
        if plan.source_mode == ChdConversionSourceMode::QuarantineSource {
            match quarantine_sources(plan, quarantine_root, &options, cancel, &mut output) {
                Ok(result) => (Some(result), ChdSourceCleanup::Quarantined),
                Err(failure) => (
                    failure.unresolved,
                    ChdSourceCleanup::QuarantineFailed {
                        detail: failure.detail,
                        sources_restored: failure.sources_restored,
                    },
                ),
            }
        } else {
            (None, ChdSourceCleanup::NotRequested)
        };
    let transaction_id = output.transaction.transaction_id.clone();
    Ok((
        ChdConversionResult {
            target_path: plan.target_path.clone(),
            source_fingerprint: plan.source_fingerprint.clone(),
            output_fingerprint,
            source_mode: plan.source_mode,
            source_quarantined: source_cleanup == ChdSourceCleanup::Quarantined,
            source_cleanup,
            transaction_id,
        },
        ChdConversionTransaction {
            output,
            source_quarantine,
            staged_path: staged,
        },
    ))
}

fn transaction_failure(detail: String) -> ChdConversionError {
    ChdConversionError::Transaction(RepairExecutionError::Apply(detail))
}

/// Accepts an apply only when the journaled transaction itself settled.
///
/// The shared engine returns `Ok` with the transaction in `ApplyFailed` when
/// an entry fails after the whole-batch preflight, so the outer `Ok` is never
/// evidence of publication. Every one of the `expected` entries must be
/// applied and re-verified at its destination - the same rule
/// `wiiu_conversion` applies to its own publication.
fn confirmed(
    outcome: Result<RepairTransactionResult, RepairExecutionError>,
    expected: usize,
) -> Result<RepairTransactionResult, String> {
    let result = outcome.map_err(|error| error.to_string())?;
    if result.transaction.state == TransactionState::Applied
        && result.summary.applied == expected
        && result.summary.failed == 0
        && result.reverify.len() == expected
        && result
            .reverify
            .iter()
            .all(|entry| entry.outcome == RepairReverifyOutcome::Verified)
    {
        return Ok(result);
    }
    let reasons = result
        .transaction
        .entries
        .iter()
        .filter_map(|entry| entry.failure_reason.as_deref())
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "{} of {expected} move(s) applied, transaction state {:?}{}{reasons}",
        result.summary.applied,
        result.transaction.state,
        if reasons.is_empty() { "" } else { ": " },
    ))
}

/// Reverses whatever an unconfirmed transaction applied, reporting the
/// outcome rather than hiding it. Failure cleanup must not be cancelled by
/// the forward operation's flag; the reused rollback still requires identity
/// and no-clobber proof for every reverse move.
fn roll_back_unconfirmed(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
) -> (bool, String) {
    if transaction.state == TransactionState::Planned {
        // Refused before the engine journalled or moved anything.
        return (true, "nothing was applied".into());
    }
    match rollback_repair_transaction(transaction, journal_dir, &AtomicBool::new(false)) {
        Ok(RollbackResult::FullyRolledBack) => (true, "fully rolled back".into()),
        Ok(other) => (
            false,
            format!("NOT fully rolled back, manual recovery required: {other:?}"),
        ),
        Err(error) => (
            false,
            format!("NOT rolled back, manual recovery required: {error}"),
        ),
    }
}

/// Whether the reviewed staged CHD is provably back in (or never left) its
/// staging directory, so the disposable stage may be discarded. Anything
/// else keeps the directory for the journal's recovery rules.
fn staged_output_recovered(
    staged: &Path,
    staged_identity: &crate::dat::rename_apply::model::ObjectIdentity,
) -> bool {
    capture_identity(staged).ok().as_ref() == Some(staged_identity)
}

/// Why the originals were not quarantined, and what state they are in.
struct SourceQuarantineFailure {
    detail: String,
    sources_restored: bool,
    /// The quarantine transaction, kept only when its rollback did not prove
    /// every original back in place, so [`rollback_chd_conversion`] and the
    /// journal's recovery rules can still settle it.
    unresolved: Option<RepairTransactionResult>,
}

/// Moves the reviewed CUE and BIN into quarantine as one unit. Either both
/// moves are applied and re-verified, or whatever was moved is put back and
/// the call fails.
fn quarantine_sources(
    plan: &ChdConversionPlan,
    quarantine_root: &Path,
    options: &RepairExecutionOptions,
    cancel: &AtomicBool,
    output: &mut RepairTransactionResult,
) -> Result<RepairTransactionResult, SourceQuarantineFailure> {
    // Nothing has moved until the transaction is applied.
    let untouched = |detail: String| SourceQuarantineFailure {
        detail: format!("source quarantine could not start ({detail}); no original was moved"),
        sources_restored: true,
        unresolved: None,
    };
    let dir = quarantine_root
        .join(DISC_CONVERSION_QUARANTINE_SUBDIR)
        .join(&plan.source_fingerprint.canonical_sha256[..16]);
    fs::create_dir_all(&dir).map_err(|e| untouched(e.to_string()))?;
    let p1 = proposal(
        "chd-source-cue",
        &plan.cue_path,
        &dir.join(plan.cue_path.file_name().unwrap()),
        "quarantine original CUE after verified conversion".into(),
        &plan.cue_identity,
    )
    .map_err(|e| untouched(e.to_string()))?;
    let p2 = proposal(
        "chd-source-bin",
        &plan.bin_path,
        &dir.join(plan.bin_path.file_name().unwrap()),
        "quarantine original BIN after verified conversion".into(),
        &plan.bin_identity,
    )
    .map_err(|e| untouched(e.to_string()))?;
    let source_plan = plan_for_moves(
        &format!("chd-conversion-source-{}", now_unix()),
        vec![p1, p2],
    )
    .map_err(|e| untouched(e.to_string()))?;
    let mut tx = build_repair_transaction(&source_plan).map_err(|e| untouched(e.to_string()))?;
    tx.unknown.insert(
        CHD_OUTPUT_ID_KEY.into(),
        output.transaction.transaction_id.clone().into(),
    );
    let mut link = recovery_link(&output.transaction)
        .map_err(untouched)?
        .ok_or_else(|| untouched("missing original-source recovery proof".into()))?;
    link.source_quarantine_id = Some(tx.transaction_id.clone());
    // Both identity and pairing must survive restart BEFORE the first source
    // move. A failed checkpoint refuses source mutation, keeping the CHD.
    write_journal(&options.journal_dir, &tx).map_err(|error| untouched(error.to_string()))?;
    output.transaction.unknown.insert(
        CHD_RECOVERY_KEY.into(),
        serde_json::to_value(link).map_err(|error| untouched(error.to_string()))?,
    );
    write_journal(&options.journal_dir, &output.transaction)
        .map_err(|error| untouched(error.to_string()))?;
    let outcome = apply_repair_transaction(&mut RepairApplyExecution {
        transaction: &mut tx,
        current_generation: 0,
        options,
        cancel,
    });
    confirmed(outcome, 2).map_err(|detail| {
        let (rolled_back, rollback) = roll_back_unconfirmed(&mut tx, &options.journal_dir);
        let sources_restored = rolled_back
            && tx.entries.iter().all(|entry| {
                capture_identity(&entry.source_path).ok().as_ref() == Some(&entry.identity)
            });
        SourceQuarantineFailure {
            detail: format!(
                "source quarantine was not confirmed ({detail}); source rollback: {rollback}"
            ),
            sources_restored,
            // An outer Apply Err still mutated `tx`. Never depend on the
            // presence of an Ok wrapper to preserve its recovery identity.
            unresolved: (!sources_restored).then(|| transaction_snapshot(&tx)),
        }
    })
}

pub fn rollback_chd_conversion(
    transaction: &mut ChdConversionTransaction,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<RollbackResult, String> {
    let link = recovery_link(&transaction.output.transaction)?;
    if transaction.source_quarantine.is_none()
        && let Some(link) = &link
    {
        transaction.source_quarantine =
            linked_source(journal_dir, &transaction.output.transaction, link)?;
    }
    if let Some(source) = transaction.source_quarantine.as_mut() {
        let outcome = rollback_repair_transaction(&mut source.transaction, journal_dir, cancel);
        *source = transaction_snapshot(&source.transaction);
        match outcome? {
            RollbackResult::FullyRolledBack => {}
            incomplete => return Ok(incomplete),
        }
    }
    // A typed full rollback can skip never-applied entries. Before removing
    // the verified CHD, also prove every reviewed original at its actual path.
    let originals_restored = if let Some(link) = &link {
        link.originals.iter().all(|original| {
            capture_identity(&original.path).ok().as_ref() == Some(&original.identity)
        })
    } else {
        transaction.source_quarantine.as_ref().is_none_or(|source| {
            source.transaction.entries.iter().all(|entry| {
                capture_identity(&entry.source_path).ok().as_ref() == Some(&entry.identity)
            })
        })
    };
    if !originals_restored {
        return Err("original CUE/BIN restoration is not proven; verified CHD retained, manual recovery required".into());
    }
    let outcome =
        rollback_repair_transaction(&mut transaction.output.transaction, journal_dir, cancel);
    transaction.output = transaction_snapshot(&transaction.output.transaction);
    let result = outcome?;
    if result == RollbackResult::FullyRolledBack
        && transaction
            .output
            .transaction
            .entries
            .first()
            .is_some_and(|entry| staged_output_recovered(&transaction.staged_path, &entry.identity))
    {
        let _ = fs::remove_file(&transaction.staged_path);
        if let Some(parent) = transaction.staged_path.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod safe_execution_tests {
    use super::*;

    #[test]
    fn safe_policy_is_explicit_and_content_equivalent() {
        assert_eq!(SAFE_CUE_BIN_TO_CHD_POLICY.mode, "createcd");
        assert_eq!(SAFE_CUE_BIN_TO_CHD_POLICY.round_trip, "CONTENT_EQUIVALENT");
        assert_eq!(SAFE_CUE_BIN_TO_CHD_POLICY.hunk_size, "8");
    }

    #[test]
    fn execution_state_does_not_offer_source_cleanup() {
        assert_eq!(
            ChdConversionExecutionState::Completed,
            ChdConversionExecutionState::Completed
        );
        assert_ne!(
            ChdConversionSourceMode::KeepSource,
            ChdConversionSourceMode::QuarantineSource
        );
    }
}
