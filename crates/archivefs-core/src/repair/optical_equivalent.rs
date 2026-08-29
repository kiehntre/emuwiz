//! Evidence-backed equivalent-content review for the narrow optical
//! fingerprint supported by [`crate::optical_fingerprint`].
//!
//! This is deliberately a review adapter, not a second disc parser.  CUE
//! layout and CHD content are fingerprinted by the existing optical module;
//! this module only groups matching fingerprints and sends the complete
//! CUE/BIN representation through the shared journaled Repair engine.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::sources::now_unix;
use crate::ingestion::cue_bin::resolve_cue_layout;
use crate::optical_fingerprint::{
    CanonicalOpticalFingerprint, OpticalDiscStructure, OpticalFingerprintComparison,
    OpticalRepresentation, compare_optical_fingerprints, fingerprint_chd, fingerprint_cue_bin,
};
use crate::repair::exact_duplicate::hash_full_file_sha256;
use crate::repair::execute::{
    RepairApplyExecution, RepairExecutionError, RepairExecutionOptions, RepairTransactionResult,
    apply_repair_transaction, build_repair_transaction, rollback_repair_transaction,
};
use crate::repair::plan::{RepairPlan, RepairPlanId, build_repair_plan};
use crate::repair::proposal::{
    RepairAction, RepairEvidence, RepairEvidenceKind, RepairProposal, RepairProposalId, SafetyState,
};
use crate::safe_read::TrustedRoots;

pub const OPTICAL_EQUIVALENT_SCAN_VERSION: &str = "optical-equivalent-content-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpticalEquivalentFile {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub physical_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpticalEquivalentRepresentation {
    pub kind: OpticalRepresentation,
    pub files: Vec<OpticalEquivalentFile>,
    pub fingerprint: CanonicalOpticalFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpticalEquivalentGroup {
    pub cue_bin: OpticalEquivalentRepresentation,
    pub chd: OpticalEquivalentRepresentation,
    pub structure: OpticalDiscStructure,
    pub canonical_sha256: String,
    pub preferred: PathBuf,
    pub quarantine_candidates: Vec<PathBuf>,
    pub projected_savings: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpticalExcludedCandidate {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpticalEquivalentScanReport {
    pub groups: Vec<OpticalEquivalentGroup>,
    pub excluded: Vec<OpticalExcludedCandidate>,
    pub files_examined: usize,
}

impl OpticalEquivalentScanReport {
    pub fn projected_savings(&self) -> u64 {
        self.groups
            .iter()
            .map(|group| group.projected_savings)
            .sum()
    }
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
}

fn safe_physical_file(
    path: &Path,
    trusted: &TrustedRoots,
) -> Result<OpticalEquivalentFile, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("not a regular non-symlink file".into());
    }
    let identity = hash_full_file_sha256(path, trusted, None).map_err(|error| error.to_string())?;
    Ok(OpticalEquivalentFile {
        path: path.to_path_buf(),
        size_bytes: identity.size_bytes,
        physical_sha256: identity.sha256,
    })
}

fn inspect_cue(
    path: &Path,
    trusted: &TrustedRoots,
) -> Result<OpticalEquivalentRepresentation, String> {
    let fingerprint = fingerprint_cue_bin(path).map_err(|error| error.to_string())?;
    let layout = resolve_cue_layout(path).map_err(|error| error.to_string())?;
    let track = layout
        .supported_single_mode1_2048()
        .map_err(|error| error.to_string())?;
    let cue = safe_physical_file(path, trusted)?;
    let bin = safe_physical_file(&track.path, trusted)?;
    Ok(OpticalEquivalentRepresentation {
        kind: OpticalRepresentation::CueBin,
        files: vec![cue, bin],
        fingerprint,
    })
}

fn inspect_chd(
    path: &Path,
    trusted: &TrustedRoots,
) -> Result<OpticalEquivalentRepresentation, String> {
    let fingerprint = fingerprint_chd(path).map_err(|error| error.to_string())?;
    Ok(OpticalEquivalentRepresentation {
        kind: OpticalRepresentation::Chd,
        files: vec![safe_physical_file(path, trusted)?],
        fingerprint,
    })
}

fn group_for(
    cue_bin: &OpticalEquivalentRepresentation,
    chd: &OpticalEquivalentRepresentation,
) -> Option<OpticalEquivalentGroup> {
    if compare_optical_fingerprints(&cue_bin.fingerprint, &chd.fingerprint)
        != OpticalFingerprintComparison::Equivalent
    {
        return None;
    }
    let preferred = chd.fingerprint.source.clone();
    let quarantine_candidates = cue_bin
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let projected_savings = cue_bin.files.iter().map(|file| file.size_bytes).sum();
    Some(OpticalEquivalentGroup {
        cue_bin: cue_bin.clone(),
        chd: chd.clone(),
        structure: cue_bin.fingerprint.structure.clone(),
        canonical_sha256: cue_bin.fingerprint.canonical_sha256.clone(),
        preferred,
        quarantine_candidates,
        projected_savings,
    })
}

/// Scans only `.cue` and `.chd` files.  Unsupported or malformed candidates
/// are retained as exclusions and never become equivalent groups.
pub fn scan_optical_equivalent_duplicates(
    candidates: &[PathBuf],
    trusted: &TrustedRoots,
    cancel: Option<&AtomicBool>,
) -> OpticalEquivalentScanReport {
    let mut report = OpticalEquivalentScanReport::default();
    let mut cues = Vec::new();
    let mut chds = Vec::new();
    for path in candidates {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            break;
        }
        let Some(kind) = extension(path) else {
            continue;
        };
        let inspected = match kind.as_str() {
            "cue" => inspect_cue(path, trusted),
            "chd" => inspect_chd(path, trusted),
            _ => continue,
        };
        match inspected {
            Ok(value) => {
                report.files_examined += 1;
                if value.kind == OpticalRepresentation::CueBin {
                    cues.push(value);
                } else {
                    chds.push(value);
                }
            }
            Err(reason) => report.excluded.push(OpticalExcludedCandidate {
                path: path.clone(),
                reason,
            }),
        }
    }
    let mut seen = BTreeMap::<String, bool>::new();
    for cue in &cues {
        for chd in &chds {
            if let Some(group) = group_for(cue, chd) {
                let key = format!(
                    "{}:{}",
                    cue.fingerprint.source.display(),
                    chd.fingerprint.source.display()
                );
                if seen.insert(key, true).is_none() {
                    report.groups.push(group);
                }
            }
        }
    }
    report
}

fn quarantine_destination(
    root: &Path,
    group: &OpticalEquivalentGroup,
    source: &Path,
) -> Result<PathBuf, String> {
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "source has no safe basename".to_string())?;
    Ok(root
        .join(".emuwiz-quarantine")
        .join("optical-equivalent")
        .join(&group.canonical_sha256[..group.canonical_sha256.len().min(16)])
        .join(name))
}

fn build_optical_plan(
    group: &OpticalEquivalentGroup,
    root: &Path,
    trusted: &TrustedRoots,
) -> Result<RepairPlan, String> {
    let mut proposals = Vec::new();
    for file in &group.cue_bin.files {
        let live = safe_physical_file(&file.path, trusted)?;
        if live != *file {
            return Err(format!("'{}' changed since scan", file.path.display()));
        }
        let destination = quarantine_destination(root, group, &file.path)?;
        let identity = capture_identity(&file.path).map_err(|error| error.to_string())?;
        let path_crc =
            crate::identity_source::hashing::Crc32::of(file.path.to_string_lossy().as_bytes());
        let id = RepairProposalId::new(format!(
            "optical-equivalent-{}-{}",
            &group.canonical_sha256[..16],
            path_crc
        ))
        .ok_or_else(|| "could not create optical proposal id".to_string())?;
        proposals.push(RepairProposal {
            id,
            action: RepairAction::MovePath { destination },
            source_path: file.path.clone(),
            reason: format!(
                "CUE/BIN representation is equivalent to preferred CHD '{}'",
                group.chd.fingerprint.source.display()
            ),
            evidence: vec![RepairEvidence::new(
                RepairEvidenceKind::DuplicateContent,
                format!(
                    "optical structure matches and canonical SHA-256 is {}",
                    group.canonical_sha256
                ),
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
            verdict_label: Some("Equivalent optical content".to_string()),
            match_confident: true,
            is_outer_archive: false,
            is_outer_archive_verified: false,
            survivor_path: None,
        });
    }
    let id = RepairPlanId::new(format!(
        "optical-equivalent-{}",
        &group.canonical_sha256[..16]
    ))
    .ok_or_else(|| "could not create optical repair plan id".to_string())?;
    Ok(build_repair_plan(
        id,
        0,
        now_unix(),
        Some(OPTICAL_EQUIVALENT_SCAN_VERSION.into()),
        proposals,
    ))
}

pub fn apply_optical_equivalent_group(
    group: &OpticalEquivalentGroup,
    root: &Path,
    trusted: TrustedRoots,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<RepairTransactionResult, RepairExecutionError> {
    let fresh_cue = fingerprint_cue_bin(&group.cue_bin.fingerprint.source).map_err(|_error| {
        RepairExecutionError::StaleSource {
            source: group.cue_bin.fingerprint.source.clone(),
        }
    })?;
    let fresh_chd = fingerprint_chd(&group.chd.fingerprint.source).map_err(|_error| {
        RepairExecutionError::StaleSource {
            source: group.chd.fingerprint.source.clone(),
        }
    })?;
    if compare_optical_fingerprints(&fresh_cue, &fresh_chd)
        != OpticalFingerprintComparison::Equivalent
        || fresh_cue != group.cue_bin.fingerprint
        || fresh_chd != group.chd.fingerprint
    {
        return Err(RepairExecutionError::StaleSource {
            source: group.cue_bin.fingerprint.source.clone(),
        });
    }
    for file in group.cue_bin.files.iter().chain(group.chd.files.iter()) {
        let live = safe_physical_file(&file.path, &trusted).map_err(|_| {
            RepairExecutionError::StaleSource {
                source: file.path.clone(),
            }
        })?;
        if live != *file {
            return Err(RepairExecutionError::StaleSource {
                source: file.path.clone(),
            });
        }
    }
    let destination_parent = root
        .join(".emuwiz-quarantine")
        .join("optical-equivalent")
        .join(&group.canonical_sha256[..group.canonical_sha256.len().min(16)]);
    fs::create_dir_all(&destination_parent).map_err(|error| RepairExecutionError::Build {
        detail: format!("could not prepare quarantine directory: {error}"),
    })?;
    let plan = build_optical_plan(group, root, &trusted)
        .map_err(|detail| RepairExecutionError::Build { detail })?;
    let mut transaction = build_repair_transaction(&plan)?;
    apply_repair_transaction(&mut RepairApplyExecution {
        transaction: &mut transaction,
        current_generation: 0,
        options: &RepairExecutionOptions {
            trusted,
            journal_dir: journal_dir.to_path_buf(),
            audit_cache: crate::dat::sources::audit_cache::AuditCacheConfig::Disabled,
        },
        cancel,
    })
}

pub fn rollback_optical_equivalent_group(
    transaction: &mut crate::dat::rename_apply::model::RenameTransaction,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<crate::dat::rename_apply::model::RollbackResult, String> {
    rollback_repair_transaction(transaction, journal_dir, cancel)
}

#[cfg(test)]
mod tests;
