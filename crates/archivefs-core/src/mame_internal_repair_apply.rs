//! Transactional materialisation of exact bytes found by the MAME internal
//! repair planner.
//!
//! This is deliberately narrower than the planner: only exploded set
//! directories with an exact SHA-1 source are actionable.  Archives, BAD_DUMP,
//! NO_DUMP, ambiguous entries and existing wrong content remain preview-only.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::dat::rename_apply::{
    ApplyExecution, ApplyOutcome, DirectoryPolicy, EntryState, HardConflictMode, RenameTransaction,
    RollbackOutcome, TransactionEntry, TransactionOperation, TransactionState, apply_transaction,
    capture_identity, default_rename_transaction_dir, new_transaction_id,
    rollback_transaction_confined,
};
use crate::mame_internal_repair::{
    MameInternalRepairPlan, MameInternalRepairRequirement, MameRepairDisposition,
    MameRepairRelationshipKind,
};
use crate::safe_read::TrustedRoots;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameRepairApplyStrategy {
    Hardlink,
    Reflink,
    Copy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameRepairOperationState {
    Planned,
    AlreadySatisfied,
    ApplyUnsupported,
    ConflictExistingWrongContent,
    Refused,
    RepairedAndVerified,
    VerificationFailed,
    ApplyRefused,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameInternalRepairOperation {
    pub affected_set: String,
    pub destination_container: PathBuf,
    pub destination_member: String,
    pub destination_path: PathBuf,
    pub expected_sha1: String,
    pub expected_size: Option<u64>,
    pub selected_source: PathBuf,
    pub source_sha1: String,
    pub source_size: u64,
    pub relationship: MameRepairRelationshipKind,
    pub repair_reason: String,
    pub strategy: Option<MameRepairApplyStrategy>,
    pub state: MameRepairOperationState,
    pub refusal_reason: Option<String>,
    #[serde(default)]
    pub post_apply_verification: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameInternalRepairApplyPlan {
    pub plan_digest: String,
    pub collection_root: PathBuf,
    pub operations: Vec<MameInternalRepairOperation>,
    pub refused_count: usize,
    pub unsupported_count: usize,
    pub already_satisfied_count: usize,
    pub transaction: Option<RenameTransaction>,
}

#[derive(Clone, Debug)]
pub struct MameInternalRepairApplyOptions {
    /// Hardlinks are opt-in because later editing a linked ROM can mutate the
    /// source inode. The normal safe default is an independent copy.
    pub allow_hardlinks: bool,
    pub journal_dir: Option<PathBuf>,
}

impl Default for MameInternalRepairApplyOptions {
    fn default() -> Self {
        Self {
            allow_hardlinks: false,
            journal_dir: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MameInternalRepairApplyResult {
    pub outcome: ApplyOutcome,
    pub operations: Vec<MameInternalRepairOperation>,
}

pub fn plan_mame_internal_repair_apply(
    plan: &MameInternalRepairPlan,
    options: &MameInternalRepairApplyOptions,
) -> MameInternalRepairApplyPlan {
    let plan_digest = digest_plan(plan);
    let mut operations = Vec::new();
    for requirement in &plan.requirements {
        if requirement.disposition != MameRepairDisposition::SafeInternalRepair {
            continue;
        }
        operations.push(operation_for_requirement(
            plan,
            requirement,
            options.allow_hardlinks,
        ));
    }
    let mut transaction_entries = Vec::new();
    for operation in &operations {
        if operation.state != MameRepairOperationState::Planned {
            continue;
        }
        let Ok(identity) = capture_identity(&operation.selected_source) else {
            continue;
        };
        transaction_entries.push(TransactionEntry {
            source_path: operation.selected_source.clone(),
            destination_path: operation.destination_path.clone(),
            original_basename: operation
                .selected_source
                .file_name()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default(),
            proposed_basename: operation.destination_member.clone(),
            identity,
            operation: match operation.strategy {
                Some(MameRepairApplyStrategy::Hardlink) => TransactionOperation::CreateHardlink {
                    expected_source: operation.selected_source.clone(),
                    destination_root: plan.collection_root.clone(),
                },
                Some(MameRepairApplyStrategy::Reflink | MameRepairApplyStrategy::Copy) => {
                    TransactionOperation::CreateCopy {
                        expected_source: operation.selected_source.clone(),
                        destination_root: plan.collection_root.clone(),
                    }
                }
                None => continue,
            },
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        });
    }
    let transaction = if transaction_entries.is_empty() {
        None
    } else {
        let mut tx = RenameTransaction {
            transaction_id: new_transaction_id(crate::dat::sources::now_unix()),
            plan_generation: digest_generation(&plan_digest),
            classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.into()),
            created_at_unix: crate::dat::sources::now_unix(),
            source_scan_root: plan.collection_root.to_string_lossy().into_owned(),
            state: TransactionState::Planned,
            entries: transaction_entries,
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        };
        tx.unknown.insert(
            "mame_internal_repair_plan_digest".into(),
            serde_json::json!(plan_digest),
        );
        tx.unknown.insert(
            "mame_catalogue_version".into(),
            serde_json::json!(plan.catalogue_version),
        );
        Some(tx)
    };
    let refused_count = operations
        .iter()
        .filter(|item| {
            matches!(
                item.state,
                MameRepairOperationState::Refused
                    | MameRepairOperationState::ConflictExistingWrongContent
            )
        })
        .count();
    let unsupported_count = operations
        .iter()
        .filter(|item| item.state == MameRepairOperationState::ApplyUnsupported)
        .count();
    let already_satisfied_count = operations
        .iter()
        .filter(|item| item.state == MameRepairOperationState::AlreadySatisfied)
        .count();
    MameInternalRepairApplyPlan {
        plan_digest,
        collection_root: plan.collection_root.clone(),
        operations,
        refused_count,
        unsupported_count,
        already_satisfied_count,
        transaction,
    }
}

pub fn apply_mame_internal_repair_plan(
    apply_plan: &mut MameInternalRepairApplyPlan,
    options: &MameInternalRepairApplyOptions,
    cancel: &AtomicBool,
) -> Result<MameInternalRepairApplyResult, String> {
    let Some(transaction) = apply_plan.transaction.as_mut() else {
        return Err("no safe internal repair operations are applicable".into());
    };
    let journal_dir = match &options.journal_dir {
        Some(path) => path.clone(),
        None => default_rename_transaction_dir().map_err(|error| error.to_string())?,
    };
    let approved_paths: BTreeSet<String> = transaction
        .entries
        .iter()
        .map(|entry| entry.source_path.to_string_lossy().into_owned())
        .collect();
    let current_generation = transaction.plan_generation;
    let trusted = TrustedRoots::from_paths([&apply_plan.collection_root]);
    let mut execution = ApplyExecution {
        transaction,
        approved_paths,
        current_generation,
        trusted,
        journal_dir,
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel,
        directory_policy: DirectoryPolicy::SameFilesystem,
        allow_symlink_source: false,
    };
    let mut outcome = apply_transaction(&mut execution).map_err(|error| error.to_string())?;
    if outcome.transaction.state == TransactionState::ApplyFailed {
        let rollback = rollback_transaction_confined(
            execution.transaction,
            &execution.journal_dir,
            &AtomicBool::new(false),
            &execution.trusted,
        )?;
        if !matches!(
            rollback.result,
            crate::dat::rename_apply::RollbackResult::FullyRolledBack
        ) {
            return Err("partial repair failed and could not be fully rolled back".into());
        }
        outcome.transaction = rollback.transaction;
    }
    for operation in &mut apply_plan.operations {
        if let Some(entry) = outcome
            .transaction
            .entries
            .iter()
            .find(|entry| entry.destination_path == operation.destination_path)
        {
            operation.state = match entry.state {
                EntryState::Applied => MameRepairOperationState::RepairedAndVerified,
                EntryState::Skipped | EntryState::ApplyFailed => {
                    MameRepairOperationState::ApplyRefused
                }
                _ => operation.state,
            };
            if entry.state == EntryState::Applied {
                operation.post_apply_verification = Some("destination hash verified; targeted MAME verification was not run by this filesystem transaction".into());
            } else if entry.state == EntryState::ApplyFailed {
                operation.post_apply_verification =
                    Some("apply failed; transaction rollback was attempted".into());
            }
        }
    }
    Ok(MameInternalRepairApplyResult {
        outcome,
        operations: apply_plan.operations.clone(),
    })
}

pub fn rollback_mame_internal_repair(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
) -> Result<RollbackOutcome, String> {
    let trusted = TrustedRoots::from_paths([Path::new(&transaction.source_scan_root)]);
    rollback_transaction_confined(transaction, journal_dir, &AtomicBool::new(false), &trusted)
}

fn operation_for_requirement(
    plan: &MameInternalRepairPlan,
    requirement: &MameInternalRepairRequirement,
    allow_hardlinks: bool,
) -> MameInternalRepairOperation {
    let expected_sha1 = requirement.sha1.clone().unwrap_or_default();
    let source = requirement.exact_matching_sources.first();
    let (source_path, source_sha1, source_size) = source
        .map(|source| {
            (
                source.container.join(&source.member),
                source.sha1.clone(),
                source.size_bytes.unwrap_or_default(),
            )
        })
        .unwrap_or_else(|| (PathBuf::new(), String::new(), 0));
    let destination_container = requirement
        .expected_destination_container
        .clone()
        .unwrap_or_default();
    let destination_path = destination_container.join(&requirement.expected_destination_member);
    let mut operation = MameInternalRepairOperation {
        affected_set: requirement.affected_set.clone(),
        destination_container,
        destination_member: requirement.expected_destination_member.clone(),
        destination_path,
        expected_sha1: expected_sha1.clone(),
        expected_size: requirement.required_size,
        selected_source: source_path,
        source_sha1,
        source_size,
        relationship: requirement.relationship.kind,
        repair_reason: requirement.proposed_operation.clone(),
        strategy: None,
        state: MameRepairOperationState::Refused,
        refusal_reason: None,
        post_apply_verification: None,
    };
    if source.is_none()
        || !source.is_some_and(|source| source.container_is_directory)
        || !requirement.expected_destination_is_directory
    {
        operation.state = MameRepairOperationState::ApplyUnsupported;
        operation.refusal_reason = Some(
            "archive/member destination is not supported by the safe exploded-directory apply path"
                .into(),
        );
        return operation;
    }
    if operation.selected_source.as_os_str().is_empty() || operation.expected_sha1.is_empty() {
        operation.refusal_reason = Some("exact source SHA-1 evidence is incomplete".into());
        return operation;
    }
    if !within_root(&plan.collection_root, &operation.selected_source)
        || !within_root(&plan.collection_root, &operation.destination_path)
    {
        operation.refusal_reason =
            Some("source or destination escapes the configured arcade root".into());
        return operation;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&operation.destination_path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            operation.refusal_reason = Some("destination is a symlink or special file".into());
        } else if hash_sha1(&operation.destination_path).ok().as_deref()
            == Some(expected_sha1.as_str())
        {
            operation.state = MameRepairOperationState::AlreadySatisfied;
        } else {
            operation.state = MameRepairOperationState::ConflictExistingWrongContent;
            operation.refusal_reason = Some(
                "destination already exists with different content; automatic overwrite is refused"
                    .into(),
            );
        }
        return operation;
    }
    if !std::fs::symlink_metadata(&operation.selected_source)
        .is_ok_and(|metadata| metadata.is_file())
    {
        operation.refusal_reason = Some("selected source is missing or not a regular file".into());
        return operation;
    }
    if hash_sha1(&operation.selected_source).ok().as_deref() != Some(expected_sha1.as_str()) {
        operation.refusal_reason =
            Some("selected source no longer matches the planner SHA-1".into());
        return operation;
    }
    if let Some(expected_size) = operation.expected_size
        && operation.source_size != expected_size
    {
        operation.refusal_reason =
            Some("selected source size does not match the catalogue expectation".into());
        return operation;
    }
    operation.strategy = Some(if allow_hardlinks {
        MameRepairApplyStrategy::Hardlink
    } else {
        MameRepairApplyStrategy::Copy
    });
    operation.state = MameRepairOperationState::Planned;
    operation
}

fn within_root(root: &Path, path: &Path) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    let candidate = path
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok());
    candidate.is_some_and(|candidate| candidate.starts_with(&root))
}

fn hash_sha1(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(encode_hex(&hasher.finalize()))
}

fn digest_plan(plan: &MameInternalRepairPlan) -> String {
    let bytes = serde_json::to_vec(plan).unwrap_or_default();
    encode_hex(&sha2::Sha256::digest(bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn digest_generation(digest: &str) -> u64 {
    u64::from_str_radix(&digest[..16], 16).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn same_hash_different_filename_plans_copy_without_mutating_source() {
        let root = tempdir().unwrap();
        let source_set = root.path().join("source");
        let dest_set = root.path().join("dest");
        std::fs::create_dir_all(&source_set).unwrap();
        std::fs::create_dir_all(&dest_set).unwrap();
        std::fs::write(source_set.join("elsewhere.bin"), b"exact bytes").unwrap();
        let expected = hash_sha1(&source_set.join("elsewhere.bin")).unwrap();
        let requirement = MameInternalRepairRequirement {
            affected_set: "dest".into(),
            required_filename: "required.bin".into(),
            required_size: Some(11),
            crc: None,
            sha1: Some(expected.clone()),
            requirement_class:
                crate::mame_internal_repair::MameRepairRequirementClass::GameSpecificRom,
            relationship: crate::mame_internal_repair::MameRepairRelationship {
                kind: MameRepairRelationshipKind::GameSpecific,
                set: "dest".into(),
                parent: None,
                merge_member: None,
                device_refs: vec![],
            },
            expected_destination_container: Some(dest_set.clone()),
            expected_destination_is_directory: true,
            expected_destination_member: "required.bin".into(),
            exact_matching_sources: vec![crate::mame_internal_repair::MameRepairSource {
                container: source_set.clone(),
                member: "elsewhere.bin".into(),
                container_is_directory: true,
                sha1: expected.clone(),
                size_bytes: Some(11),
            }],
            source_sha1_verified: true,
            ambiguity: None,
            preservation_status: "Verified dump expected".into(),
            proposed_operation: "copy exact bytes".into(),
            repair_confidence:
                crate::mame_internal_repair::MameRepairConfidence::ExactSha1Deterministic,
            disposition: MameRepairDisposition::SafeInternalRepair,
            refusal_reason: None,
        };
        let plan = MameInternalRepairPlan {
            schema_version: 1,
            collection_root: root.path().to_path_buf(),
            catalogue_version: Some("0.264".into()),
            sets_currently_failing: 1,
            affected_sets: vec!["dest".into()],
            requirements: vec![requirement],
            safe_internal_repair_count: 1,
            no_download_needed_count: 1,
            genuinely_absent_count: 0,
            preservation_only_no_dump_count: 0,
            bad_dump_count: 0,
            ambiguous_count: 0,
            wrong_content_same_name_count: 0,
            unique_source_identities_needed: 1,
            filesystem_operations_required: 1,
            projected_sets_repairable: 1,
            top_repairs_by_impact: vec![],
            warnings: vec![],
        };
        let mut apply_plan = plan_mame_internal_repair_apply(&plan, &Default::default());
        assert_eq!(
            apply_plan.operations[0].state,
            MameRepairOperationState::Planned
        );
        let before = std::fs::read(source_set.join("elsewhere.bin")).unwrap();
        let journal = root.path().join("journal");
        let result = apply_mame_internal_repair_plan(
            &mut apply_plan,
            &MameInternalRepairApplyOptions {
                journal_dir: Some(journal.clone()),
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.outcome.transaction.entries.len(), 1);
        assert_eq!(
            std::fs::read(dest_set.join("required.bin")).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read(source_set.join("elsewhere.bin")).unwrap(),
            before
        );
        let rollback =
            rollback_mame_internal_repair(&mut result.outcome.transaction.clone(), &journal)
                .unwrap();
        assert!(matches!(
            rollback.result,
            crate::dat::rename_apply::RollbackResult::FullyRolledBack
        ));
        assert!(!dest_set.join("required.bin").exists());
    }
}
