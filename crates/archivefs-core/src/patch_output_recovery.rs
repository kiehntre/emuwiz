//! Durable, provider-neutral recovery for standalone patch output.
//!
//! This journal deliberately sits beside the destination because the output
//! must be published atomically on the same filesystem. It records intent
//! before any user-visible output is created, keeps every checkpoint durable,
//! and never resumes or removes anything during discovery. The standalone
//! patch decoder remains in [`crate::standalone_patch`]; this module owns only
//! publication, verification, and recovery evidence.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::standalone_patch::StandalonePatchFormat;

pub const PATCH_OUTPUT_JOURNAL_SCHEMA_VERSION: u32 = 1;
pub const MAX_PATCH_OUTPUT_JOURNALS: usize = 512;
pub const MAX_PATCH_OUTPUT_JOURNAL_BYTES: u64 = 512 * 1024;
const JOURNAL_PREFIX: &str = ".emuwiz-patch-output-";
static OPERATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchOutputState {
    Planned,
    Preparing,
    Prepared,
    VerifyingTemporary,
    Publishing,
    Published,
    VerifyingPublished,
    Completed,
    Failed,
    RollingBack,
    RolledBack,
    RollbackFailed,
    NeedsReview,
    UnsafeToResume,
}

impl PatchOutputState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::RolledBack)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchOutputCheckpoint {
    IntentDurable,
    Preparing,
    TemporaryWriteStarted,
    TemporaryWritten,
    Prepared,
    VerifyingTemporary,
    TemporaryVerified,
    BeforePublish,
    Published,
    BeforePublishedVerification,
    PublishedVerified,
    BeforeCompleted,
    RollbackStarted,
    RollbackMutated,
    RollbackCompleted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PatchOutputJournal {
    pub schema_version: u32,
    pub operation_id: String,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub patch_format: StandalonePatchFormat,
    pub source_path: PathBuf,
    pub source_size: u64,
    pub source_sha256: String,
    pub patch_path: PathBuf,
    pub patch_size: u64,
    pub patch_sha256: String,
    pub destination_path: PathBuf,
    pub temporary_output_path: PathBuf,
    pub provenance_path: PathBuf,
    pub provenance_sha256: Option<String>,
    pub expected_output_size: Option<u64>,
    pub expected_output_sha256: Option<String>,
    pub destination_preexisting: bool,
    pub previous_destination_sha256: Option<String>,
    pub backup_path: Option<PathBuf>,
    pub state: PatchOutputState,
    pub checkpoints: Vec<PatchOutputCheckpoint>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatchOutputIntent {
    pub patch_format: StandalonePatchFormat,
    pub source_path: PathBuf,
    pub source_size: u64,
    pub source_sha256: String,
    pub patch_path: PathBuf,
    pub patch_size: u64,
    pub patch_sha256: String,
    pub destination_path: PathBuf,
    pub expected_output_size: Option<u64>,
    pub provenance_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedPatchOutput {
    pub bytes: Vec<u8>,
    pub application: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurablePatchOutputResult {
    pub journal_path: PathBuf,
    pub output_sha256: String,
    pub output_size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatchOutputRecoveryClassification {
    SafeToResume,
    PublicationLikelyComplete,
    Failed,
    Completed,
    RolledBack,
    NeedsReview,
    UnsafeToResume,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatchOutputRecoveryInspection {
    pub journal_path: PathBuf,
    pub journal: PatchOutputJournal,
    pub classification: PatchOutputRecoveryClassification,
    pub explanation: String,
    pub source_matches: bool,
    pub patch_matches: bool,
    pub temporary_matches: bool,
    pub destination_matches: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatchOutputRecoveryPlan {
    pub journal_path: PathBuf,
    pub operation_id: String,
    pub classification: PatchOutputRecoveryClassification,
    pub precondition_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatchOutputRecoveryError {
    Io(String),
    InvalidJournal(String),
    UnknownSchema(u32),
    PreconditionsChanged(String),
    Unsafe(String),
    Interrupted(String),
    Failed(String),
}

impl std::fmt::Display for PatchOutputRecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(value) => write!(f, "patch-output recovery I/O error: {value}"),
            Self::InvalidJournal(value) => write!(f, "invalid patch-output journal: {value}"),
            Self::UnknownSchema(value) => {
                write!(f, "unsupported patch-output journal schema: {value}")
            }
            Self::PreconditionsChanged(value) => {
                write!(f, "patch-output recovery preconditions changed: {value}")
            }
            Self::Unsafe(value) => write!(f, "unsafe patch-output recovery: {value}"),
            Self::Interrupted(value) => {
                write!(f, "deterministic patch-output interruption: {value}")
            }
            Self::Failed(value) => write!(f, "patch-output operation failed: {value}"),
        }
    }
}

impl std::error::Error for PatchOutputRecoveryError {}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}

fn digest_path(path: &Path) -> Result<(u64, String), PatchOutputRecoveryError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(PatchOutputRecoveryError::Unsafe(format!(
            "not a regular non-symlink file: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))?;
    Ok((metadata.len(), digest_bytes(&bytes)))
}

fn digest_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|value| format!("{value:02x}"))
        .collect()
}

fn operation_id(intent: &PatchOutputIntent) -> String {
    let sequence = OPERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stamp = now();
    let mut hasher = Sha256::new();
    hasher.update(intent.source_sha256.as_bytes());
    hasher.update(intent.patch_sha256.as_bytes());
    hasher.update(intent.destination_path.to_string_lossy().as_bytes());
    format!(
        "{stamp}-{sequence}-{}",
        &hex_digest(&hasher.finalize())[..16]
    )
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

fn journal_path_for(
    intent: &PatchOutputIntent,
    id: &str,
) -> Result<PathBuf, PatchOutputRecoveryError> {
    let parent = intent
        .destination_path
        .parent()
        .ok_or_else(|| PatchOutputRecoveryError::Unsafe("destination has no parent".into()))?;
    let name = intent
        .destination_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            PatchOutputRecoveryError::Unsafe("destination filename is not representable".into())
        })?;
    Ok(parent.join(format!("{JOURNAL_PREFIX}{name}-{id}.json")))
}

fn temporary_path(
    intent: &PatchOutputIntent,
    id: &str,
) -> Result<PathBuf, PatchOutputRecoveryError> {
    let parent = intent
        .destination_path
        .parent()
        .ok_or_else(|| PatchOutputRecoveryError::Unsafe("destination has no parent".into()))?;
    let name = intent
        .destination_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            PatchOutputRecoveryError::Unsafe("destination filename is not representable".into())
        })?;
    Ok(parent.join(format!(".{name}.emuwiz-patch-{id}.tmp")))
}

fn ensure_sibling(path: &Path, sibling: &Path) -> Result<(), PatchOutputRecoveryError> {
    if sibling.parent() != path.parent() {
        return Err(PatchOutputRecoveryError::Unsafe(
            "journal/temp is not beside destination".into(),
        ));
    }
    if sibling.exists() || fs::symlink_metadata(sibling).is_ok() {
        return Err(PatchOutputRecoveryError::Unsafe(format!(
            "operation artifact already exists: {}",
            sibling.display()
        )));
    }
    Ok(())
}

fn write_journal(
    path: &Path,
    journal: &PatchOutputJournal,
) -> Result<(), PatchOutputRecoveryError> {
    let body = serde_json::to_string_pretty(journal)
        .map_err(|error| PatchOutputRecoveryError::InvalidJournal(error.to_string()))?
        + "\n";
    crate::atomic_write_text(path, &body)
        .map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))
}

fn transition(
    journal_path: &Path,
    journal: &mut PatchOutputJournal,
    state: PatchOutputState,
    checkpoint: PatchOutputCheckpoint,
) -> Result<(), PatchOutputRecoveryError> {
    journal.state = state;
    journal.updated_at_unix = now();
    journal.checkpoints.push(checkpoint);
    write_journal(journal_path, journal)
}

#[cfg(test)]
thread_local! {
    static INJECTED_CHECKPOINT: std::cell::Cell<Option<PatchOutputCheckpoint>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn inject_checkpoint_failure(checkpoint: Option<PatchOutputCheckpoint>) {
    INJECTED_CHECKPOINT.with(|cell| cell.set(checkpoint));
}

fn checkpoint(checkpoint: PatchOutputCheckpoint) -> Result<(), PatchOutputRecoveryError> {
    #[cfg(not(test))]
    let _ = checkpoint;
    #[cfg(test)]
    if INJECTED_CHECKPOINT.with(|cell| cell.get() == Some(checkpoint)) {
        return Err(PatchOutputRecoveryError::Interrupted(format!(
            "after {checkpoint:?}"
        )));
    }
    Ok(())
}

fn verify_file(
    path: &Path,
    expected_size: Option<u64>,
    expected_hash: Option<&str>,
) -> Result<(u64, String), PatchOutputRecoveryError> {
    let actual = digest_path(path)?;
    if expected_size.is_some_and(|size| size != actual.0) {
        return Err(PatchOutputRecoveryError::PreconditionsChanged(
            "output size differs from journal".into(),
        ));
    }
    if expected_hash.is_some_and(|hash| hash != actual.1) {
        return Err(PatchOutputRecoveryError::PreconditionsChanged(
            "output hash differs from journal".into(),
        ));
    }
    Ok(actual)
}

/// Execute the byte-producing applier only after durable intent exists. The
/// finalizer is called after the published output is verified and before the
/// journal becomes `Completed` (used for the existing provenance sidecar).
pub fn publish_durable_patch_output<F, G>(
    intent: PatchOutputIntent,
    produce: F,
    finalize: G,
) -> Result<DurablePatchOutputResult, PatchOutputRecoveryError>
where
    F: FnOnce() -> Result<PreparedPatchOutput, PatchOutputRecoveryError>,
    G: FnOnce(&PreparedPatchOutput) -> Result<(), PatchOutputRecoveryError>,
{
    let journal_path = journal_path_for(&intent, &operation_id(&intent))?;
    let id = journal_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("operation")
        .to_string();
    let temporary_path = temporary_path(&intent, &id)?;
    ensure_sibling(&intent.destination_path, &journal_path)?;
    ensure_sibling(&intent.destination_path, &temporary_path)?;
    if fs::symlink_metadata(&intent.destination_path).is_ok() {
        return Err(PatchOutputRecoveryError::Unsafe(
            "destination already exists; no-clobber is required".into(),
        ));
    }
    let timestamp = now();
    let mut journal = PatchOutputJournal {
        schema_version: PATCH_OUTPUT_JOURNAL_SCHEMA_VERSION,
        operation_id: id,
        created_at_unix: timestamp,
        updated_at_unix: timestamp,
        patch_format: intent.patch_format,
        source_path: intent.source_path,
        source_size: intent.source_size,
        source_sha256: intent.source_sha256,
        patch_path: intent.patch_path,
        patch_size: intent.patch_size,
        patch_sha256: intent.patch_sha256,
        destination_path: intent.destination_path,
        temporary_output_path: temporary_path,
        provenance_path: intent.provenance_path,
        provenance_sha256: None,
        expected_output_size: intent.expected_output_size,
        expected_output_sha256: None,
        destination_preexisting: false,
        previous_destination_sha256: None,
        backup_path: None,
        state: PatchOutputState::Planned,
        checkpoints: vec![PatchOutputCheckpoint::IntentDurable],
        failure_reason: None,
    };
    write_journal(&journal_path, &journal)?;
    checkpoint(PatchOutputCheckpoint::IntentDurable)?;
    transition(
        &journal_path,
        &mut journal,
        PatchOutputState::Preparing,
        PatchOutputCheckpoint::Preparing,
    )?;
    checkpoint(PatchOutputCheckpoint::Preparing)?;

    let prepared = match produce() {
        Ok(value) => value,
        Err(error) => {
            journal.failure_reason = Some(error.to_string());
            let _ = transition(
                &journal_path,
                &mut journal,
                PatchOutputState::Failed,
                PatchOutputCheckpoint::Preparing,
            );
            return Err(error);
        }
    };
    let output_hash = digest_bytes(&prepared.bytes);
    journal.expected_output_size = Some(prepared.bytes.len() as u64);
    journal.expected_output_sha256 = Some(output_hash.clone());
    transition(
        &journal_path,
        &mut journal,
        PatchOutputState::Prepared,
        PatchOutputCheckpoint::Prepared,
    )?;
    checkpoint(PatchOutputCheckpoint::Prepared)?;
    transition(
        &journal_path,
        &mut journal,
        PatchOutputState::VerifyingTemporary,
        PatchOutputCheckpoint::VerifyingTemporary,
    )?;
    if let Err(error) = (|| {
        checkpoint(PatchOutputCheckpoint::TemporaryWriteStarted)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&journal.temporary_output_path)
            .map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))?;
        use std::io::Write;
        file.write_all(&prepared.bytes)
            .map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))?;
        file.sync_all()
            .map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))?;
        checkpoint(PatchOutputCheckpoint::TemporaryWritten)?;
        verify_file(
            &journal.temporary_output_path,
            journal.expected_output_size,
            journal.expected_output_sha256.as_deref(),
        )?;
        checkpoint(PatchOutputCheckpoint::TemporaryVerified)?;
        Ok::<(), PatchOutputRecoveryError>(())
    })() {
        if matches!(error, PatchOutputRecoveryError::Interrupted(_)) {
            return Err(error);
        }
        journal.failure_reason = Some(error.to_string());
        let _ = transition(
            &journal_path,
            &mut journal,
            PatchOutputState::Failed,
            PatchOutputCheckpoint::VerifyingTemporary,
        );
        return Err(error);
    }
    transition(
        &journal_path,
        &mut journal,
        PatchOutputState::Publishing,
        PatchOutputCheckpoint::BeforePublish,
    )?;
    checkpoint(PatchOutputCheckpoint::BeforePublish)?;
    if let Err(error) = fs::hard_link(&journal.temporary_output_path, &journal.destination_path) {
        journal.failure_reason = Some(error.to_string());
        let _ = transition(
            &journal_path,
            &mut journal,
            PatchOutputState::Failed,
            PatchOutputCheckpoint::BeforePublish,
        );
        return Err(PatchOutputRecoveryError::Io(error.to_string()));
    }
    transition(
        &journal_path,
        &mut journal,
        PatchOutputState::Published,
        PatchOutputCheckpoint::Published,
    )?;
    checkpoint(PatchOutputCheckpoint::Published)?;
    transition(
        &journal_path,
        &mut journal,
        PatchOutputState::VerifyingPublished,
        PatchOutputCheckpoint::BeforePublishedVerification,
    )?;
    checkpoint(PatchOutputCheckpoint::BeforePublishedVerification)?;
    verify_file(
        &journal.destination_path,
        journal.expected_output_size,
        journal.expected_output_sha256.as_deref(),
    )?;
    checkpoint(PatchOutputCheckpoint::PublishedVerified)?;
    finalize(&prepared)?;
    journal.provenance_sha256 = Some(digest_path(&journal.provenance_path)?.1);
    checkpoint(PatchOutputCheckpoint::BeforeCompleted)?;
    transition(
        &journal_path,
        &mut journal,
        PatchOutputState::Completed,
        PatchOutputCheckpoint::BeforeCompleted,
    )?;
    remove_owned_artifact(&journal.temporary_output_path)?;
    Ok(DurablePatchOutputResult {
        journal_path,
        output_sha256: output_hash,
        output_size: prepared.bytes.len() as u64,
    })
}

fn remove_owned_artifact(path: &Path) -> Result<(), PatchOutputRecoveryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))
        }
        Ok(_) => Err(PatchOutputRecoveryError::Unsafe(format!(
            "refusing to remove non-regular artifact: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PatchOutputRecoveryError::Io(error.to_string())),
    }
}

fn read_journal(path: &Path) -> Result<PatchOutputJournal, PatchOutputRecoveryError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_PATCH_OUTPUT_JOURNAL_BYTES
    {
        return Err(PatchOutputRecoveryError::InvalidJournal(
            "journal is not a bounded regular file".into(),
        ));
    }
    let bytes = fs::read(path).map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| PatchOutputRecoveryError::InvalidJournal(error.to_string()))?;
    let schema = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            PatchOutputRecoveryError::InvalidJournal("schema_version is missing".into())
        })?;
    if schema != PATCH_OUTPUT_JOURNAL_SCHEMA_VERSION as u64 {
        return Err(PatchOutputRecoveryError::UnknownSchema(schema as u32));
    }
    let journal: PatchOutputJournal = serde_json::from_value(value)
        .map_err(|error| PatchOutputRecoveryError::InvalidJournal(error.to_string()))?;
    validate_journal_paths(path, &journal)?;
    Ok(journal)
}

fn validate_journal_paths(
    journal_path: &Path,
    journal: &PatchOutputJournal,
) -> Result<(), PatchOutputRecoveryError> {
    let journal_parent = journal_path
        .parent()
        .ok_or_else(|| PatchOutputRecoveryError::InvalidJournal("journal has no parent".into()))?;
    let destination_parent = journal.destination_path.parent().ok_or_else(|| {
        PatchOutputRecoveryError::InvalidJournal("destination has no parent".into())
    })?;
    if destination_parent != journal_parent
        || journal.temporary_output_path.parent() != Some(destination_parent)
        || journal.provenance_path.parent() != Some(destination_parent)
        || !journal.destination_path.is_absolute()
    {
        return Err(PatchOutputRecoveryError::InvalidJournal(
            "journal artifacts are not confined beside the destination".into(),
        ));
    }
    Ok(())
}

fn matches_expected(path: &Path, size: Option<u64>, hash: Option<&str>) -> bool {
    verify_file(path, size, hash).is_ok()
}

pub fn inspect_patch_output_operation(
    path: &Path,
) -> Result<PatchOutputRecoveryInspection, PatchOutputRecoveryError> {
    let journal = read_journal(path)?;
    let source_matches = matches_expected(
        &journal.source_path,
        Some(journal.source_size),
        Some(&journal.source_sha256),
    );
    let patch_matches = matches_expected(
        &journal.patch_path,
        Some(journal.patch_size),
        Some(&journal.patch_sha256),
    );
    let temporary_matches = matches_expected(
        &journal.temporary_output_path,
        journal.expected_output_size,
        journal.expected_output_sha256.as_deref(),
    );
    let destination_matches = matches_expected(
        &journal.destination_path,
        journal.expected_output_size,
        journal.expected_output_sha256.as_deref(),
    );
    let provenance_present = fs::symlink_metadata(&journal.provenance_path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink());
    let (classification, explanation) = if journal.state.is_terminal() {
        (
            if journal.state == PatchOutputState::Completed {
                PatchOutputRecoveryClassification::Completed
            } else {
                PatchOutputRecoveryClassification::RolledBack
            },
            "terminal history".into(),
        )
    } else if !source_matches || !patch_matches {
        (
            PatchOutputRecoveryClassification::UnsafeToResume,
            "source or patch changed since the durable plan".into(),
        )
    } else if matches!(
        journal.state,
        PatchOutputState::Publishing
            | PatchOutputState::Published
            | PatchOutputState::VerifyingPublished
    ) && destination_matches
        && provenance_present
    {
        (
            PatchOutputRecoveryClassification::PublicationLikelyComplete,
            "destination exactly matches the verified prepared output".into(),
        )
    } else if matches!(
        journal.state,
        PatchOutputState::Prepared
            | PatchOutputState::VerifyingTemporary
            | PatchOutputState::Publishing
    ) && temporary_matches
        && !fs::symlink_metadata(&journal.destination_path).is_ok()
    {
        (
            PatchOutputRecoveryClassification::SafeToResume,
            "verified temporary output is present and destination is absent".into(),
        )
    } else if journal.state == PatchOutputState::Failed {
        (
            PatchOutputRecoveryClassification::Failed,
            journal
                .failure_reason
                .clone()
                .unwrap_or_else(|| "operation failed before completion".into()),
        )
    } else {
        (
            PatchOutputRecoveryClassification::NeedsReview,
            "filesystem state does not match a safe recovery case".into(),
        )
    };
    Ok(PatchOutputRecoveryInspection {
        journal_path: path.to_path_buf(),
        journal,
        classification,
        explanation,
        source_matches,
        patch_matches,
        temporary_matches,
        destination_matches,
    })
}

pub fn plan_patch_output_recovery(
    path: &Path,
) -> Result<PatchOutputRecoveryPlan, PatchOutputRecoveryError> {
    let inspection = inspect_patch_output_operation(path)?;
    let token = recovery_token(&inspection);
    Ok(PatchOutputRecoveryPlan {
        journal_path: path.to_path_buf(),
        operation_id: inspection.journal.operation_id.clone(),
        classification: inspection.classification,
        precondition_token: token,
    })
}

fn recovery_token(inspection: &PatchOutputRecoveryInspection) -> String {
    let mut hasher = Sha256::new();
    hasher.update(inspection.journal.operation_id.as_bytes());
    hasher.update(inspection.journal.state.serialize_to_string().as_bytes());
    if inspection.destination_matches {
        hasher.update(b"destination-match" as &[u8]);
    } else {
        hasher.update(b"destination-diff" as &[u8]);
    }
    hex_digest(&hasher.finalize())
}

trait StateString {
    fn serialize_to_string(self) -> String;
}
impl StateString for PatchOutputState {
    fn serialize_to_string(self) -> String {
        serde_json::to_string(&self).unwrap_or_default()
    }
}

fn verify_plan(
    plan: &PatchOutputRecoveryPlan,
) -> Result<(PatchOutputRecoveryInspection, PatchOutputJournal), PatchOutputRecoveryError> {
    let inspection = inspect_patch_output_operation(&plan.journal_path)?;
    if recovery_token(&inspection) != plan.precondition_token
        || inspection.journal.operation_id != plan.operation_id
    {
        return Err(PatchOutputRecoveryError::PreconditionsChanged(
            "reviewed recovery plan is stale".into(),
        ));
    }
    Ok((inspection, read_journal(&plan.journal_path)?))
}

/// Explicitly publish an already verified temporary output. It never reruns a
/// patcher and revalidates source, patch, temp, and destination immediately.
pub fn resume_patch_output(plan: &PatchOutputRecoveryPlan) -> Result<(), PatchOutputRecoveryError> {
    let (inspection, mut journal) = verify_plan(plan)?;
    if inspection.classification == PatchOutputRecoveryClassification::PublicationLikelyComplete {
        journal.provenance_sha256 = Some(digest_path(&journal.provenance_path)?.1);
        transition(
            &plan.journal_path,
            &mut journal,
            PatchOutputState::Completed,
            PatchOutputCheckpoint::BeforeCompleted,
        )?;
        remove_owned_artifact(&journal.temporary_output_path)?;
        return Ok(());
    }
    if inspection.classification != PatchOutputRecoveryClassification::SafeToResume {
        return Err(PatchOutputRecoveryError::Unsafe(
            "recovery is not classified safe to resume".into(),
        ));
    }
    transition(
        &plan.journal_path,
        &mut journal,
        PatchOutputState::Publishing,
        PatchOutputCheckpoint::BeforePublish,
    )?;
    fs::hard_link(&journal.temporary_output_path, &journal.destination_path)
        .map_err(|error| PatchOutputRecoveryError::Io(error.to_string()))?;
    transition(
        &plan.journal_path,
        &mut journal,
        PatchOutputState::Published,
        PatchOutputCheckpoint::Published,
    )?;
    verify_file(
        &journal.destination_path,
        journal.expected_output_size,
        journal.expected_output_sha256.as_deref(),
    )?;
    transition(
        &plan.journal_path,
        &mut journal,
        PatchOutputState::VerifyingPublished,
        PatchOutputCheckpoint::PublishedVerified,
    )?;
    journal.provenance_sha256 = Some(digest_path(&journal.provenance_path)?.1);
    transition(
        &plan.journal_path,
        &mut journal,
        PatchOutputState::Completed,
        PatchOutputCheckpoint::BeforeCompleted,
    )?;
    remove_owned_artifact(&journal.temporary_output_path)
}

pub fn rollback_patch_output(
    plan: &PatchOutputRecoveryPlan,
) -> Result<(), PatchOutputRecoveryError> {
    let (inspection, mut journal) = verify_plan(plan)?;
    if !matches!(
        inspection.classification,
        PatchOutputRecoveryClassification::PublicationLikelyComplete
    ) || journal.destination_preexisting
    {
        return Err(PatchOutputRecoveryError::Unsafe(
            "rollback is not safe for this operation".into(),
        ));
    }
    if !inspection.destination_matches {
        return Err(PatchOutputRecoveryError::PreconditionsChanged(
            "destination no longer matches EmuWiz output".into(),
        ));
    }
    transition(
        &plan.journal_path,
        &mut journal,
        PatchOutputState::RollingBack,
        PatchOutputCheckpoint::RollbackStarted,
    )?;
    checkpoint(PatchOutputCheckpoint::RollbackStarted)?;
    let rollback_result = (|| {
        remove_owned_artifact(&journal.destination_path)?;
        checkpoint(PatchOutputCheckpoint::RollbackMutated)?;
        remove_owned_artifact(&journal.temporary_output_path)?;
        if journal
            .provenance_sha256
            .as_deref()
            .is_some_and(|expected| {
                matches_expected(&journal.provenance_path, None, Some(expected))
            })
        {
            remove_owned_artifact(&journal.provenance_path)?;
        }
        checkpoint(PatchOutputCheckpoint::RollbackCompleted)
    })();
    if let Err(error) = rollback_result {
        journal.failure_reason = Some(error.to_string());
        let _ = transition(
            &plan.journal_path,
            &mut journal,
            PatchOutputState::RollbackFailed,
            PatchOutputCheckpoint::RollbackMutated,
        );
        return Err(error);
    }
    transition(
        &plan.journal_path,
        &mut journal,
        PatchOutputState::RolledBack,
        PatchOutputCheckpoint::RollbackCompleted,
    )
}

pub fn discover_pending_patch_outputs(
    directory: &Path,
) -> (Vec<PatchOutputRecoveryInspection>, Vec<String>) {
    let mut paths = fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(JOURNAL_PREFIX) && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.truncate(MAX_PATCH_OUTPUT_JOURNALS);
    let mut found = Vec::new();
    let mut problems = Vec::new();
    for path in paths {
        match inspect_patch_output_operation(&path) {
            Ok(inspection) => {
                if !inspection.journal.state.is_terminal() {
                    found.push(inspection);
                }
            }
            Err(error) => problems.push(format!("{}: {error}", path.display())),
        }
    }
    (found, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(root: &Path) -> PatchOutputIntent {
        let source = root.join("source.bin");
        let patch = root.join("patch.ips");
        let destination = root.join("output.bin");
        fs::write(&source, b"source-bytes").unwrap();
        fs::write(&patch, b"patch-bytes").unwrap();
        PatchOutputIntent {
            patch_format: StandalonePatchFormat::Ips,
            source_path: source.clone(),
            source_size: 12,
            source_sha256: digest_bytes(b"source-bytes"),
            patch_path: patch.clone(),
            patch_size: 11,
            patch_sha256: digest_bytes(b"patch-bytes"),
            destination_path: destination.clone(),
            expected_output_size: Some(6),
            provenance_path: root.join("output.bin.emuwiz-patch.json"),
        }
    }

    fn publish(root: &Path) -> DurablePatchOutputResult {
        publish_durable_patch_output(
            intent(root),
            || {
                Ok(PreparedPatchOutput {
                    bytes: b"output".to_vec(),
                    application: "fixture".into(),
                })
            },
            |prepared| {
                fs::write(
                    root.join("output.bin.emuwiz-patch.json"),
                    digest_bytes(&prepared.bytes),
                )
                .unwrap();
                Ok(())
            },
        )
        .unwrap()
    }

    #[test]
    fn happy_path_is_durable_and_cleans_only_owned_temp() {
        let directory = tempfile::tempdir().unwrap();
        intent(directory.path());
        let source_before = fs::read(directory.path().join("source.bin")).unwrap();
        let result = publish(directory.path());
        let inspection = inspect_patch_output_operation(&result.journal_path).unwrap();
        assert_eq!(
            inspection.classification,
            PatchOutputRecoveryClassification::Completed
        );
        assert_eq!(
            fs::read(directory.path().join("source.bin")).unwrap(),
            source_before
        );
        assert_eq!(
            fs::read(directory.path().join("output.bin")).unwrap(),
            b"output"
        );
        assert!(!fs::read_dir(directory.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn interruption_checkpoints_leave_truthful_journals() {
        let checkpoints = [
            PatchOutputCheckpoint::IntentDurable,
            PatchOutputCheckpoint::Preparing,
            PatchOutputCheckpoint::Prepared,
            PatchOutputCheckpoint::TemporaryWriteStarted,
            PatchOutputCheckpoint::TemporaryWritten,
            PatchOutputCheckpoint::TemporaryVerified,
            PatchOutputCheckpoint::BeforePublish,
            PatchOutputCheckpoint::Published,
            PatchOutputCheckpoint::BeforePublishedVerification,
            PatchOutputCheckpoint::PublishedVerified,
            PatchOutputCheckpoint::BeforeCompleted,
        ];
        for checkpoint_value in checkpoints {
            let directory = tempfile::tempdir().unwrap();
            let operation_intent = intent(directory.path());
            let provenance_path = operation_intent.provenance_path.clone();
            inject_checkpoint_failure(Some(checkpoint_value));
            let result = publish_durable_patch_output(
                operation_intent,
                || {
                    Ok(PreparedPatchOutput {
                        bytes: b"output".to_vec(),
                        application: "fixture".into(),
                    })
                },
                |prepared| {
                    fs::write(&provenance_path, digest_bytes(&prepared.bytes)).unwrap();
                    Ok(())
                },
            );
            inject_checkpoint_failure(None);
            assert!(
                matches!(result, Err(PatchOutputRecoveryError::Interrupted(_))),
                "{checkpoint_value:?}"
            );
            let (pending, problems) = discover_pending_patch_outputs(directory.path());
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(
                pending.len(),
                1,
                "checkpoint {checkpoint_value:?} must remain inspectable"
            );
            assert_eq!(
                fs::read(directory.path().join("source.bin")).unwrap(),
                b"source-bytes"
            );
        }
    }

    #[test]
    fn prepared_temp_can_be_explicitly_resumed_only_with_matching_inputs() {
        let directory = tempfile::tempdir().unwrap();
        inject_checkpoint_failure(Some(PatchOutputCheckpoint::TemporaryWritten));
        let result = publish_durable_patch_output(
            intent(directory.path()),
            || {
                Ok(PreparedPatchOutput {
                    bytes: b"output".to_vec(),
                    application: "fixture".into(),
                })
            },
            |_prepared| Ok(()),
        );
        inject_checkpoint_failure(None);
        assert!(result.is_err());
        let journal = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry.file_name().to_string_lossy().ends_with(".json")
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(JOURNAL_PREFIX)
            })
            .unwrap()
            .path();
        let inspection = inspect_patch_output_operation(&journal).unwrap();
        assert_eq!(
            inspection.classification,
            PatchOutputRecoveryClassification::SafeToResume
        );
        fs::write(
            directory.path().join("output.bin.emuwiz-patch.json"),
            b"fixture provenance",
        )
        .unwrap();
        let plan = plan_patch_output_recovery(&journal).unwrap();
        resume_patch_output(&plan).unwrap();
        assert_eq!(
            inspect_patch_output_operation(&journal)
                .unwrap()
                .classification,
            PatchOutputRecoveryClassification::Completed
        );

        let changed = tempfile::tempdir().unwrap();
        inject_checkpoint_failure(Some(PatchOutputCheckpoint::TemporaryWritten));
        let interrupted = publish_durable_patch_output(
            intent(changed.path()),
            || {
                Ok(PreparedPatchOutput {
                    bytes: b"output".to_vec(),
                    application: "fixture".into(),
                })
            },
            |_prepared| Ok(()),
        );
        inject_checkpoint_failure(None);
        assert!(interrupted.is_err());
        let changed_journal = fs::read_dir(changed.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry.file_name().to_string_lossy().ends_with(".json")
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(JOURNAL_PREFIX)
            })
            .unwrap()
            .path();
        fs::write(changed.path().join("source.bin"), b"changed").unwrap();
        assert_eq!(
            inspect_patch_output_operation(&changed_journal)
                .unwrap()
                .classification,
            PatchOutputRecoveryClassification::UnsafeToResume
        );
    }

    #[test]
    fn rollback_removes_only_an_unchanged_published_output() {
        let directory = tempfile::tempdir().unwrap();
        let result = publish(directory.path());
        let mut journal = read_journal(&result.journal_path).unwrap();
        journal.state = PatchOutputState::Published;
        write_journal(&result.journal_path, &journal).unwrap();
        let plan = plan_patch_output_recovery(&result.journal_path).unwrap();
        rollback_patch_output(&plan).unwrap();
        assert!(!directory.path().join("output.bin").exists());
        assert_eq!(
            inspect_patch_output_operation(&result.journal_path)
                .unwrap()
                .classification,
            PatchOutputRecoveryClassification::RolledBack
        );

        let changed = tempfile::tempdir().unwrap();
        let result = publish(changed.path());
        let mut journal = read_journal(&result.journal_path).unwrap();
        journal.state = PatchOutputState::Published;
        write_journal(&result.journal_path, &journal).unwrap();
        fs::write(changed.path().join("output.bin"), b"user changed").unwrap();
        let plan = plan_patch_output_recovery(&result.journal_path).unwrap();
        assert!(matches!(
            rollback_patch_output(&plan),
            Err(PatchOutputRecoveryError::Unsafe(_))
                | Err(PatchOutputRecoveryError::PreconditionsChanged(_))
        ));
        assert_eq!(
            fs::read(changed.path().join("output.bin")).unwrap(),
            b"user changed"
        );
    }

    #[test]
    fn unknown_schema_is_never_recovered() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".emuwiz-patch-output-future.json");
        fs::write(&path, r#"{"schema_version":999}"#).unwrap();
        let (_pending, problems) = discover_pending_patch_outputs(directory.path());
        assert_eq!(problems.len(), 1);
        assert!(matches!(
            inspect_patch_output_operation(&path),
            Err(PatchOutputRecoveryError::UnknownSchema(999))
        ));
    }
}
