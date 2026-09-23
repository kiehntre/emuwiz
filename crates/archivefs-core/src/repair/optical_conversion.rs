//! Verified, deliberately narrow CUE/BIN to CHD conversion.
//!
//! Conversion is not considered successful when `chdman` exits successfully.
//! The staged output must independently produce the same canonical optical
//! fingerprint as the source before the existing journaled Repair engine is
//! allowed to finalize it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::rename_apply::model::RollbackResult;
use crate::dat::sources::now_unix;
use crate::optical_fingerprint::{
    CanonicalOpticalFingerprint, OpticalFingerprintComparison, compare_optical_fingerprints,
    fingerprint_chd, fingerprint_cue_bin,
};
use crate::repair::execute::{
    RepairApplyExecution, RepairExecutionError, RepairExecutionOptions, RepairTransactionResult,
    apply_repair_transaction, build_repair_transaction, rollback_repair_transaction,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChdConversionResult {
    pub target_path: PathBuf,
    pub source_fingerprint: CanonicalOpticalFingerprint,
    pub output_fingerprint: CanonicalOpticalFingerprint,
    pub source_mode: ChdConversionSourceMode,
    pub source_quarantined: bool,
    pub transaction_id: String,
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
    let layout = crate::ingestion::cue_bin::resolve_cue_layout(cue_path)
        .map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?;
    let track = layout
        .supported_single_mode1_2048()
        .map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?;
    safe_regular(&track.path)
        .map_err(|e| ChdConversionError::InvalidSource(format!("BIN: {e}")))?;
    let source_fingerprint = fingerprint_cue_bin(cue_path)
        .map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?;
    let cue_identity =
        capture_identity(cue_path).map_err(|e| ChdConversionError::InvalidSource(e.to_string()))?;
    let bin_identity = capture_identity(&track.path)
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
    Ok(ChdConversionPlan {
        cue_path: cue_path.to_path_buf(),
        bin_path: track.path.clone(),
        target_path: target_path.to_path_buf(),
        source_fingerprint,
        cue_identity,
        bin_identity,
        chdman_path: resolve_chdman(chdman_path)?,
        source_mode,
    })
}

fn proposal(
    id: &str,
    source: &Path,
    destination: &Path,
    reason: String,
) -> Result<RepairProposal, ChdConversionError> {
    let identity = capture_identity(source).map_err(|e| {
        ChdConversionError::Transaction(RepairExecutionError::Build {
            detail: e.to_string(),
        })
    })?;
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
        expected_source_identity: Some(identity),
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

pub fn execute_chd_conversion(
    plan: &ChdConversionPlan,
    trusted: TrustedRoots,
    journal_dir: &Path,
    quarantine_root: &Path,
    cancel: &AtomicBool,
) -> Result<(ChdConversionResult, ChdConversionTransaction), ChdConversionError> {
    if capture_identity(&plan.cue_path).ok().as_ref() != Some(&plan.cue_identity) {
        return Err(ChdConversionError::StaleSource(plan.cue_path.clone()));
    }
    if capture_identity(&plan.bin_path).ok().as_ref() != Some(&plan.bin_identity) {
        return Err(ChdConversionError::StaleSource(plan.bin_path.clone()));
    }
    let fresh = fingerprint_cue_bin(&plan.cue_path)
        .map_err(|_| ChdConversionError::StaleSource(plan.cue_path.clone()))?;
    if fresh != plan.source_fingerprint {
        return Err(ChdConversionError::StaleSource(plan.cue_path.clone()));
    }
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
    safe_regular(&staged).map_err(ChdConversionError::VerificationFailed)?;
    let output_fingerprint = fingerprint_chd(&staged)
        .map_err(|e| ChdConversionError::VerificationFailed(e.to_string()))?;
    if compare_optical_fingerprints(&plan.source_fingerprint, &output_fingerprint)
        != OpticalFingerprintComparison::Equivalent
    {
        return Err(ChdConversionError::VerificationFailed(
            "canonical optical fingerprints differ".into(),
        ));
    }
    let staged_identity = capture_identity(&staged)
        .map_err(|e| ChdConversionError::VerificationFailed(e.to_string()))?;
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
            expected_source_identity: Some(staged_identity),
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
    let options = RepairExecutionOptions {
        trusted: trusted.clone(),
        journal_dir: journal_dir.to_path_buf(),
        audit_cache: crate::dat::sources::audit_cache::AuditCacheConfig::Disabled,
    };
    let output = apply_repair_transaction(&mut RepairApplyExecution {
        transaction: &mut output_tx,
        current_generation: 0,
        options: &options,
        cancel,
    })
    .map_err(ChdConversionError::Transaction)?;
    // The output transaction's rollback source is inside this directory.
    // Retain the empty staging directory until that transaction is explicitly
    // rolled back or recovery has settled it.
    stage_guard.retain = true;
    let source_quarantine = if plan.source_mode == ChdConversionSourceMode::QuarantineSource {
        let dir = quarantine_root
            .join(DISC_CONVERSION_QUARANTINE_SUBDIR)
            .join(&plan.source_fingerprint.canonical_sha256[..16]);
        fs::create_dir_all(&dir).map_err(|e| {
            ChdConversionError::Transaction(RepairExecutionError::Build {
                detail: e.to_string(),
            })
        })?;
        let p1 = proposal(
            "chd-source-cue",
            &plan.cue_path,
            &dir.join(plan.cue_path.file_name().unwrap()),
            "quarantine original CUE after verified conversion".into(),
        )?;
        let p2 = proposal(
            "chd-source-bin",
            &plan.bin_path,
            &dir.join(plan.bin_path.file_name().unwrap()),
            "quarantine original BIN after verified conversion".into(),
        )?;
        let source_plan = plan_for_moves(
            &format!("chd-conversion-source-{}", now_unix()),
            vec![p1, p2],
        )?;
        let mut tx =
            build_repair_transaction(&source_plan).map_err(ChdConversionError::Transaction)?;
        match apply_repair_transaction(&mut RepairApplyExecution {
            transaction: &mut tx,
            current_generation: 0,
            options: &options,
            cancel,
        }) {
            Ok(result) => Some(result),
            Err(error) => return Err(ChdConversionError::Transaction(error)),
        }
    } else {
        None
    };
    let transaction_id = output.transaction.transaction_id.clone();
    Ok((
        ChdConversionResult {
            target_path: plan.target_path.clone(),
            source_fingerprint: plan.source_fingerprint.clone(),
            output_fingerprint,
            source_mode: plan.source_mode,
            source_quarantined: source_quarantine.is_some(),
            transaction_id,
        },
        ChdConversionTransaction {
            output,
            source_quarantine,
            staged_path: staged,
        },
    ))
}

pub fn rollback_chd_conversion(
    transaction: &mut ChdConversionTransaction,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<RollbackResult, String> {
    if let Some(source) = transaction.source_quarantine.as_mut() {
        rollback_repair_transaction(&mut source.transaction, journal_dir, cancel)?;
    }
    let result =
        rollback_repair_transaction(&mut transaction.output.transaction, journal_dir, cancel)?;
    if transaction.staged_path.exists() {
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
