//! Equivalent-content review for Nintendo 64 byte-order representations.
//!
//! This is deliberately not a fuzzy or filename-based duplicate detector.
//! A member is eligible only when its bytes carry one of the three known N64
//! byte-order headers and the existing [`crate::n64_byte_order`] transform
//! produces a canonical Z64 image.  Physical SHA-256 and canonical SHA-256
//! are both retained so this review remains distinct from exact duplicates.
//!
//! Quarantine uses the shared Repair transaction engine.  The only extra
//! safety check here is the canonical revalidation immediately before the
//! transaction is built; the shared engine then provides no-clobber moves,
//! durable journals, rollback, and recovery classification.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::sources::now_unix;
use crate::n64_byte_order::{N64ByteOrder, detect_n64_byte_order, normalize_to_z64};
use crate::repair::execute::{
    RepairApplyExecution, RepairExecutionError, RepairExecutionOptions, RepairTransactionResult,
    apply_repair_transaction, build_repair_transaction, rollback_repair_transaction,
};
use crate::repair::plan::{RepairPlan, RepairPlanId, build_repair_plan};
use crate::repair::proposal::{
    RepairAction, RepairEvidence, RepairEvidenceKind, RepairProposal, RepairProposalId, SafetyState,
};
use crate::safe_read::{TrustedRoots, open_bounded_read};

use super::exact_duplicate::hash_full_file_sha256;

/// Version of the N64 equivalent-content classifier.
pub const N64_EQUIVALENT_SCAN_VERSION: &str = "n64-equivalent-content-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct N64EquivalentMember {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub physical_sha256: String,
    pub canonical_sha256: String,
    pub byte_order: N64ByteOrder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct N64EquivalentGroup {
    pub canonical_sha256: String,
    pub members: Vec<N64EquivalentMember>,
    pub preferred: PathBuf,
    pub quarantine_candidates: Vec<PathBuf>,
    pub projected_savings: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct N64ExcludedCandidate {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct N64EquivalentScanReport {
    pub groups: Vec<N64EquivalentGroup>,
    pub excluded: Vec<N64ExcludedCandidate>,
    pub files_examined: usize,
}

impl N64EquivalentScanReport {
    pub fn projected_savings(&self) -> u64 {
        self.groups
            .iter()
            .map(|group| group.projected_savings)
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InspectRefusal {
    UnsupportedExtension,
    NotRegular,
    Unreadable(String),
    UnknownMagic,
    InvalidLength(String),
}

impl std::fmt::Display for InspectRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedExtension => write!(f, "not a .z64, .v64, or .n64 candidate"),
            Self::NotRegular => write!(f, "not a regular file"),
            Self::Unreadable(detail) => write!(f, "could not be read: {detail}"),
            Self::UnknownMagic => write!(f, "unrecognized N64 byte-order magic"),
            Self::InvalidLength(detail) => write!(f, "invalid N64 length: {detail}"),
        }
    }
}

fn supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "z64" | "v64" | "n64"
            )
        })
}

fn read_and_normalize(
    path: &Path,
    trusted: &TrustedRoots,
) -> Result<N64EquivalentMember, InspectRefusal> {
    let metadata = fs::symlink_metadata(path).map_err(|_| InspectRefusal::NotRegular)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(InspectRefusal::NotRegular);
    }
    let physical = hash_full_file_sha256(path, trusted, None)
        .map_err(|error| InspectRefusal::Unreadable(error.to_string()))?;
    let file = open_bounded_read(path, trusted)
        .map_err(|error| InspectRefusal::Unreadable(error.detail()))?;
    let mut reader = file.into_file();
    let mut bytes = Vec::with_capacity(physical.size_bytes.min(8 * 1024 * 1024) as usize);
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| InspectRefusal::Unreadable(error.to_string()))?;
    if bytes.len() as u64 != physical.size_bytes {
        return Err(InspectRefusal::Unreadable(
            "file changed while the canonical view was being read".to_string(),
        ));
    }
    let after = hash_full_file_sha256(path, trusted, None)
        .map_err(|error| InspectRefusal::Unreadable(error.to_string()))?;
    if after != physical {
        return Err(InspectRefusal::Unreadable(
            "file content changed while the canonical view was being read".to_string(),
        ));
    }
    let order = detect_n64_byte_order(&bytes).ok_or(InspectRefusal::UnknownMagic)?;
    let normalized = normalize_to_z64(&bytes, order)
        .map_err(|error| InspectRefusal::InvalidLength(error.detail()))?;
    let canonical_sha256 = Sha256::digest(&normalized.bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(N64EquivalentMember {
        path: path.to_path_buf(),
        size_bytes: physical.size_bytes,
        physical_sha256: physical.sha256,
        canonical_sha256,
        byte_order: order,
    })
}

fn preference(path: &Path) -> u8 {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("z64") => 0,
        Some("v64") => 1,
        Some("n64") => 2,
        _ => 3,
    }
}

/// Scans candidates read-only. Non-N64 and malformed candidates are retained
/// in `excluded`, never silently discarded. Exact physical duplicates are not
/// repeated in an equivalent group; Exact Duplicate Review remains their
/// authoritative cleanup path.
pub fn scan_n64_equivalent_duplicates(
    candidates: &[PathBuf],
    trusted: &TrustedRoots,
    cancel: Option<&AtomicBool>,
) -> N64EquivalentScanReport {
    let mut report = N64EquivalentScanReport::default();
    let mut by_canonical: BTreeMap<String, Vec<N64EquivalentMember>> = BTreeMap::new();
    for path in candidates {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            break;
        }
        if !supported_extension(path) {
            report.excluded.push(N64ExcludedCandidate {
                path: path.clone(),
                reason: InspectRefusal::UnsupportedExtension.to_string(),
            });
            continue;
        }
        match read_and_normalize(path, trusted) {
            Ok(member) => {
                report.files_examined += 1;
                let members = by_canonical
                    .entry(member.canonical_sha256.clone())
                    .or_default();
                if members
                    .iter()
                    .any(|existing| existing.physical_sha256 == member.physical_sha256)
                {
                    report.excluded.push(N64ExcludedCandidate {
                        path: member.path,
                        reason: "exact physical duplicate is left to Exact Duplicate Review"
                            .to_string(),
                    });
                } else {
                    members.push(member);
                }
            }
            Err(error) => report.excluded.push(N64ExcludedCandidate {
                path: path.clone(),
                reason: error.to_string(),
            }),
        }
    }
    for (canonical_sha256, mut members) in by_canonical {
        if members.len() < 2 {
            continue;
        }
        members.sort_by(|a, b| {
            preference(&a.path)
                .cmp(&preference(&b.path))
                .then_with(|| a.path.cmp(&b.path))
        });
        let preferred = members[0].path.clone();
        let quarantine_candidates = members
            .iter()
            .skip(1)
            .map(|member| member.path.clone())
            .collect::<Vec<_>>();
        let projected_savings = members.iter().skip(1).map(|member| member.size_bytes).sum();
        report.groups.push(N64EquivalentGroup {
            canonical_sha256,
            members,
            preferred,
            quarantine_candidates,
            projected_savings,
        });
    }
    report
}

fn quarantine_destination(
    trusted_root: &Path,
    group: &N64EquivalentGroup,
    source: &Path,
) -> Result<PathBuf, String> {
    let basename = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "source has no safe basename".to_string())?;
    if basename.is_empty()
        || basename == "."
        || basename == ".."
        || basename.contains(['/', '\\', '\0'])
    {
        return Err("source basename is unsafe".to_string());
    }
    Ok(trusted_root
        .join(".emuwiz-quarantine")
        .join("n64-equivalent")
        .join(&group.canonical_sha256[..group.canonical_sha256.len().min(16)])
        .join(basename))
}

fn equivalent_proposals(
    group: &N64EquivalentGroup,
    trusted_root: &Path,
    trusted: &TrustedRoots,
) -> Result<Vec<RepairProposal>, String> {
    let mut proposals = Vec::new();
    for source in &group.quarantine_candidates {
        let live = read_and_normalize(source, trusted)
            .map_err(|error| format!("'{}': {error}", source.display()))?;
        if live.canonical_sha256 != group.canonical_sha256 {
            return Err(format!(
                "'{}' no longer has the group's canonical hash",
                source.display()
            ));
        }
        let destination = quarantine_destination(trusted_root, group, source)?;
        let identity = capture_identity(source)
            .map_err(|error| format!("could not capture '{}': {error}", source.display()))?;
        let path_crc =
            crate::identity_source::hashing::Crc32::of(source.to_string_lossy().as_bytes());
        let id = RepairProposalId::new(format!(
            "n64-equivalent-{}-{}",
            &group.canonical_sha256[..group.canonical_sha256.len().min(16)],
            path_crc
        ))
        .ok_or_else(|| "could not create proposal id".to_string())?;
        proposals.push(RepairProposal {
            id,
            action: RepairAction::MovePath { destination },
            source_path: source.clone(),
            reason: format!(
                "'{}' is a {} representation of canonical N64 content; keep preferred '{}'",
                source.display(),
                live.byte_order.label(),
                group.preferred.display()
            ),
            evidence: vec![RepairEvidence::new(
                RepairEvidenceKind::DuplicateContent,
                format!(
                    "physical SHA-256 {} differs while canonical N64 SHA-256 is {}",
                    live.physical_sha256, group.canonical_sha256
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
            verdict_label: Some("Equivalent N64 content".to_string()),
            match_confident: true,
            is_outer_archive: false,
            is_outer_archive_verified: false,
            survivor_path: None,
        });
    }
    Ok(proposals)
}

/// Builds a normal Repair plan for the group's redundant representations.
/// This is read-only; the quarantine directory is created only by apply.
pub fn build_n64_equivalent_repair_plan(
    group: &N64EquivalentGroup,
    trusted_root: &Path,
    trusted: &TrustedRoots,
) -> Result<RepairPlan, String> {
    let proposals = equivalent_proposals(group, trusted_root, trusted)?;
    let id = RepairPlanId::new(format!(
        "n64-equivalent-{}",
        &group.canonical_sha256[..group.canonical_sha256.len().min(16)]
    ))
    .ok_or_else(|| "could not create repair plan id".to_string())?;
    Ok(build_repair_plan(
        id,
        0,
        now_unix(),
        Some(N64_EQUIVALENT_SCAN_VERSION.to_string()),
        proposals,
    ))
}

/// Applies one approved group through the shared journaled Repair engine.
/// Canonical hashes and regular-file identity are re-read before any
/// quarantine directory is created or source is moved.
pub fn apply_n64_equivalent_group(
    group: &N64EquivalentGroup,
    trusted_root: &Path,
    trusted: TrustedRoots,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<RepairTransactionResult, RepairExecutionError> {
    let preferred = read_and_normalize(&group.preferred, &trusted).map_err(|error| {
        RepairExecutionError::Build {
            detail: format!("preferred copy refused: {error}"),
        }
    })?;
    if preferred.canonical_sha256 != group.canonical_sha256 {
        return Err(RepairExecutionError::Build {
            detail: "preferred copy no longer matches canonical group".to_string(),
        });
    }
    for path in &group.quarantine_candidates {
        let member = read_and_normalize(path, &trusted).map_err(|_error| {
            RepairExecutionError::StaleSource {
                source: path.clone(),
            }
        })?;
        if member.canonical_sha256 != group.canonical_sha256 {
            return Err(RepairExecutionError::StaleSource {
                source: path.clone(),
            });
        }
    }
    let destination_parent = trusted_root
        .join(".emuwiz-quarantine")
        .join("n64-equivalent")
        .join(&group.canonical_sha256[..group.canonical_sha256.len().min(16)]);
    fs::create_dir_all(&destination_parent).map_err(|error| RepairExecutionError::Build {
        detail: format!("could not prepare quarantine directory: {error}"),
    })?;
    let plan = build_n64_equivalent_repair_plan(group, trusted_root, &trusted)
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

pub fn rollback_n64_equivalent_group(
    transaction: &mut crate::dat::rename_apply::model::RenameTransaction,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<crate::dat::rename_apply::model::RollbackResult, String> {
    rollback_repair_transaction(transaction, journal_dir, cancel)
}

#[cfg(test)]
mod tests;
