//! One durable RetroDECK operation, using the existing link executor and guarded
//! ES-DE publisher. The immutable receipt precedes mutation; mutable checkpoints
//! describe progress, never cross-file atomicity. Child journals remain intact.
use super::*;
use crate::dat::rename_apply::{
    executor::{ApplyExecution, apply_retrodeck_transaction},
    journal::journal_path,
    model::{EntryState, RenameTransaction, RollbackResult, TransactionState},
    rollback::rollback_retrodeck_links,
};
use crate::safe_read::TrustedRoots;
use std::io::Write;
use std::sync::atomic::AtomicBool;

const LIMIT: u64 = MAX_RECOVERY_RECORD_BYTES + 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Prepared,
    LinksApplying,
    MetadataPublishing,
    Completed,
    MetadataRestoring,
    LinksRestoring,
    Restored,
    RecoveryRequired,
}

/// Immutable approval evidence; fields cannot be replaced by a later preview.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    schema: u32,
    links: RenameTransaction,
    metadata: EsDeGamelistRecoveryRecord,
    link_namespace: Vec<(PathBuf, DestinationEvidence)>,
    digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    receipt_digest: String,
    phase: Phase,
    detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OperationOutcome {
    pub transaction: RenameTransaction,
    pub phase: Phase,
    pub detail: Option<String>,
}

fn paths(dir: &Path, id: &str) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let journal = journal_path(dir, id).ok_or("invalid transaction identifier")?;
    Ok((
        journal.with_extension("retrodeck-receipt"),
        journal.with_extension("retrodeck-state"),
        journal,
    ))
}
fn digest(receipt: &Receipt) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut copy = receipt.clone();
    copy.digest.clear();
    Ok(
        Sha256::digest(serde_json::to_vec(&copy).map_err(|e| e.to_string())?)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}
fn checkpoint(
    dir: &Path,
    receipt: &Receipt,
    phase: Phase,
    detail: Option<String>,
) -> Result<(), String> {
    let (_, path, _) = paths(dir, &receipt.links.transaction_id)?;
    // Refuse symlinks or an unavailable ancestor before invoking the shared writer.
    DestinationEvidence::capture(&path).map_err(|e| e.to_string())?;
    let state = Checkpoint {
        receipt_digest: receipt.digest.clone(),
        phase,
        detail,
    };
    crate::atomic_write_text(
        &path,
        &serde_json::to_string(&state).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut transaction = bound_transaction(dir, receipt)?;
    transaction.unknown.insert(
        "retrodeck_gamelist_path".into(),
        serde_json::to_value(&receipt.metadata.gamelist_path).map_err(|e| e.to_string())?,
    );
    let (receipt_path, checkpoint_path, link_journal) = paths(dir, &receipt.links.transaction_id)?;
    transaction.unknown.insert(
        "retrodeck_child_evidence".into(),
        serde_json::to_value([receipt_path, checkpoint_path, link_journal])
            .map_err(|e| e.to_string())?,
    );
    transaction.unknown.insert(
        "retrodeck_operation_phase".into(),
        serde_json::to_value(phase).map_err(|e| e.to_string())?,
    );
    transaction.unknown.insert(
        "retrodeck_operation_detail".into(),
        serde_json::to_value(&state.detail).map_err(|e| e.to_string())?,
    );
    crate::dat::rename_apply::journal::write_journal(dir, &transaction).map_err(|e| e.to_string())
}

pub fn load(dir: &Path, id: &str) -> Result<(Receipt, Phase, Option<String>), String> {
    let (path, state_path, _) = paths(dir, id)?;
    let receipt: Receipt =
        serde_json::from_str(&bounded_text(&path, LIMIT).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if receipt.schema != 1
        || receipt.links.transaction_id != id
        || digest(&receipt)? != receipt.digest
        || receipt.metadata.schema_version != RECOVERY_SCHEMA_VERSION
        || receipt.metadata.after_content.is_none()
        || receipt.metadata.destination_evidence.is_none()
        || receipt.metadata.digest.as_deref()
            != Some(
                record_digest(&receipt.metadata)
                    .map_err(|e| e.to_string())?
                    .as_str(),
            )
    {
        return Err("invalid RetroDECK receipt; manual review required".into());
    }
    match bounded_text(&state_path, 64 * 1024) {
        Ok(body) => {
            let state: Checkpoint = serde_json::from_str(&body).map_err(|e| e.to_string())?;
            if state.receipt_digest != receipt.digest {
                return Err("mismatched RetroDECK checkpoint".into());
            }
            Ok((receipt, state.phase, state.detail))
        }
        Err(_) => Ok((
            receipt,
            Phase::RecoveryRequired,
            Some("checkpoint unavailable; operation interrupted".into()),
        )),
    }
}

/// Read-only restart discovery. Invalid receipts are reported, never discarded.
pub fn discover(dir: &Path) -> (Vec<OperationOutcome>, Vec<String>) {
    let mut outcomes = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (outcomes, errors),
        Err(e) => {
            return (
                outcomes,
                vec![format!("RetroDECK recovery storage unavailable: {e}")],
            );
        }
    };
    let mut ids = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index == 1000 {
            errors.push("RetroDECK recovery discovery reached its 1000-entry bound; further receipts require review".into());
            break;
        }
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.extension().is_some_and(|x| x == "retrodeck-receipt") {
                    if let Some(id) = path.file_stem().and_then(|x| x.to_str()) {
                        ids.push(id.to_owned());
                    } else {
                        errors.push(
                            "RetroDECK receipt has an unsupported name; preserve for review".into(),
                        );
                    }
                }
            }
            Err(e) => errors.push(format!("RetroDECK recovery entry unavailable: {e}")),
        }
    }
    ids.sort();
    for id in ids {
        match inspect(dir, &id) {
            Ok(value) => outcomes.push(value),
            Err(error) => errors.push(format!("{id}: {error}")),
        }
    }
    (outcomes, errors)
}

fn bound_transaction(dir: &Path, receipt: &Receipt) -> Result<RenameTransaction, String> {
    let (_, _, path) = paths(dir, &receipt.links.transaction_id)?;
    let transaction: RenameTransaction =
        serde_json::from_str(&bounded_text(&path, 16 * 1024 * 1024).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let a = &receipt.links;
    if transaction.transaction_id != a.transaction_id
        || transaction.plan_generation != a.plan_generation
        || transaction.classifier_version != a.classifier_version
        || transaction.source_scan_root != a.source_scan_root
        || transaction.entries.len() != a.entries.len()
        || transaction.entries.iter().zip(&a.entries).any(|(x, y)| {
            x.source_path != y.source_path
                || x.destination_path != y.destination_path
                || x.operation != y.operation
                || x.identity != y.identity
        })
        || transaction.unknown.get("retrodeck_operation_receipt")
            != Some(&serde_json::Value::String(receipt.digest.clone()))
    {
        return Err("link journal does not match immutable RetroDECK receipt".into());
    }
    Ok(transaction)
}

pub fn inspect(dir: &Path, id: &str) -> Result<OperationOutcome, String> {
    let (receipt, mut phase, mut detail) = load(dir, id)?;
    let transaction = bound_transaction(dir, &receipt)?;
    let settled = match phase {
        Phase::Completed => {
            transaction.state == TransactionState::Applied
                && transaction
                    .entries
                    .iter()
                    .all(|e| e.state == EntryState::Applied)
        }
        Phase::Restored => transaction.is_fully_rolled_back(),
        _ => true,
    };
    if !settled
        || transaction.unknown.get("retrodeck_operation_phase")
            != Some(&serde_json::to_value(phase).map_err(|e| e.to_string())?)
    {
        phase = Phase::RecoveryRequired;
        detail =
            Some("operation checkpoint and link journal disagree; review recovery evidence".into());
    }
    Ok(OperationOutcome {
        transaction,
        phase,
        detail,
    })
}

fn lock_output(transaction: &RenameTransaction) -> Result<PublicationLock, String> {
    use crate::dat::rename_apply::model::TransactionOperation;
    let Some(TransactionOperation::CreateSymlink {
        destination_root: root,
        ..
    }) = transaction.entries.first().map(|e| &e.operation)
    else {
        return Err("missing link destination root".into());
    };
    if transaction.entries.iter().any(|e| !matches!(&e.operation, TransactionOperation::CreateSymlink { destination_root, .. } if destination_root == root)) { return Err("mixed RetroDECK output roots are unsupported".into()); }
    let path = root.join(".retrodeck-operation");
    DestinationEvidence::capture(&path).map_err(|e| e.to_string())?;
    PublicationLock::acquire(&path).map_err(|e| e.to_string())
}

/// A failed link apply never publishes metadata. A failed stage attempts the
/// guarded inverse; conflicts retain both receipts for explicit recovery.
pub fn apply(
    publication: &EsDeGamelistPublication,
    execution: &mut ApplyExecution<'_>,
) -> Result<OperationOutcome, String> {
    let journal = journal_path(
        &execution.journal_dir,
        &execution.transaction.transaction_id,
    )
    .ok_or("invalid transaction identifier")?;
    if execution
        .transaction
        .unknown
        .get("retrodeck_operation_intent")
        != Some(&serde_json::Value::Bool(true))
        || crate::dat::rename_apply::journal::read_journal(&journal).map_err(|e| e.to_string())?
            != *execution.transaction
    {
        return Err("RetroDECK requires its original prepared intent receipt".into());
    }

    if execution.transaction.entries.iter().any(|e| {
        !matches!(
            e.operation,
            crate::dat::rename_apply::model::TransactionOperation::CreateSymlink { .. }
        )
    }) {
        return Err("RetroDECK publication requires a symlink-only transaction".into());
    }
    if !publication.refused.is_empty() {
        return Err(
            "RetroDECK metadata contains refused elections; resolve them before Apply".into(),
        );
    }
    if publication
        .added
        .iter()
        .chain(&publication.already_present)
        .any(|entry| {
            !execution
                .transaction
                .entries
                .iter()
                .any(|link| link.destination_path == entry.destination_path)
        })
    {
        return Err("publication includes a game outside the applied link transaction".into());
    }
    let _output_lock = lock_output(execution.transaction)?;
    for entry in &execution.transaction.entries {
        match fs::symlink_metadata(&entry.destination_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            _ => return Err("destination link already exists or is unavailable; review its ownership before applying".into()),
        }
    }
    publication
        .destination_evidence
        .verify_before(&publication.gamelist_path, &publication.previous_content)
        .map_err(|e| e.to_string())?;
    if let Some(parent) = publication.gamelist_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    publication
        .destination_evidence
        .verify_before(&publication.gamelist_path, &publication.previous_content)
        .map_err(|e| e.to_string())?;
    let mut receipt = Receipt {
        schema: 1,
        links: execution.transaction.clone(),
        metadata: publication_record(publication).map_err(|e| e.to_string())?,
        link_namespace: execution
            .transaction
            .entries
            .iter()
            .flat_map(|e| [&e.source_path, &e.destination_path])
            .map(|path| {
                // Namespace evidence must not inspect the resulting symlink as
                // though it were a gamelist. This probe path is never created.
                let probe = path.with_file_name(".emuwiz-retrodeck-namespace");
                DestinationEvidence::capture(&probe)
                    .map(|evidence| (probe, evidence))
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<_, _>>()?,
        digest: String::new(),
    };
    receipt.digest = digest(&receipt)?;
    let (path, _, _) = paths(&execution.journal_dir, &receipt.links.transaction_id)?;
    DestinationEvidence::capture(&path).map_err(|e| e.to_string())?;
    let _lock = PublicationLock::acquire(&path).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&receipt).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("RetroDECK receipt too large".into());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&path).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    execution.transaction.unknown.insert(
        "retrodeck_operation_receipt".into(),
        serde_json::Value::String(receipt.digest.clone()),
    );
    crate::dat::rename_apply::journal::write_journal(&execution.journal_dir, execution.transaction)
        .map_err(|e| e.to_string())?;
    checkpoint(&execution.journal_dir, &receipt, Phase::LinksApplying, None)?;
    *execution.transaction = bound_transaction(&execution.journal_dir, &receipt)?;
    operation_hook("before_links", &publication.gamelist_path);
    let applied = apply_retrodeck_transaction(execution);
    let complete = applied.as_ref().is_ok_and(|o| {
        o.transaction.state == TransactionState::Applied
            && o.transaction
                .entries
                .iter()
                .all(|e| e.state == EntryState::Applied)
    });
    if !complete {
        let detail = Some(match applied {
            Err(e) => e.to_string(),
            Ok(o) => format!("link application incomplete: {:?}", o.transaction.state),
        });
        checkpoint(
            &execution.journal_dir,
            &receipt,
            Phase::RecoveryRequired,
            detail.clone(),
        )?;
        drop(_lock);
        drop(_output_lock);
        return restore_inner(
            &execution.journal_dir,
            &receipt.links.transaction_id,
            &AtomicBool::new(false),
            detail,
        );
    }
    checkpoint(
        &execution.journal_dir,
        &receipt,
        Phase::MetadataPublishing,
        None,
    )?;
    let result = operation_fault("before_metadata")
        .and_then(|_| apply_es_de_gamelist_publication(publication).map_err(|e| e.to_string()));
    let (phase, detail) = match result {
        Ok(()) => (Phase::Completed, None),
        Err(e) => (Phase::RecoveryRequired, Some(e.to_string())),
    };
    operation_fault("before_completion")?;
    checkpoint(&execution.journal_dir, &receipt, phase, detail.clone())?;
    if phase == Phase::RecoveryRequired {
        drop(_lock);
        drop(_output_lock);
        return restore_inner(
            &execution.journal_dir,
            &receipt.links.transaction_id,
            &AtomicBool::new(false),
            detail,
        );
    }
    Ok(OperationOutcome {
        transaction: bound_transaction(&execution.journal_dir, &receipt)?,
        phase,
        detail,
    })
}

/// Explicit Undo/recovery: metadata first, then confined link rollback. Never
/// proceeds past a metadata conflict. Interrupted reverse stages are retryable.
pub fn restore(dir: &Path, id: &str, cancel: &AtomicBool) -> Result<OperationOutcome, String> {
    restore_inner(dir, id, cancel, None)
}

fn restore_inner(
    dir: &Path,
    id: &str,
    cancel: &AtomicBool,
    failure_detail: Option<String>,
) -> Result<OperationOutcome, String> {
    let (receipt, _, _) = load(dir, id)?;
    let (path, _, _) = paths(dir, id)?;
    let _lock = PublicationLock::acquire(&path).map_err(|e| e.to_string())?;
    let mut transaction = bound_transaction(dir, &receipt)?;
    for (path, evidence) in &receipt.link_namespace {
        evidence
            .verify_directories(path)
            .map_err(|e| e.to_string())?;
    }
    let _output_lock = lock_output(&transaction)?;
    let issues = crate::dat::rename_apply::reconcile::reconcile_retrodeck(&mut transaction, dir)?;
    if issues.iter().any(|issue| {
        !matches!(
            issue.kind,
            crate::dat::rename_apply::reconcile::RecoveryIssueKind::RenameDidNotHappen
                | crate::dat::rename_apply::reconcile::RecoveryIssueKind::RenameConfirmed
        )
    }) {
        return Err("Recovery required: unresolved link journal entries".into());
    }
    for entry in &transaction.entries {
        if entry.state == EntryState::Applied {
            if fs::read_link(&entry.destination_path).ok().as_ref() != Some(&entry.source_path) {
                let detail = Some("link output changed; metadata and links left untouched".into());
                checkpoint(dir, &receipt, Phase::RecoveryRequired, detail.clone())?;
                return Ok(OperationOutcome {
                    transaction: bound_transaction(dir, &receipt)?,
                    phase: Phase::RecoveryRequired,
                    detail,
                });
            }
        }
    }
    checkpoint(dir, &receipt, Phase::MetadataRestoring, None)?;
    let metadata = &receipt.metadata;
    let restore = (|| {
        let _metadata_lock = PublicationLock::acquire(&metadata.gamelist_path)?;
        if has_unresolved_es_de_gamelist_recovery(&metadata.gamelist_path) {
            let current = read_recovery_record(
                &es_de_gamelist_recovery_path(&metadata.gamelist_path),
                &metadata.gamelist_path,
            )?;
            if current != *metadata {
                return Err(conflict(
                    &metadata.gamelist_path,
                    "recovery evidence belongs to another publication",
                ));
            }
            guarded_restore(
                &metadata.gamelist_path,
                &metadata.previous_content,
                metadata.after_content.as_ref().unwrap(),
                metadata.destination_evidence.as_ref().unwrap(),
            )?;
            clear_record(&metadata.gamelist_path, &current)?;
        } else {
            guarded_restore(
                &metadata.gamelist_path,
                &metadata.previous_content,
                metadata.after_content.as_ref().unwrap(),
                metadata.destination_evidence.as_ref().unwrap(),
            )?;
        }
        Ok::<_, EsDePublicationError>(())
    })();
    if let Err(error) = restore {
        let detail = Some(error.to_string());
        checkpoint(dir, &receipt, Phase::RecoveryRequired, detail.clone())?;
        return Ok(OperationOutcome {
            transaction: bound_transaction(dir, &receipt)?,
            phase: Phase::RecoveryRequired,
            detail,
        });
    }
    checkpoint(dir, &receipt, Phase::LinksRestoring, None)?;
    transaction = bound_transaction(dir, &receipt)?;
    let trusted = TrustedRoots::from_paths(transaction.entries.iter().flat_map(|e| {
        [
            e.source_path.parent().unwrap().to_path_buf(),
            e.destination_path.parent().unwrap().to_path_buf(),
        ]
    }));
    operation_hook("before_link_rollback", &metadata.gamelist_path);
    let result = rollback_retrodeck_links(&mut transaction, dir, cancel, &trusted);
    let (phase, detail) = match result {
        Ok(outcome)
            if matches!(outcome.result, RollbackResult::FullyRolledBack)
                && outcome.transaction.state == TransactionState::RolledBack =>
        {
            (Phase::Restored, failure_detail)
        }
        Ok(outcome) => (
            Phase::RecoveryRequired,
            Some(format!("link rollback incomplete: {:?}", outcome.result)),
        ),
        Err(error) => (Phase::RecoveryRequired, Some(error)),
    };
    checkpoint(dir, &receipt, phase, detail.clone())?;
    Ok(OperationOutcome {
        transaction: bound_transaction(dir, &receipt)?,
        phase,
        detail,
    })
}

#[cfg(any(test, feature = "test-support"))]
thread_local! { pub(super) static OPERATION_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
/// Synthetic GUI tests interrupt after metadata publication, before completion.
/// Unavailable in ordinary application builds.
#[cfg(feature = "test-support")]
pub fn set_test_fault(stage: Option<&'static str>) {
    assert!(matches!(
        stage,
        None | Some("before_completion") | Some("before_metadata")
    ));
    OPERATION_FAULT.with(|fault| fault.set(stage));
}
fn operation_fault(stage: &str) -> Result<(), String> {
    #[cfg(any(test, feature = "test-support"))]
    if OPERATION_FAULT.with(|f| f.get() == Some(stage)) {
        return Err(format!("injected {stage} interruption"));
    }
    let _ = stage;
    Ok(())
}

#[cfg(test)]
thread_local! { pub(super) static OPERATION_HOOK: std::cell::RefCell<Option<Box<dyn Fn(&str, &Path)>>> = const { std::cell::RefCell::new(None) }; }
fn operation_hook(stage: &str, path: &Path) {
    #[cfg(test)]
    OPERATION_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook(stage, path);
        }
    });
    let _ = (stage, path);
}

/// Any parent reservation OR coordinator field prevents an independent child action.
/// This deliberately checks presence rather than trusting a mutable child marker.
pub fn is_coordinated(dir: &Path, child: &RenameTransaction) -> Result<bool, String> {
    // A redirected or unavailable journal namespace cannot prove that its
    // parent receipt is absent. The probe is inspected, never created.
    DestinationEvidence::capture(&dir.join(".emuwiz-retrodeck-ownership-probe"))
        .map_err(|e| format!("RetroDECK ownership namespace unavailable: {e}"))?;
    let (receipt, state, _) = paths(dir, &child.transaction_id)?;
    let mut owned = child.unknown.keys().any(|k| k.starts_with("retrodeck_"));
    for path in [receipt, state] {
        match fs::symlink_metadata(path) {
            Ok(_) => owned = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("RetroDECK ownership unavailable: {e}")),
        }
    }
    Ok(owned)
}

pub fn require_independent(dir: &Path, child: &RenameTransaction) -> Result<(), String> {
    if is_coordinated(dir, child)? {
        return Err("RetroDECK requires coordinated metadata/link rollback or recovery from Build Playing Library; preserve its receipts".into());
    }
    Ok(())
}

pub(crate) fn validate_child(dir: &Path, child: &RenameTransaction) -> Result<(), String> {
    let (receipt, _, _) = load(dir, &child.transaction_id)?;
    let current = bound_transaction(dir, &receipt)?;
    if current != *child {
        return Err("RetroDECK child changed; recovery review required".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

/// Reserve the child as RetroDECK-owned before its first journal becomes visible.
/// No source proof is recaptured and no game/media output is changed.
pub fn prepare(dir: &Path, child: &mut RenameTransaction) -> Result<(), String> {
    require_independent(dir, child)?;
    if child.state != TransactionState::Planned
        || child.entries.iter().any(|e| {
            e.state != EntryState::Planned
                || !matches!(
                    e.operation,
                    crate::dat::rename_apply::model::TransactionOperation::CreateSymlink { .. }
                )
        })
    {
        return Err("RetroDECK preparation requires a fresh symlink-only preview".into());
    }
    let path = journal_path(dir, &child.transaction_id).ok_or("invalid transaction identifier")?;
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let old = crate::dat::rename_apply::journal::read_journal(&path)
                .map_err(|e| e.to_string())?;
            if old != *child {
                return Err("transaction journal already belongs to another attempt".into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    child.unknown.insert(
        "retrodeck_operation_intent".into(),
        serde_json::Value::Bool(true),
    );
    crate::dat::rename_apply::journal::write_journal(dir, child).map_err(|e| e.to_string())
}
