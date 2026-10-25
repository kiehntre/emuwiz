//! Transactional HackHash patch publication.
//!
//! Patch bytes are prepared in memory by the already-bounded standalone
//! applier, then handed to the shared transaction/history executor. The base
//! and patch are never modified and the shared executor is the only publisher.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use md5::Md5;
use sha1::{Digest, Sha1};

use super::hackhash::{
    HackHashNativeInspection, HackHashPatchProvenance, HackHashVerificationState,
};
use super::hackhash_identity::HackHashOutputHashes;
use super::hackhash_readiness::{HackHashPatchReadinessEvidence, HackHashPatchReadinessStatus};
use crate::game_identity::{IdentityStatus, inspect_catalogued_game_identity};
use crate::identity_source::hashing::Crc32;
use crate::identity_source::hashing::hash_file;
use crate::patch_manager::{
    PreviewAdapter, PreviewDestinationState, PreviewProposedAction, SHARED_APPLY_SCHEMA_VERSION,
    SharedApplyConfirmation, SharedApplyContext, SharedApplyOptions, SharedApplyResult,
    SharedContentVerification, SharedMaterializedOutput, SharedPlanEntry,
    SharedRollbackConfirmation, SharedRollbackOptions, SharedRollbackResult, SharedTransactionPath,
    SharedTransactionPlan, execute_shared_materialized_apply, execute_shared_rollback,
    generate_shared_operation_id, preview_shared_rollback, seal_shared_transaction_plan,
};
use crate::safe_read::TrustedRoots;
use crate::standalone_patch::{StandalonePatchApplyPlan, prepare_standalone_patch_output};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackHashPatchApplyPlan {
    pub standalone: StandalonePatchApplyPlan,
    pub shared: SharedTransactionPlan,
    pub destination_path: PathBuf,
    pub expected_output: HackHashOutputHashes,
    pub readiness_status: HackHashPatchReadinessStatus,
    pub provider_snapshot_sha256: String,
    pub base_identity: Option<String>,
    pub hack_titles: Vec<String>,
    pub family_versions: Vec<String>,
    pub provider_provenance: String,
}

#[derive(Debug)]
pub struct HackHashPatchApplyResult {
    pub shared: SharedApplyResult,
    pub produced_output: HackHashOutputHashes,
    pub tool: String,
    pub provenance: HackHashPatchProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HackHashPatchApplyError {
    NotReady(String),
    Stale(String),
    Unsafe(String),
    Patch(String),
    OutputMismatch(String),
    Transaction(String),
}

impl std::fmt::Display for HackHashPatchApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady(value) => write!(f, "HackHash apply is not ready: {value}"),
            Self::Stale(value) => write!(f, "HackHash apply plan is stale: {value}"),
            Self::Unsafe(value) => write!(f, "unsafe HackHash apply: {value}"),
            Self::Patch(value) => write!(f, "HackHash patch preparation failed: {value}"),
            Self::OutputMismatch(value) => write!(f, "HackHash output mismatch: {value}"),
            Self::Transaction(value) => write!(f, "HackHash transaction failed: {value}"),
        }
    }
}

impl std::error::Error for HackHashPatchApplyError {}

pub fn build_hackhash_patch_apply_plan(
    readiness: &HackHashPatchReadinessEvidence,
    standalone: StandalonePatchApplyPlan,
    destination_root: impl AsRef<Path>,
    destination_path: impl AsRef<Path>,
    provider_snapshot_sha256: &str,
) -> Result<HackHashPatchApplyPlan, HackHashPatchApplyError> {
    if readiness.status != HackHashPatchReadinessStatus::ReadyToPatch {
        return Err(HackHashPatchApplyError::NotReady(format!(
            "status is {:?}",
            readiness.status
        )));
    }
    let expected_output = readiness.expected_output.clone().ok_or_else(|| {
        HackHashPatchApplyError::NotReady("expected output hashes are missing".into())
    })?;
    for source in [
        standalone.reviewed.base_path.as_path(),
        standalone.reviewed.patch_path.as_path(),
    ] {
        let metadata = fs::symlink_metadata(source).map_err(|error| {
            HackHashPatchApplyError::Unsafe(format!("{}: {error}", source.display()))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(HackHashPatchApplyError::Unsafe(format!(
                "source is not a regular non-symlink file: {}",
                source.display()
            )));
        }
    }
    let root = destination_root.as_ref().to_path_buf();
    let destination = destination_path.as_ref().to_path_buf();
    validate_root_and_file(&root, &destination)?;
    let relative = destination
        .strip_prefix(&root)
        .map_err(|_| HackHashPatchApplyError::Unsafe("destination escapes managed root".into()))?;
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::RootDir
            )
        })
    {
        return Err(HackHashPatchApplyError::Unsafe(
            "destination has unsafe relative components".into(),
        ));
    }
    let base = &standalone.reviewed.base_path;
    let source_root = base
        .parent()
        .ok_or_else(|| HackHashPatchApplyError::Unsafe("base ROM has no parent".into()))?;
    let identity = format!(
        "hackhash:{}:{}",
        provider_snapshot_sha256, expected_output.sha1
    );
    let shared_context = SharedApplyContext {
        adapter: PreviewAdapter::LocalModPackage,
        selected_archive: SharedTransactionPath::from_path(base),
        verified_game_identity: identity,
        profile_id: "hackhash-phase5".into(),
        source_mode: "hackhash_patch".into(),
    };
    let mut shared = SharedTransactionPlan {
        schema_version: SHARED_APPLY_SCHEMA_VERSION,
        plan_id: String::new(),
        context: shared_context,
        approved_source_root: SharedTransactionPath::from_path(source_root),
        destination_root: SharedTransactionPath::from_path(&root),
        entries: vec![SharedPlanEntry {
            adapter: PreviewAdapter::LocalModPackage,
            selected_archive: SharedTransactionPath::from_path(base),
            verified_game_identity: format!("hackhash:{}", expected_output.sha1),
            source_path: SharedTransactionPath::from_path(base),
            source_digest: standalone.reviewed.base_sha256.clone().ok_or_else(|| {
                HackHashPatchApplyError::NotReady("standalone plan has no base hash".into())
            })?,
            destination_root: SharedTransactionPath::from_path(&root),
            destination_relative_path: SharedTransactionPath::from_path(relative),
            destination_pre_state: PreviewDestinationState::Missing,
            destination_pre_digest: None,
            proposed_action: PreviewProposedAction::Install,
            backup_required: false,
            parent_creation_approved: true,
            content_verification: Some(SharedContentVerification::HackHashPatch {
                base_hash: standalone.reviewed.base_sha256.clone().unwrap_or_default(),
                patch_hash: standalone.reviewed.patch_sha256.clone(),
                expected_output_sha1: expected_output.sha1.clone(),
                expected_output_md5: expected_output.md5.clone(),
                expected_output_crc32: expected_output.crc32.clone(),
                expected_output_hash: String::new(),
                provider_snapshot_hash: provider_snapshot_sha256.into(),
                tool: "pending".into(),
                provenance: None,
            }),
        }],
    };
    // The output hash and tool are known only after bounded preparation. The
    // transaction plan is sealed after preparation by apply_hackhash_patch.
    seal_shared_transaction_plan(&mut shared)
        .map_err(|error| HackHashPatchApplyError::Transaction(error.detail))?;
    Ok(HackHashPatchApplyPlan {
        standalone,
        shared,
        destination_path: destination,
        expected_output,
        readiness_status: readiness.status,
        provider_snapshot_sha256: provider_snapshot_sha256.into(),
        base_identity: readiness.expected_base_identity.clone(),
        hack_titles: readiness.hack_titles.clone(),
        family_versions: readiness.family_versions.clone(),
        provider_provenance: readiness.provider_provenance.clone(),
    })
}

pub fn apply_hackhash_patch(
    plan: &HackHashPatchApplyPlan,
    current_snapshot_sha256: &str,
    current_expected_output: &HackHashOutputHashes,
    confirmation_phrase: &str,
    history_root: impl AsRef<Path>,
    backup_root: impl AsRef<Path>,
) -> Result<HackHashPatchApplyResult, HackHashPatchApplyError> {
    if plan.readiness_status != HackHashPatchReadinessStatus::ReadyToPatch {
        return Err(HackHashPatchApplyError::NotReady(format!(
            "status is {:?}",
            plan.readiness_status
        )));
    }
    if current_snapshot_sha256 != plan.provider_snapshot_sha256 {
        return Err(HackHashPatchApplyError::Stale(
            "active HackHash snapshot changed".into(),
        ));
    }
    if current_expected_output != &plan.expected_output {
        return Err(HackHashPatchApplyError::Stale(
            "expected output evidence changed".into(),
        ));
    }
    if confirmation_phrase != "APPLY HACKHASH PATCH" {
        return Err(HackHashPatchApplyError::NotReady(
            "confirmation phrase was not accepted".into(),
        ));
    }
    if fs::symlink_metadata(&plan.destination_path).is_ok() {
        return Err(HackHashPatchApplyError::Unsafe(
            "destination collision; replacement is not enabled".into(),
        ));
    }
    for source in [
        plan.standalone.reviewed.base_path.as_path(),
        plan.standalone.reviewed.patch_path.as_path(),
    ] {
        let metadata = fs::symlink_metadata(source).map_err(|error| {
            HackHashPatchApplyError::Unsafe(format!("{}: {error}", source.display()))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(HackHashPatchApplyError::Unsafe(format!(
                "source is not a regular non-symlink file: {}",
                source.display()
            )));
        }
    }
    let prepared = prepare_standalone_patch_output(&plan.standalone)
        .map_err(|error| HackHashPatchApplyError::Patch(error.to_string()))?;
    let produced = hashes(&prepared.bytes);
    if produced != plan.expected_output {
        return Err(HackHashPatchApplyError::OutputMismatch(format!(
            "expected SHA-1 {}, produced SHA-1 {}",
            plan.expected_output.sha1, produced.sha1
        )));
    }
    let output_sha256 = sha256(&prepared.bytes);
    let native = inspect_prepared_output(&prepared.bytes, &plan.destination_path)?;
    let operation_id = generate_shared_operation_id();
    let timestamp_unix_seconds = now();
    let native_state = if native.complete && !native.evidence.is_empty() {
        HackHashVerificationState::ProviderOutputAndNativeInspection
    } else if native
        .evidence
        .iter()
        .any(|evidence| evidence.contains(":Ambiguous:") || evidence.contains(":Invalid:"))
    {
        HackHashVerificationState::ProviderOutputNativeIdentityConflict
    } else {
        HackHashVerificationState::ProviderOutputNativeIdentityUnavailable
    };
    let mut provenance = HackHashPatchProvenance {
        base_path: plan.standalone.reviewed.base_path.clone(),
        base_identity: plan.base_identity.clone(),
        base_sha256: plan
            .standalone
            .reviewed
            .base_sha256
            .clone()
            .unwrap_or_default(),
        patch_path: plan.standalone.reviewed.patch_path.clone(),
        patch_sha256: plan.standalone.reviewed.patch_sha256.clone(),
        patch_format: plan.standalone.reviewed.patch_format,
        patch_source: "local HackHash patch selected by the user".into(),
        expected_output: plan.expected_output.clone(),
        actual_output: produced.clone(),
        output_path: plan.destination_path.clone(),
        output_sha256: output_sha256.clone(),
        hack_titles: plan.hack_titles.clone(),
        family_versions: plan.family_versions.clone(),
        provider_snapshot_sha256: plan.provider_snapshot_sha256.clone(),
        provider_provenance: plan.provider_provenance.clone(),
        transaction_id: operation_id.clone(),
        timestamp_unix_seconds,
        verification_state: native_state,
        native_inspection: native,
    };
    let mut shared = plan.shared.clone();
    let entry = shared
        .entries
        .first_mut()
        .ok_or_else(|| HackHashPatchApplyError::Transaction("shared plan has no entry".into()))?;
    entry.content_verification = Some(SharedContentVerification::HackHashPatch {
        base_hash: plan
            .standalone
            .reviewed
            .base_sha256
            .clone()
            .unwrap_or_default(),
        patch_hash: plan.standalone.reviewed.patch_sha256.clone(),
        expected_output_sha1: plan.expected_output.sha1.clone(),
        expected_output_md5: plan.expected_output.md5.clone(),
        expected_output_crc32: plan.expected_output.crc32.clone(),
        expected_output_hash: output_sha256.clone(),
        provider_snapshot_hash: plan.provider_snapshot_sha256.clone(),
        tool: prepared.application.clone(),
        provenance: Some(provenance.clone()),
    });
    seal_shared_transaction_plan(&mut shared)
        .map_err(|error| HackHashPatchApplyError::Transaction(error.detail))?;
    let options = SharedApplyOptions {
        dry_run: false,
        confirmation: Some(SharedApplyConfirmation {
            plan_id: shared.plan_id.clone(),
            general_approved: true,
            replacement_approved: false,
        }),
        operation_id,
        timestamp_unix_seconds,
        current_context: shared.context.clone(),
        history_root: history_root.as_ref().to_path_buf(),
        backup_root: backup_root.as_ref().to_path_buf(),
    };
    let materialized = SharedMaterializedOutput {
        source_path: plan.standalone.reviewed.base_path.clone(),
        source_digest: plan
            .standalone
            .reviewed
            .base_sha256
            .clone()
            .unwrap_or_default(),
        output_digest: output_sha256,
        bytes: prepared.bytes,
        guard_paths: vec![(
            plan.standalone.reviewed.patch_path.clone(),
            plan.standalone.reviewed.patch_sha256.clone(),
        )],
    };
    let shared_result = execute_shared_materialized_apply(&shared, &options, &materialized);
    if shared_result.journal.status == crate::patch_manager::SharedApplyStatus::Success {
        match inspect_native_path(&plan.destination_path) {
            Ok(native) => provenance.native_inspection = native,
            Err(error) => {
                provenance.verification_state =
                    HackHashVerificationState::ProviderOutputNativeIdentityUnavailable;
                provenance.native_inspection.warnings.push(format!(
                    "post-publication native inspection unavailable: {error}"
                ));
            }
        }
    }
    Ok(HackHashPatchApplyResult {
        shared: shared_result,
        produced_output: produced,
        tool: prepared.application,
        provenance,
    })
}

pub fn undo_hackhash_patch(
    result: &HackHashPatchApplyResult,
    destination_root: &Path,
    history_root: impl AsRef<Path>,
    backup_root: impl AsRef<Path>,
) -> SharedRollbackResult {
    let Some(journal_path) = result.shared.journal_path.as_ref() else {
        return SharedRollbackResult {
            preview: preview_shared_rollback(
                Path::new("/unavailable"),
                destination_root,
                backup_root.as_ref(),
            ),
            journal_path: None,
            status: crate::patch_manager::SharedApplyStatus::Failed,
        };
    };
    let preview = preview_shared_rollback(journal_path, destination_root, backup_root.as_ref());
    let options = SharedRollbackOptions {
        confirmation: SharedRollbackConfirmation {
            preview_id: preview.preview_id.clone(),
            approved: true,
        },
        rollback_operation_id: generate_shared_operation_id(),
        timestamp_unix_seconds: now(),
        history_root: history_root.as_ref().to_path_buf(),
        backup_root: backup_root.as_ref().to_path_buf(),
    };
    execute_shared_rollback(&preview, &options)
}

fn validate_root_and_file(root: &Path, destination: &Path) -> Result<(), HackHashPatchApplyError> {
    let root_metadata = fs::symlink_metadata(root).ok();
    if !root.is_absolute()
        || root.parent().is_none()
        || !root_metadata
            .is_some_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        return Err(HackHashPatchApplyError::Unsafe(
            "managed destination root must be an existing directory".into(),
        ));
    }
    if destination.parent() != Some(root) && destination.strip_prefix(root).is_err() {
        return Err(HackHashPatchApplyError::Unsafe(
            "destination is outside managed root".into(),
        ));
    }
    if fs::symlink_metadata(destination).is_ok() {
        return Err(HackHashPatchApplyError::Unsafe(
            "destination already exists".into(),
        ));
    }
    Ok(())
}

fn hashes(bytes: &[u8]) -> HackHashOutputHashes {
    let mut sha1 = Sha1::new();
    sha1.update(bytes);
    let mut crc = Crc32::new();
    crc.update(bytes);
    HackHashOutputHashes {
        sha1: hex(&sha1.finalize()),
        md5: hex(&Md5::digest(bytes)),
        crc32: crc.finish_hex(),
    }
}

fn inspect_prepared_output(
    bytes: &[u8],
    output_path: &Path,
) -> Result<HackHashNativeInspection, HackHashPatchApplyError> {
    let temp_root = tempfile::tempdir().map_err(|error| {
        HackHashPatchApplyError::Transaction(format!(
            "native inspection staging directory unavailable: {error}"
        ))
    })?;
    let file_name = output_path.file_name().ok_or_else(|| {
        HackHashPatchApplyError::Unsafe("output has no filename for native inspection".into())
    })?;
    let inspection_path = temp_root.path().join(file_name);
    fs::write(&inspection_path, bytes).map_err(|error| {
        HackHashPatchApplyError::Transaction(format!(
            "native inspection staging write failed: {error}"
        ))
    })?;
    inspect_native_path(&inspection_path)
}

fn inspect_native_path(path: &Path) -> Result<HackHashNativeInspection, HackHashPatchApplyError> {
    let trusted_root = path.parent().unwrap_or(path);
    let hashes =
        hash_file(path, &TrustedRoots::from_paths([trusted_root]), None).map_err(|error| {
            HackHashPatchApplyError::Transaction(format!(
                "native output hash inspection failed: {}",
                error.detail()
            ))
        })?;
    let report = inspect_catalogued_game_identity(path, None);
    let evidence = report
        .evidence
        .iter()
        .map(|item| {
            format!(
                "{}={:?}:{}",
                item.kind,
                item.status,
                item.value.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>();
    let warnings = report.warnings;
    let complete = report.complete
        && !report
            .evidence
            .iter()
            .any(|item| matches!(item.status, IdentityStatus::Invalid));
    Ok(HackHashNativeInspection {
        complete,
        platform: format!("{:?}", report.platform),
        format: format!("{:?}", report.format),
        evidence,
        warnings,
        hashes: HackHashOutputHashes {
            sha1: hashes.sha1,
            md5: hashes.md5,
            crc32: hashes.crc32,
        },
    })
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest as Sha2Digest, Sha256};
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::hackhash_readiness::{
        HackHashPatchReadinessEvidence, HackHashPatchReadinessStatus,
    };
    use crate::standalone_patch::{
        StandalonePatchFormat, build_standalone_patch_apply_plan, inspect_standalone_patch,
    };

    fn ips_patch() -> Vec<u8> {
        b"PATCH\x00\x00\x01\x00\x01ZEOF".to_vec()
    }

    fn readiness(expected: HackHashOutputHashes) -> HackHashPatchReadinessEvidence {
        HackHashPatchReadinessEvidence {
            status: HackHashPatchReadinessStatus::ReadyToPatch,
            base_hash_match: true,
            expected_base_identity: Some("Base".into()),
            patch_hash_match: true,
            patch_format: StandalonePatchFormat::Ips,
            expected_output_hashes: vec![format!("SHA-1 {}", expected.sha1)],
            expected_output: Some(expected),
            hack_titles: vec!["Hack".into()],
            versions: vec!["1".into()],
            family_versions: vec![],
            snapshot_sha256: Some("snapshot".into()),
            provider_provenance: "test snapshot".into(),
            conflicts: vec![],
            missing_evidence: vec![],
            refusal_reasons: vec![],
            explanation: "exact".into(),
        }
    }

    fn fixture() -> (
        tempfile::TempDir,
        HackHashPatchApplyPlan,
        HackHashOutputHashes,
        PathBuf,
        PathBuf,
    ) {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("base.rom");
        let patch = root.path().join("hack.ips");
        let output_root = root.path().join("managed");
        let output = output_root.join("hack.rom");
        fs::create_dir(&output_root).unwrap();
        fs::write(&base, b"abc").unwrap();
        fs::write(&patch, ips_patch()).unwrap();
        let inspection = inspect_standalone_patch(&patch).unwrap();
        let standalone =
            build_standalone_patch_apply_plan(&inspection, &base, &output, &output_root).unwrap();
        let expected = hashes(&prepare_standalone_patch_output(&standalone).unwrap().bytes);
        let plan = build_hackhash_patch_apply_plan(
            &readiness(expected.clone()),
            standalone,
            &output_root,
            &output,
            "snapshot",
        )
        .unwrap();
        (root, plan, expected, base, patch)
    }

    #[test]
    fn exact_patch_publishes_transactionally_and_undo_removes_only_output() {
        let (_root, plan, expected, base, patch) = fixture();
        let history = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let base_before = fs::read(&base).unwrap();
        let patch_before = fs::read(&patch).unwrap();
        let result = apply_hackhash_patch(
            &plan,
            "snapshot",
            &expected,
            "APPLY HACKHASH PATCH",
            history.path(),
            backup.path(),
        )
        .unwrap();
        assert_eq!(
            result.shared.journal.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        assert_eq!(result.provenance.actual_output, expected);
        assert_eq!(result.provenance.native_inspection.hashes, expected);
        assert_eq!(
            result.provenance.transaction_id,
            result.shared.journal.operation_id
        );
        assert!(result.shared.journal.entries.iter().any(|entry| {
            matches!(
                entry.plan_entry.content_verification,
                Some(
                    crate::patch_manager::SharedContentVerification::HackHashPatch {
                        provenance: Some(_),
                        ..
                    }
                )
            )
        }));
        let journal_text =
            fs::read_to_string(result.shared.journal_path.as_ref().unwrap()).unwrap();
        assert!(journal_text.contains("provider_snapshot_sha256"));
        assert_eq!(fs::read(&base).unwrap(), base_before);
        assert_eq!(fs::read(&patch).unwrap(), patch_before);
        assert_eq!(hashes(&fs::read(&plan.destination_path).unwrap()), expected);
        let undo = undo_hackhash_patch(
            &result,
            plan.shared
                .destination_root
                .to_path_buf()
                .unwrap()
                .as_path(),
            history.path(),
            backup.path(),
        );
        assert_eq!(
            undo.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        assert!(!plan.destination_path.exists());
    }

    #[test]
    fn stale_snapshot_and_output_refuse_before_mutation() {
        let (root, plan, expected, base, _) = fixture();
        let error = apply_hackhash_patch(
            &plan,
            "changed-snapshot",
            &expected,
            "APPLY HACKHASH PATCH",
            root.path().join("history"),
            root.path().join("backup"),
        )
        .unwrap_err();
        assert!(matches!(error, HackHashPatchApplyError::Stale(_)));
        assert_eq!(fs::read(base).unwrap(), b"abc");

        let (_, plan, _, _, _) = fixture();
        let mut changed = expected.clone();
        changed.sha1.replace_range(..1, "0");
        let error = apply_hackhash_patch(
            &plan,
            "snapshot",
            &changed,
            "APPLY HACKHASH PATCH",
            root.path().join("history-2"),
            root.path().join("backup-2"),
        )
        .unwrap_err();
        assert!(matches!(error, HackHashPatchApplyError::Stale(_)));

        let (root, plan, expected, base, _) = fixture();
        fs::write(&base, b"changed").unwrap();
        let error = apply_hackhash_patch(
            &plan,
            "snapshot",
            &expected,
            "APPLY HACKHASH PATCH",
            root.path().join("history"),
            root.path().join("backup"),
        )
        .unwrap_err();
        assert!(matches!(error, HackHashPatchApplyError::Patch(_)));

        let (root, plan, expected, _, patch) = fixture();
        fs::write(&patch, b"changed").unwrap();
        let error = apply_hackhash_patch(
            &plan,
            "snapshot",
            &expected,
            "APPLY HACKHASH PATCH",
            root.path().join("history"),
            root.path().join("backup"),
        )
        .unwrap_err();
        assert!(matches!(error, HackHashPatchApplyError::Patch(_)));
    }

    #[test]
    fn output_mismatch_and_destination_collision_fail_closed() {
        let (root, plan, _, _, _) = fixture();
        let wrong = HackHashOutputHashes {
            sha1: "0".repeat(40),
            md5: "0".repeat(32),
            crc32: "00000000".into(),
        };
        let error = apply_hackhash_patch(
            &plan,
            "snapshot",
            &wrong,
            "APPLY HACKHASH PATCH",
            root.path().join("history"),
            root.path().join("backup"),
        )
        .unwrap_err();
        assert!(matches!(error, HackHashPatchApplyError::Stale(_)));

        fs::write(&plan.destination_path, b"user file").unwrap();
        let error = apply_hackhash_patch(
            &plan,
            "snapshot",
            &plan.expected_output,
            "APPLY HACKHASH PATCH",
            root.path().join("history-3"),
            root.path().join("backup-3"),
        )
        .unwrap_err();
        assert!(matches!(error, HackHashPatchApplyError::Unsafe(_)));
    }
}
