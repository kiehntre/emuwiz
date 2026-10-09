//! Channel-aware update discovery and explicitly reviewed portable updates.
//! Executable moves use hash/identity evidence, no-clobber publication, durable
//! journals and shared kernel locks. Backup and displaced images are retained.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod process_probe;
mod safety;
pub use safety::FileIdentity;

use crate::emulator_inventory::{
    BuildChannel, EmulatorInstallation, InstallationType, InventoryEmulator, SaveStateRisk,
    UpdateCapability,
};

pub const MAX_METADATA_BYTES: usize = 64 * 1024;
pub const METADATA_TIMEOUT: Duration = Duration::from_secs(5);
pub const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
pub const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;
pub const UPDATE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable,
    InstalledNewer,
    VersionUnknown,
    LatestUnknown,
    ChannelMismatch,
    ComparisonUnsupported,
    Offline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateMetadataSource {
    OfficialReleaseApi,
    PackageMetadata,
    FlatpakMetadata,
    LocalCache,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AvailableVersion {
    pub version: String,
    pub channel: BuildChannel,
    pub source: UpdateMetadataSource,
    pub provenance: String,
    pub checked_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateResult {
    pub emulator: InventoryEmulator,
    pub executable_path: std::path::PathBuf,
    pub installed_version: Option<String>,
    pub installed_channel: BuildChannel,
    pub available_version: Option<String>,
    pub available_channel: BuildChannel,
    pub status: UpdateStatus,
    pub source: UpdateMetadataSource,
    pub provenance: String,
    pub checked_unix_seconds: u64,
    pub warning: Option<String>,
    pub save_state_warning: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateReport {
    pub results: Vec<UpdateResult>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateExecutionEligibility {
    Ready,
    RunningBlocked,
    /// Whether the emulator is running could not be established (for example
    /// processes EmuWiz may not inspect). Distinct from a running emulator.
    QuiescenceUnknown,
    UnsupportedInstallType,
    VersionUnknown,
    StaleMetadata,
    InvalidTarget,
    VerificationUnavailable,
    ReviewRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateVerificationLevel {
    Sha256,
    Unverified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateTransactionState {
    Planned,
    Downloading,
    Verifying,
    Staged,
    Published,
    Failed,
    RolledBack,
    NeedsReconciliation,
    Stale,
    /// Durable intent recorded; the executable has not moved yet.
    Applying,
    /// The old executable is preserved at the backup path; not yet published.
    BackupMoved,
    /// An undo is in flight (intent recorded before bytes move).
    Undoing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateArtifact {
    pub version: String,
    pub channel: BuildChannel,
    pub url: String,
    pub sha256: Option<String>,
    pub source: UpdateMetadataSource,
    pub provenance: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateExecutionPlan {
    pub transaction_id: String,
    pub emulator: InventoryEmulator,
    pub installation_type: InstallationType,
    pub target_path: PathBuf,
    pub target_sha256: String,
    pub installed_version: String,
    pub installed_channel: BuildChannel,
    pub new_version: String,
    pub new_channel: BuildChannel,
    pub artifact: UpdateArtifact,
    pub verification: UpdateVerificationLevel,
    pub rollback_path: PathBuf,
    pub eligibility: UpdateExecutionEligibility,
    pub save_state_warning: bool,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateJournal {
    pub transaction_id: String,
    pub emulator: InventoryEmulator,
    pub target_path: PathBuf,
    pub rollback_path: PathBuf,
    pub old_version: String,
    pub new_version: String,
    pub channel: BuildChannel,
    pub source_url: String,
    pub provenance: String,
    pub verification: UpdateVerificationLevel,
    pub staged_path: Option<PathBuf>,
    pub state: UpdateTransactionState,
    pub failure: Option<String>,
    /// SHA-256 of the executable that was reviewed and replaced.
    pub original_sha256: String,
    /// SHA-256 of the verified artifact that was published.
    pub published_sha256: String,
    /// Where an undo parked the published executable (never deleted).
    pub displaced_path: Option<PathBuf>,
    /// Monotonic per-target creation order, assigned while holding its lock.
    /// Missing on legacy records; ambiguous legacy histories require review.
    #[serde(default)]
    pub sequence: Option<u64>,
    #[serde(default)]
    pub root_binding: Option<crate::catalogue_health::SourceRootBinding>,
    #[serde(default)]
    pub original_identity: Option<FileIdentity>,
    #[serde(default)]
    pub target_parent_binding: Option<crate::catalogue_health::SourceRootBinding>,
    #[serde(default)]
    pub staged_identity: Option<FileIdentity>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UpdateExecutionError {
    Ineligible(UpdateExecutionEligibility),
    Stale,
    Download(String),
    Verification(String),
    Io(String),
    NeedsReconciliation(String),
    /// The published executable no longer matches the recorded update.
    TargetChanged(String),
    /// The preserved original no longer matches the recorded original.
    BackupChanged(String),
    /// Running state could not be established; unknown is not stopped.
    QuiescenceUnknown,
    /// Another update or an unrecovered interrupted one holds the lock.
    Concurrent(String),
    /// The durable operation record is missing, corrupt or inconsistent.
    Record(String),
    /// The executable itself is unsafe to mutate (for example several hard
    /// links). Refused before anything was changed.
    UnsafeTarget(String),
}

impl std::fmt::Display for UpdateExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ineligible(reason) => write!(f, "update is not executable: {reason:?}"),
            Self::Stale => f.write_str("update preview is stale; review it again"),
            Self::Download(message) => write!(f, "update download failed: {message}"),
            Self::Verification(message) => write!(f, "update verification failed: {message}"),
            Self::Io(message) => write!(f, "update filesystem operation failed: {message}"),
            Self::NeedsReconciliation(message) => {
                write!(f, "update needs reconciliation: {message}")
            }
            Self::TargetChanged(message) => write!(f, "undo refused, executable changed: {message}"),
            Self::BackupChanged(message) => write!(f, "undo refused, backup changed: {message}"),
            Self::QuiescenceUnknown => f.write_str(
                "could not establish that the emulator is stopped; unknown is not treated as stopped",
            ),
            Self::Concurrent(message) => write!(f, "another update is in progress: {message}"),
            Self::Record(message) => write!(f, "operation record problem: {message}"),
            Self::UnsafeTarget(message) => write!(f, "update refused: {message}"),
        }
    }
}

impl std::error::Error for UpdateExecutionError {}

pub trait UpdateDownloader {
    fn download(&mut self, url: &str, destination: &mut File) -> Result<(), UpdateExecutionError>;
}

#[derive(Default)]
pub struct HttpsUpdateDownloader;

impl UpdateDownloader for HttpsUpdateDownloader {
    fn download(&mut self, url: &str, destination: &mut File) -> Result<(), UpdateExecutionError> {
        if !url.starts_with("https://") {
            return Err(UpdateExecutionError::Download(
                "only HTTPS sources are allowed".into(),
            ));
        }
        let agent = crate::http_agent::config(|builder| {
            builder
                .https_only(true)
                .max_redirects(0)
                .timeout_global(Some(UPDATE_TIMEOUT))
        })
        .new_agent();
        let mut response = agent
            .get(url)
            .call()
            .map_err(|error| UpdateExecutionError::Download(error.to_string()))?;
        if !(200..300).contains(&response.status().as_u16()) {
            return Err(UpdateExecutionError::Download(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let output = destination;
        let mut limited = response.body_mut().as_reader().take(MAX_UPDATE_BYTES + 1);
        let copied = std::io::copy(&mut limited, output)
            .map_err(|error| UpdateExecutionError::Download(error.to_string()))?;
        if copied > MAX_UPDATE_BYTES {
            return Err(UpdateExecutionError::Download(
                "download exceeded size limit".into(),
            ));
        }
        output
            .flush()
            .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
        output
            .sync_all()
            .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
        Ok(())
    }
}

fn update_transaction_id(path: &Path, version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(version.as_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..24]
        .to_string()
}

/// Evidence that the emulator is not using the executable.  Only `Stopped`
/// permits a replacement or an undo; `Unknown` is deliberately NOT treated
/// as stopped.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum QuiescenceEvidence {
    Stopped,
    Running,
    Unknown,
}

impl QuiescenceEvidence {
    /// Combines two independent observations: any `Running` wins, then any
    /// `Unknown`; `Stopped` only when both say so.
    pub fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Running, _) | (_, Self::Running) => Self::Running,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::Stopped,
        }
    }

    pub fn from_tracked_flag(running: bool) -> Self {
        if running {
            Self::Running
        } else {
            Self::Stopped
        }
    }
}

/// Fail-closed process evidence. A process may start after the last probe;
/// the advisory update lock cannot prevent external emulator launches.
pub fn probe_executable_quiescence(executable: &Path) -> QuiescenceEvidence {
    probe_executable_quiescence_at(executable, Path::new("/proc"))
}
/// Synthetic proc root support; reads exe identity, argv[0] and necessary stat
/// evidence only. Environments are never inspected.
pub fn probe_executable_quiescence_at(executable: &Path, proc_root: &Path) -> QuiescenceEvidence {
    process_probe::normal(executable, proc_root)
}
/// Recovery can inspect a missing install path using its hash-proven backup.
pub fn probe_update_quiescence(journal: &UpdateJournal) -> QuiescenceEvidence {
    process_probe::recorded(journal, Path::new("/proc"))
}

fn require_stopped(evidence: QuiescenceEvidence) -> Result<(), UpdateExecutionError> {
    match evidence {
        QuiescenceEvidence::Stopped => Ok(()),
        QuiescenceEvidence::Running => Err(UpdateExecutionError::Ineligible(
            UpdateExecutionEligibility::RunningBlocked,
        )),
        QuiescenceEvidence::Unknown => Err(UpdateExecutionError::QuiescenceUnknown),
    }
}

pub fn plan_staged_update(
    installation: &EmulatorInstallation,
    update: &UpdateResult,
    artifact: UpdateArtifact,
    quiescence: QuiescenceEvidence,
) -> UpdateExecutionPlan {
    let installed_version = installation.version.clone().unwrap_or_default();
    let verification = if artifact.sha256.is_some() {
        UpdateVerificationLevel::Sha256
    } else {
        UpdateVerificationLevel::Unverified
    };
    let target_sha256 = match observe(&installation.executable_path) {
        Ok(Observed::Hash(hash)) => hash,
        _ => String::new(),
    };
    let eligibility = if quiescence == QuiescenceEvidence::Running {
        UpdateExecutionEligibility::RunningBlocked
    } else if quiescence == QuiescenceEvidence::Unknown {
        // Unknown is not stopped, but it is not "running" either.
        UpdateExecutionEligibility::QuiescenceUnknown
    } else if !matches!(
        installation.installation_type,
        InstallationType::AppImage | InstallationType::Portable | InstallationType::Managed
    ) || installation.update_capability != UpdateCapability::PortableManaged
    {
        // Unsupported for either reason: the install kind cannot be updated
        // in place, or this installation is not the portable/managed one.
        UpdateExecutionEligibility::UnsupportedInstallType
    } else if installation.version.is_none() {
        UpdateExecutionEligibility::VersionUnknown
    } else if update.status != UpdateStatus::UpdateAvailable
        || update.available_version.as_deref() != Some(artifact.version.as_str())
        || update.available_channel != artifact.channel
    {
        UpdateExecutionEligibility::StaleMetadata
    } else if target_sha256.is_empty()
        || artifact.url.is_empty()
        || !artifact.url.starts_with("https://")
        || safety::require_single_link(&installation.executable_path).is_err()
    {
        UpdateExecutionEligibility::InvalidTarget
    } else if verification == UpdateVerificationLevel::Unverified {
        UpdateExecutionEligibility::VerificationUnavailable
    } else {
        UpdateExecutionEligibility::Ready
    };
    let transaction_id = update_transaction_id(&installation.executable_path, &artifact.version);
    let rollback_path = installation
        .installation_root
        .join(ROLLBACK_DIR)
        .join(format!(
            "{}-{}-{}",
            installation.emulator.label(),
            installed_version,
            transaction_id
        ));
    UpdateExecutionPlan { transaction_id, emulator: installation.emulator, installation_type: installation.installation_type, target_path: installation.executable_path.clone(), target_sha256, installed_version, installed_channel: installation.channel, new_version: artifact.version.clone(), new_channel: artifact.channel, artifact, verification, rollback_path, eligibility, save_state_warning: update.save_state_warning, warning: (verification == UpdateVerificationLevel::Unverified).then_some("No published checksum/signature was supplied; execution is not safe to claim verified.".into()) }
}

pub const ROLLBACK_DIR: &str = ".emuwiz-update-rollback";

fn io_err(error: impl std::fmt::Display) -> UpdateExecutionError {
    UpdateExecutionError::Io(error.to_string())
}

fn hash_reader(file: &mut File) -> Result<String, UpdateExecutionError> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).map_err(io_err)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(io_err)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > MAX_UPDATE_BYTES {
            return Err(UpdateExecutionError::Verification(
                "staged update exceeds size limit".into(),
            ));
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn hash_file(path: &Path) -> Result<String, UpdateExecutionError> {
    use std::os::unix::fs::MetadataExt;
    let slot = safety::Slot::open(path)?;
    let mut file = slot.read()?;
    let before = file.metadata().map_err(io_err)?;
    if !before.is_file() {
        return Err(io_err("not a regular executable"));
    }
    let hash = hash_reader(&mut file)?;
    let after = file.metadata().map_err(io_err)?;
    let entry = fs::symlink_metadata(slot.path()).map_err(io_err)?;
    let stamp = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if stamp(&before) != stamp(&after) || stamp(&entry) != stamp(&after) {
        return Err(UpdateExecutionError::Stale);
    }
    Ok(hash)
}
fn file_identity(path: &Path) -> Result<FileIdentity, UpdateExecutionError> {
    FileIdentity::of(&safety::Slot::open(path)?.read()?)
}
fn matches_file(
    path: &Path,
    hash: &str,
    identity: Option<FileIdentity>,
) -> Result<bool, UpdateExecutionError> {
    Ok(observe(path)? == Observed::Hash(hash.into())
        && identity.is_none_or(|id| file_identity(path).ok() == Some(id)))
}

/// What is currently at a path, judged by exact content and kind.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Observed {
    Missing,
    /// A symlink, directory or other non-regular entry.
    NotRegular,
    Hash(String),
}

fn observe(path: &Path) -> Result<Observed, UpdateExecutionError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Observed::Missing),
        Err(error) => Err(io_err(error)),
        Ok(metadata) if !metadata.is_file() => Ok(Observed::NotRegular),
        Ok(_) => hash_file(path).map(Observed::Hash),
    }
}

fn describe(observed: &Observed) -> String {
    match observed {
        Observed::Missing => "is missing".into(),
        Observed::NotRegular => "is no longer a regular file".into(),
        Observed::Hash(hash) => format!("now has SHA-256 {}", &hash[..hash.len().min(12)]),
    }
}

fn sync_directory(path: &Path) -> Result<(), UpdateExecutionError> {
    File::open(path).map_err(io_err)?.sync_all().map_err(io_err)
}

/// Atomic no-clobber move: the destination is never overwritten.
fn move_noreplace(source: &Path, destination: &Path) -> Result<(), UpdateExecutionError> {
    safety::move_entry(source, destination)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
enum Fault {
    FailPersist(UpdateTransactionState),
    Crash(UpdateTransactionState),
    FailPublishRename,
    FailRestoreRename,
    CrashBoundary(MutationBoundary),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationBoundary {
    StageCreated,
    OriginalMoved,
    PublishedMoved,
    UndoDisplaced,
    UndoRestored,
    RecoveryMoved,
}
fn crashed_boundary(boundary: MutationBoundary) -> Result<(), UpdateExecutionError> {
    if fault_hit(Fault::CrashBoundary(boundary)) {
        Err(io_err("simulated crash at mutation boundary"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::RefCell<Vec<Fault>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn fault_hit(fault: Fault) -> bool {
    FAULT.with(|cell| cell.borrow().contains(&fault))
}

#[cfg(not(test))]
fn fault_hit(_fault: Fault) -> bool {
    false
}

fn crashed(state: UpdateTransactionState) -> Result<(), UpdateExecutionError> {
    if fault_hit(Fault::Crash(state)) {
        Err(UpdateExecutionError::Io("simulated process crash".into()))
    } else {
        Ok(())
    }
}

impl UpdateJournal {
    /// Everything needed to prove, rather than guess, that the files on disk are
    /// the ones this transaction handled. Records written before these receipts
    /// existed lack them; hash equality alone never establishes ownership, so
    /// such records are never executed or offered as Undo.
    pub fn has_ownership_evidence(&self) -> bool {
        self.sequence.is_some()
            && self.root_binding.is_some()
            && self.target_parent_binding.is_some()
            && self.original_identity.is_some()
            && self.staged_identity.is_some()
    }
    fn directory(&self) -> Result<&Path, UpdateExecutionError> {
        self.rollback_path
            .parent()
            .ok_or_else(|| io_err("rollback path has no parent directory"))
    }

    pub fn record_path(&self) -> Result<PathBuf, UpdateExecutionError> {
        Ok(record_path(self.directory()?, &self.transaction_id))
    }
}

fn record_path(directory: &Path, transaction_id: &str) -> PathBuf {
    directory.join(format!("{transaction_id}.journal.json"))
}

/// Durably records the journal (temp file, fsync, atomic rename of our own
/// record, directory fsync).  Called BEFORE each irreversible step.
fn persist_journal(journal: &UpdateJournal) -> Result<(), UpdateExecutionError> {
    if fault_hit(Fault::FailPersist(journal.state)) {
        return Err(io_err("injected record persistence failure"));
    }
    safety::persist(journal)
}

fn load_journal(path: &Path) -> Result<UpdateJournal, UpdateExecutionError> {
    let slot = safety::Slot::open(path).map_err(|e| UpdateExecutionError::Record(e.to_string()))?;
    let file = slot
        .read()
        .map_err(|e| UpdateExecutionError::Record(e.to_string()))?;
    let metadata = file.metadata().map_err(io_err)?;
    if !metadata.is_file() || metadata.len() > MAX_METADATA_BYTES as u64 {
        return Err(UpdateExecutionError::Record(
            "record is not a bounded regular file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_err)?;
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(UpdateExecutionError::Record(
            "record exceeds size limit".into(),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| UpdateExecutionError::Record(format!("{} is corrupt: {e}", path.display())))
}
fn acquire_lock(
    _directory: &Path,
    target: &Path,
    _transaction_id: &str,
) -> Result<safety::TargetLock, UpdateExecutionError> {
    safety::acquire(target)
}

/// An operation stopped after it had already moved an executable. Say so,
/// keep the durable record in its in-flight state with the cause attached, and
/// require recovery - never "nothing changed".
fn partial_failure(
    journal: &mut UpdateJournal,
    operation: &str,
    cause: &UpdateExecutionError,
) -> UpdateExecutionError {
    let message = format!(
        "{operation} had already moved an executable when it stopped ({cause}); the operation is only partly applied and recovery is required. No executable was deleted."
    );
    journal.failure = Some(message.clone());
    let _ = persist_journal(journal);
    UpdateExecutionError::NeedsReconciliation(message)
}

/// A terminal state is recorded only when the files on disk agree with every
/// recorded identity. Contradictory evidence stays recoverable/review-required.
fn persist_verified_terminal(journal: &UpdateJournal) -> Result<(), UpdateExecutionError> {
    if !terminal_consistent(journal)? {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "the resulting state contradicts the recorded executable identities; it was not recorded as complete and all evidence was preserved".into(),
        ));
    }
    persist_journal(journal).map_err(|e| {
        UpdateExecutionError::NeedsReconciliation(format!(
            "the files are as recorded but the durable record could not be saved: {e}"
        ))
    })
}

fn mark_failed(journal: &mut UpdateJournal, state: UpdateTransactionState, why: &str) {
    journal.state = state;
    journal.failure = Some(why.to_string());
    // Best effort: the failure is already being reported to the caller.
    let _ = persist_journal(journal);
}

/// Failure BEFORE publication: nothing at the target was changed (or was
/// fully restored).  The staging file is ours (txid-named, create_new).
fn fail_before_publication(
    journal: &mut UpdateJournal,
    staging: &Path,
    error: UpdateExecutionError,
) -> UpdateExecutionError {
    if let Err(problem) = retain_staging(journal, staging) {
        mark_failed(
            journal,
            UpdateTransactionState::NeedsReconciliation,
            &problem.to_string(),
        );
        return problem;
    }
    mark_failed(journal, UpdateTransactionState::Failed, &error.to_string());
    error
}

/// Retain positively identified scratch output. Unknown/replaced files stay
/// exactly where they are; no executable or temporary file is ever unlinked.
fn retain_staging(journal: &mut UpdateJournal, staging: &Path) -> Result<(), UpdateExecutionError> {
    let paths = safety::Paths::journal(&journal.record_path()?, journal)?;
    if observe(staging)? == Observed::Missing {
        if journal.staged_identity.is_some()
            && file_identity(&paths.retained).ok() == journal.staged_identity
        {
            journal.staged_path = Some(paths.retained.clone());
        } else if observe(&paths.retained)? == Observed::Missing {
            journal.staged_path = None;
        } else {
            return Err(UpdateExecutionError::NeedsReconciliation(
                "retained staging has contradictory ownership; evidence preserved".into(),
            ));
        }
        return Ok(());
    }
    if journal.staged_identity.is_none() || file_identity(staging).ok() != journal.staged_identity {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "staging ownership changed; the file and evidence were preserved".into(),
        ));
    }
    if staging == paths.retained {
        return Ok(());
    }
    move_noreplace(staging, &paths.retained)?;
    journal.staged_path = Some(paths.retained.clone());
    if file_identity(&paths.retained).ok() != journal.staged_identity {
        let _ = move_noreplace(&paths.retained, staging);
        journal.staged_path = Some(staging.into());
        return Err(UpdateExecutionError::NeedsReconciliation(
            "staging changed during retention; all bytes were preserved".into(),
        ));
    }
    Ok(())
}

/// Failure AFTER the old executable moved but before the new one was
/// published: put the user's file back without overwriting anything.
fn compensate<Q: FnMut() -> QuiescenceEvidence>(
    journal: &mut UpdateJournal,
    staging: &Path,
    cause: UpdateExecutionError,
    paths: &safety::Paths,
    lock: &safety::TargetLock,
    quiescence: &mut Q,
) -> UpdateExecutionError {
    let checked = lock
        .check()
        .and_then(|_| paths.check())
        .and_then(|_| require_stopped(quiescence()));
    let restored = if let Err(error) = checked {
        Err(error)
    } else if matches_file(
        &journal.rollback_path,
        &journal.original_sha256,
        journal.original_identity,
    )
    .ok()
        != Some(true)
    {
        Err(UpdateExecutionError::NeedsReconciliation(
            "preserved original changed before compensation; it was not moved".into(),
        ))
    } else if fault_hit(Fault::FailRestoreRename) {
        Err(io_err("injected restore failure"))
    } else {
        move_noreplace(&journal.rollback_path, &journal.target_path)
    };
    match restored {
        Ok(()) if matches!(observe(&journal.target_path), Ok(Observed::Hash(ref h)) if *h == journal.original_sha256) =>
        {
            let _ = journal.target_path.parent().map(sync_directory);
            fail_before_publication(journal, staging, cause)
        }
        other => {
            let detail = match other {
                Ok(()) => "restored file did not match the original".to_string(),
                Err(error) => error.to_string(),
            };
            let message = format!(
                "{cause}; restoring the previous executable failed ({detail}); it is preserved at {}",
                journal.rollback_path.display()
            );
            mark_failed(
                journal,
                UpdateTransactionState::NeedsReconciliation,
                &message,
            );
            UpdateExecutionError::NeedsReconciliation(message)
        }
    }
}

pub fn execute_staged_update<D, Q>(
    plan: &UpdateExecutionPlan,
    installation: &EmulatorInstallation,
    update: &UpdateResult,
    mut quiescence: Q,
    downloader: &mut D,
) -> Result<UpdateJournal, UpdateExecutionError>
where
    D: UpdateDownloader,
    Q: FnMut() -> QuiescenceEvidence,
{
    if plan.eligibility != UpdateExecutionEligibility::Ready {
        return Err(UpdateExecutionError::Ineligible(plan.eligibility));
    }
    if plan.artifact.sha256.is_none() {
        return Err(UpdateExecutionError::Ineligible(
            UpdateExecutionEligibility::VerificationUnavailable,
        ));
    }
    require_stopped(quiescence())?;
    if installation.emulator != plan.emulator
        || update.executable_path != plan.target_path
        || installation.executable_path != plan.target_path
        || installation.installation_type != plan.installation_type
        || installation.version.as_deref() != Some(plan.installed_version.as_str())
        || installation.channel != plan.installed_channel
        || update.status != UpdateStatus::UpdateAvailable
        || update.available_version.as_deref() != Some(plan.new_version.as_str())
        || update.available_channel != plan.new_channel
    {
        return Err(UpdateExecutionError::Stale);
    }
    if observe(&plan.target_path)? != Observed::Hash(plan.target_sha256.clone()) {
        return Err(UpdateExecutionError::Stale);
    }
    let paths = safety::Paths::new(
        &installation.installation_root,
        &plan.target_path,
        &plan.transaction_id,
        plan.emulator,
        &plan.installed_version,
        &plan.new_version,
    )?;
    if plan.rollback_path != paths.backup {
        return Err(UpdateExecutionError::Record(
            "reviewed backup path is not derived from the installation and transaction".into(),
        ));
    }
    // Refusals that need no filesystem change come BEFORE the first one.
    safety::require_supported_storage(&paths.root)?;
    safety::require_single_link(&plan.target_path)?;
    use std::os::unix::fs::DirBuilderExt;
    match fs::DirBuilder::new().mode(0o700).create(&paths.directory) {
        Ok(()) => sync_directory(&paths.root)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io_err(e)),
    }
    paths.check()?;
    let directory = paths.directory.as_path();
    let lock = acquire_lock(directory, &paths.target, &plan.transaction_id)?;
    lock.check()?;
    paths.check()?;
    // Incomplete prior transactions must be reconciled explicitly. A released
    // kernel lock proves exclusion, not that their filesystem state is safe.
    let prior = discover_update_records(&paths.root);
    if prior.iter().any(|e| {
        e.journal.as_ref().is_err()
            || (e
                .journal
                .as_ref()
                .is_ok_and(|j| j.target_path == paths.target)
                && e.needs_attention())
    }) {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "an earlier installation transaction requires explicit review/recovery".into(),
        ));
    }
    if fs::symlink_metadata(record_path(&paths.directory, &plan.transaction_id)).is_ok() {
        return Err(UpdateExecutionError::Record("this reviewed transaction already has a durable record; create a new preview before retrying".into()));
    }
    remember_installation_root(&paths.root)?;
    let result = apply_update(plan, &paths, &lock, &mut quiescence, downloader);
    result
}

fn apply_update<D, Q>(
    plan: &UpdateExecutionPlan,
    paths: &safety::Paths,
    lock: &safety::TargetLock,
    quiescence: &mut Q,
    downloader: &mut D,
) -> Result<UpdateJournal, UpdateExecutionError>
where
    D: UpdateDownloader,
    Q: FnMut() -> QuiescenceEvidence,
{
    let staging = paths.staging.clone();
    if fs::symlink_metadata(&staging).is_ok() {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "staging path already exists".into(),
        ));
    }
    if fs::symlink_metadata(&plan.rollback_path).is_ok() {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "backup path already exists; it is never overwritten".into(),
        ));
    }
    let mut journal = UpdateJournal {
        transaction_id: plan.transaction_id.clone(),
        emulator: plan.emulator,
        target_path: plan.target_path.clone(),
        rollback_path: plan.rollback_path.clone(),
        old_version: plan.installed_version.clone(),
        new_version: plan.new_version.clone(),
        channel: plan.new_channel,
        source_url: plan.artifact.url.clone(),
        provenance: plan.artifact.provenance.clone(),
        verification: plan.verification,
        staged_path: Some(staging.clone()),
        state: UpdateTransactionState::Planned,
        failure: None,
        original_sha256: plan.target_sha256.clone(),
        published_sha256: plan.artifact.sha256.clone().unwrap_or_default(),
        displaced_path: None,
        sequence: Some(next_sequence(&paths.root, &paths.target)?),
        root_binding: Some(paths.binding.clone()),
        target_parent_binding: Some(paths.parent_binding.clone()),
        original_identity: Some(file_identity(&paths.target)?),
        staged_identity: None,
    };
    // Nothing has changed yet; a persistence failure here is a clean refusal.
    // The core owns the create_new file descriptor, not a pathname returned
    // by a downloader. Its inode receipt is durable before download begins.
    let mut staged_file = safety::Slot::open(&staging)?.create()?;
    journal.staged_identity = Some(FileIdentity::of(&staged_file)?);
    crashed_boundary(MutationBoundary::StageCreated)?;
    if let Err(error) = persist_journal(&journal) {
        return Err(fail_before_publication(&mut journal, &staging, error));
    }
    journal.state = UpdateTransactionState::Downloading;
    persist_journal(&journal).inspect_err(|_| {
        mark_failed(
            &mut journal,
            UpdateTransactionState::Failed,
            "record not saved",
        )
    })?;
    if let Err(error) = downloader.download(&plan.artifact.url, &mut staged_file) {
        return Err(fail_before_publication(&mut journal, &staging, error));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::symlink_metadata(&plan.target_path)
            .map_err(io_err)
            .map(|metadata| metadata.permissions().mode());
        if let Err(error) = mode.and_then(|mode| {
            staged_file
                .set_permissions(fs::Permissions::from_mode(mode))
                .map_err(io_err)
        }) {
            return Err(fail_before_publication(&mut journal, &staging, error));
        }
    }
    journal.state = UpdateTransactionState::Verifying;
    let hash = match hash_reader(&mut staged_file) {
        Ok(hash) => hash,
        Err(error) => return Err(fail_before_publication(&mut journal, &staging, error)),
    };
    if plan.artifact.sha256.as_deref() != Some(hash.as_str()) {
        let error =
            UpdateExecutionError::Verification("SHA-256 does not match published artifact".into());
        return Err(fail_before_publication(&mut journal, &staging, error));
    }
    staged_file.sync_all().map_err(io_err)?;
    if file_identity(&staging).ok() != journal.staged_identity
        || observe(&staging)? != Observed::Hash(hash.clone())
    {
        return Err(fail_before_publication(
            &mut journal,
            &staging,
            UpdateExecutionError::Stale,
        ));
    }
    journal.state = UpdateTransactionState::Staged;
    // The original must still be exactly the reviewed file.
    match observe(&plan.target_path) {
        Ok(Observed::Hash(current)) if current == plan.target_sha256 => {}
        Ok(_) | Err(_) => {
            return Err(fail_before_publication(
                &mut journal,
                &staging,
                UpdateExecutionError::Stale,
            ));
        }
    }
    // Fresh quiescence evidence immediately before the first byte moves.
    if let Err(error) = require_stopped(quiescence()) {
        return Err(fail_before_publication(&mut journal, &staging, error));
    }
    lock.check()?;
    paths.check()?;
    if let Err(error) = safety::require_single_link(&plan.target_path) {
        return Err(fail_before_publication(&mut journal, &staging, error));
    }
    // Durable intent BEFORE replacing anything.
    journal.state = UpdateTransactionState::Applying;
    if let Err(error) = persist_journal(&journal) {
        return Err(fail_before_publication(&mut journal, &staging, error));
    }
    crashed(UpdateTransactionState::Applying)?;
    lock.check()?;
    paths.check()?;
    if let Err(error) = move_noreplace(&plan.target_path, &plan.rollback_path) {
        if matches!(error, UpdateExecutionError::NeedsReconciliation(_)) {
            return Err(error);
        }
        return Err(fail_before_publication(&mut journal, &staging, error));
    }
    crashed_boundary(MutationBoundary::OriginalMoved)?;
    // The preserved copy must be exactly the file that was reviewed.
    match observe(&plan.rollback_path) {
        Ok(Observed::Hash(ref backup))
            if *backup == plan.target_sha256
                && file_identity(&plan.rollback_path).ok() == journal.original_identity => {}
        other => {
            // The target changed between the check and the move: what we
            // moved is someone else's file.  Give it back untouched.
            let cause = UpdateExecutionError::Stale;
            let note = format!(
                "preserved copy did not match the reviewed file ({})",
                other
                    .as_ref()
                    .map(describe)
                    .unwrap_or_else(|e| e.to_string())
            );
            journal.failure = Some(note);
            return match move_noreplace(&plan.rollback_path, &plan.target_path) {
                Ok(()) => Err(fail_before_publication(&mut journal, &staging, cause)),
                Err(error) => {
                    let message = format!(
                        "target changed during update and could not be restored ({error}); it is preserved at {}",
                        plan.rollback_path.display()
                    );
                    mark_failed(
                        &mut journal,
                        UpdateTransactionState::NeedsReconciliation,
                        &message,
                    );
                    Err(UpdateExecutionError::NeedsReconciliation(message))
                }
            };
        }
    }
    journal.state = UpdateTransactionState::BackupMoved;
    if let Err(error) = persist_journal(&journal) {
        return Err(compensate(
            &mut journal,
            &staging,
            error,
            paths,
            lock,
            quiescence,
        ));
    }
    crashed(UpdateTransactionState::BackupMoved)?;
    if let Err(cause) = lock.check().and_then(|_| paths.check()) {
        return Err(partial_failure(&mut journal, "Update", &cause));
    }
    // Download ownership and bytes must still match immediately before publication.
    if !matches_file(&staging, &journal.published_sha256, journal.staged_identity)? {
        return Err(compensate(
            &mut journal,
            &staging,
            UpdateExecutionError::Stale,
            paths,
            lock,
            quiescence,
        ));
    }
    let published = if fault_hit(Fault::FailPublishRename) {
        Err(io_err("injected publish failure"))
    } else {
        move_noreplace(&staging, &plan.target_path)
    };
    if let Err(error) = published {
        if matches!(error, UpdateExecutionError::NeedsReconciliation(_)) {
            return Err(error);
        }
        return Err(compensate(
            &mut journal,
            &staging,
            error,
            paths,
            lock,
            quiescence,
        ));
    }
    crashed_boundary(MutationBoundary::PublishedMoved)?;
    if let Some(parent) = plan.target_path.parent() {
        sync_directory(parent)
            .map_err(|error| UpdateExecutionError::NeedsReconciliation(error.to_string()))?;
    }
    if observe(&plan.target_path)? != Observed::Hash(journal.published_sha256.clone()) {
        let message = "published file does not match the verified artifact".to_string();
        mark_failed(
            &mut journal,
            UpdateTransactionState::NeedsReconciliation,
            &message,
        );
        return Err(UpdateExecutionError::NeedsReconciliation(message));
    }
    journal.state = UpdateTransactionState::Published;
    journal.staged_path = None;
    if !terminal_consistent(&journal).unwrap_or(false) {
        let message = "the published state contradicts the recorded identities (target, backup or leftovers); it was not recorded as complete".to_string();
        journal.state = UpdateTransactionState::BackupMoved;
        mark_failed(
            &mut journal,
            UpdateTransactionState::NeedsReconciliation,
            &message,
        );
        return Err(UpdateExecutionError::NeedsReconciliation(message));
    }
    if let Err(error) = persist_journal(&journal) {
        // Published on disk, but the final record is missing.  The earlier
        // BackupMoved record plus the matching hashes let recovery finish.
        return Err(UpdateExecutionError::NeedsReconciliation(format!(
            "update was published but its final record could not be saved ({error}); run recovery"
        )));
    }
    Ok(journal)
}

/// Undo of a published update.  Requires the durable record, the current
/// output to match the recorded publication, the backup to match the
/// recorded original, and fresh quiescence evidence.  The replaced
/// (published) executable is never deleted: it is kept beside the backup.
pub fn rollback_staged_update<Q>(
    journal: &UpdateJournal,
    mut quiescence: Q,
) -> Result<UpdateJournal, UpdateExecutionError>
where
    Q: FnMut() -> QuiescenceEvidence,
{
    let record = journal.record_path()?;
    let paths = safety::Paths::journal(&record, journal)?;
    if !journal.has_ownership_evidence() {
        return Err(UpdateExecutionError::Record(
            "this record predates executable ownership receipts; Undo is refused because file contents alone cannot prove which executables are EmuWiz's. Nothing was changed"
                .into(),
        ));
    }
    require_stopped(quiescence())?;
    let lock = acquire_lock(&paths.directory, &paths.target, &journal.transaction_id)?;
    let recorded = load_journal(&record)?;
    safety::Paths::journal(&record, &recorded)?;
    if &recorded != journal {
        return Err(UpdateExecutionError::Record(
            "the reviewed journal differs from the durable record; reload it".into(),
        ));
    }
    if recorded.state != UpdateTransactionState::Published
        || latest_record(&discover_update_records(&paths.root), &paths.target).as_ref()
            != Some(&recorded)
    {
        return Err(UpdateExecutionError::Ineligible(
            UpdateExecutionEligibility::ReviewRequired,
        ));
    }
    let directory = paths.directory.clone();
    let result = undo_locked(recorded.clone(), &directory, &mut quiescence, &paths, &lock);
    result
}

fn undo_locked<Q>(
    mut journal: UpdateJournal,
    _directory: &Path,
    quiescence: &mut Q,
    paths: &safety::Paths,
    lock: &safety::TargetLock,
) -> Result<UpdateJournal, UpdateExecutionError>
where
    Q: FnMut() -> QuiescenceEvidence,
{
    let current = observe(&journal.target_path)?;
    if current != Observed::Hash(journal.published_sha256.clone())
        || !matches_file(
            &journal.target_path,
            &journal.published_sha256,
            journal.staged_identity,
        )?
    {
        return Err(UpdateExecutionError::TargetChanged(format!(
            "{} {}; it no longer matches the recorded update, so nothing was changed",
            journal.target_path.display(),
            describe(&current)
        )));
    }
    let backup = observe(&journal.rollback_path)?;
    if backup != Observed::Hash(journal.original_sha256.clone())
        || !matches_file(
            &journal.rollback_path,
            &journal.original_sha256,
            journal.original_identity,
        )?
    {
        return Err(UpdateExecutionError::BackupChanged(format!(
            "{} {}; the preserved copy cannot be trusted, so nothing was changed",
            journal.rollback_path.display(),
            describe(&backup)
        )));
    }
    let displaced = paths.displaced.clone();
    if fs::symlink_metadata(&displaced).is_ok() {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "a displaced-executable path already exists; it is never overwritten".into(),
        ));
    }
    require_stopped(quiescence())?;
    lock.check()?;
    paths.check()?;
    safety::require_single_link(&journal.target_path)?;
    safety::require_single_link(&journal.rollback_path)?;
    // Durable intent before any executable bytes move.
    journal.state = UpdateTransactionState::Undoing;
    journal.displaced_path = Some(displaced.clone());
    if let Err(error) = persist_journal(&journal) {
        return Err(error);
    }
    crashed(UpdateTransactionState::Undoing)?;
    let restore_published = |journal: &mut UpdateJournal| {
        journal.state = UpdateTransactionState::Published;
        journal.displaced_path = None;
        let _ = persist_journal(journal);
    };
    if let Err(error) = move_noreplace(&journal.target_path, &displaced) {
        if matches!(error, UpdateExecutionError::NeedsReconciliation(_)) {
            return Err(error);
        }
        restore_published(&mut journal);
        return Err(error);
    }
    crashed_boundary(MutationBoundary::UndoDisplaced)?;
    match observe(&displaced) {
        Ok(Observed::Hash(ref hash))
            if *hash == journal.published_sha256
                && file_identity(&displaced).ok() == journal.staged_identity => {}
        other => {
            // Replaced between the check and the move: it is not ours.
            return match move_noreplace(&displaced, &journal.target_path) {
                Ok(()) => {
                    restore_published(&mut journal);
                    Err(UpdateExecutionError::TargetChanged(format!(
                        "the executable changed during undo ({}); it was put back",
                        other
                            .as_ref()
                            .map(describe)
                            .unwrap_or_else(|e| e.to_string())
                    )))
                }
                Err(error) => Err(UpdateExecutionError::NeedsReconciliation(format!(
                    "the executable changed during undo and could not be put back ({error}); it is at {}",
                    displaced.display()
                ))),
            };
        }
    }
    // From here the published executable is already parked. Any stop is a
    // partial operation: say so, keep the Undoing record, require recovery.
    if let Err(cause) = lock
        .check()
        .and_then(|_| paths.check())
        .and_then(|_| require_stopped(quiescence()))
    {
        return Err(partial_failure(&mut journal, "Undo", &cause));
    }
    match matches_file(
        &journal.rollback_path,
        &journal.original_sha256,
        journal.original_identity,
    ) {
        Ok(true) => {}
        other => {
            let cause = UpdateExecutionError::NeedsReconciliation(match other {
                Ok(_) => "the backup changed immediately before Undo restoration".into(),
                Err(error) => error.to_string(),
            });
            return Err(partial_failure(&mut journal, "Undo", &cause));
        }
    }
    if let Err(error) = move_noreplace(&journal.rollback_path, &journal.target_path) {
        if matches!(error, UpdateExecutionError::NeedsReconciliation(_)) {
            return Err(error);
        }
        return match move_noreplace(&displaced, &journal.target_path) {
            Ok(()) => {
                restore_published(&mut journal);
                Err(error)
            }
            Err(second) => Err(UpdateExecutionError::NeedsReconciliation(format!(
                "{error}; putting the update back failed ({second}); it is at {}",
                displaced.display()
            ))),
        };
    }
    crashed_boundary(MutationBoundary::UndoRestored)?;
    if let Some(parent) = journal.target_path.parent() {
        sync_directory(parent)
            .map_err(|error| UpdateExecutionError::NeedsReconciliation(error.to_string()))?;
    }
    if !matches_file(
        &journal.target_path,
        &journal.original_sha256,
        journal.original_identity,
    )? {
        let cause = UpdateExecutionError::NeedsReconciliation(
            "restored file does not match the recorded original".into(),
        );
        return Err(partial_failure(&mut journal, "Undo", &cause));
    }
    journal.state = UpdateTransactionState::RolledBack;
    if !terminal_consistent(&journal).unwrap_or(false) {
        journal.state = UpdateTransactionState::Undoing;
        let cause = UpdateExecutionError::NeedsReconciliation(
            "the restored state contradicts the recorded identities".into(),
        );
        return Err(partial_failure(&mut journal, "Undo", &cause));
    }
    if let Err(error) = persist_journal(&journal) {
        return Err(UpdateExecutionError::NeedsReconciliation(format!(
            "the previous executable was restored but the record could not be saved ({error}); run recovery"
        )));
    }
    Ok(journal)
}

/// One durable record associated with its containing installation root.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UpdateRecordEntry {
    pub path: PathBuf,
    pub journal: Result<UpdateJournal, String>,
    pub superseded: bool,
}
impl UpdateRecordEntry {
    pub fn needs_attention(&self) -> bool {
        if self.superseded {
            return false;
        }
        match &self.journal {
            Err(_) => true,
            Ok(j) => {
                !matches!(
                    j.state,
                    UpdateTransactionState::Published
                        | UpdateTransactionState::RolledBack
                        | UpdateTransactionState::Failed
                ) || !terminal_consistent(j).unwrap_or(false)
            }
        }
    }
}
fn terminal_consistent(j: &UpdateJournal) -> Result<bool, UpdateExecutionError> {
    let p = safety::Paths::journal(&j.record_path()?, j)?;
    use UpdateTransactionState as S;
    let original = matches_file(&p.target, &j.original_sha256, j.original_identity)?;
    let backup = matches_file(&p.backup, &j.original_sha256, j.original_identity)?;
    let staged = match &j.staged_path {
        None => observe(&p.staging)? == Observed::Missing,
        Some(path) => {
            observe(path)? == Observed::Missing
                || j.staged_identity
                    .is_some_and(|id| file_identity(path).ok() == Some(id))
        }
    };
    Ok(staged
        && match j.state {
            S::Published => {
                matches_file(&p.target, &j.published_sha256, j.staged_identity)?
                    && backup
                    && j.staged_path.is_none()
                    && j.displaced_path.is_none()
                    && observe(&p.retained)? == Observed::Missing
                    && observe(&p.displaced)? == Observed::Missing
            }
            S::RolledBack => {
                original
                    && observe(&p.backup)? == Observed::Missing
                    && j.displaced_path.as_ref() == Some(&p.displaced)
                    && matches_file(&p.displaced, &j.published_sha256, j.staged_identity)?
            }
            S::Failed => {
                original
                    && observe(&p.backup)? == Observed::Missing
                    && j.displaced_path.is_none()
                    && observe(&p.displaced)? == Observed::Missing
            }
            _ => false,
        })
}
/// Directory filenames are display order only; never Undo authority.
pub fn discover_update_records(root: &Path) -> Vec<UpdateRecordEntry> {
    let directory = root.join(ROLLBACK_DIR);
    let failure = |message: String| UpdateRecordEntry {
        path: directory.clone(),
        journal: Err(message),
        superseded: false,
    };
    let entries = match fs::read_dir(&directory) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            return vec![failure(format!(
                "update record directory cannot be inspected: {e}"
            ))];
        }
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                records.push(failure(format!(
                    "update record entry cannot be inspected: {e}"
                )));
                continue;
            }
        };
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|n| !n.starts_with('.') && n.ends_with(".journal.json"))
        {
            continue;
        }
        if records.len() >= 4096 {
            records.push(failure(
                "too many update records; results were not silently truncated".into(),
            ));
            break;
        }
        let path = entry.path();
        let journal = load_journal(&path)
            .and_then(|j| {
                safety::Paths::journal(&path, &j)?;
                Ok(j)
            })
            .map_err(|e| e.to_string());
        records.push(UpdateRecordEntry {
            path,
            journal,
            superseded: false,
        });
    }
    let orders: std::collections::BTreeMap<PathBuf, u64> = records
        .iter()
        .filter_map(|e| e.journal.as_ref().ok())
        .filter_map(|j| j.sequence.map(|n| (j.target_path.clone(), n)))
        .fold(std::collections::BTreeMap::new(), |mut m, (p, n)| {
            m.entry(p).and_modify(|v| *v = (*v).max(n)).or_insert(n);
            m
        });
    for entry in &mut records {
        if let Ok(j) = &entry.journal {
            entry.superseded = j
                .sequence
                .is_some_and(|n| orders.get(&j.target_path).is_some_and(|new| n < *new));
        }
    }
    records.sort_by(|a, b| {
        a.journal
            .as_ref()
            .ok()
            .and_then(|j| j.sequence)
            .cmp(&b.journal.as_ref().ok().and_then(|j| j.sequence))
            .then(a.path.cmp(&b.path))
    });
    records
}
fn next_sequence(root: &Path, target: &Path) -> Result<u64, UpdateExecutionError> {
    discover_update_records(root)
        .iter()
        .filter_map(|e| e.journal.as_ref().ok())
        .filter(|j| j.target_path == target)
        .filter_map(|j| j.sequence)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| {
            UpdateExecutionError::Record("transaction ordering space is exhausted".into())
        })
}
/// Only an unambiguous latest record for THIS target can be offered as Undo.
/// Multiple old records without ordering evidence require manual review.
fn latest_record(records: &[UpdateRecordEntry], target: &Path) -> Option<UpdateJournal> {
    if records.iter().any(|e| {
        e.journal.is_err()
            && (if e.path.file_name() == Some(std::ffi::OsStr::new(ROLLBACK_DIR)) {
                e.path.parent()
            } else {
                e.path.parent().and_then(Path::parent)
            })
            .is_some_and(|root| target.starts_with(root))
    }) {
        return None;
    }
    let matching: Vec<_> = records
        .iter()
        .filter_map(|e| e.journal.as_ref().ok())
        .filter(|j| j.target_path == target)
        .collect();
    let selected = if matching.len() == 1 {
        matching[0]
    } else {
        if matching.iter().any(|j| j.sequence.is_none()) {
            return None;
        }
        let max = matching.iter().filter_map(|j| j.sequence).max()?;
        let latest: Vec<_> = matching
            .iter()
            .filter(|j| j.sequence == Some(max))
            .collect();
        if latest.len() != 1 {
            return None;
        }
        latest[0]
    };
    Some(selected.clone())
}
pub fn actionable_undo(records: &[UpdateRecordEntry], target: &Path) -> Option<UpdateJournal> {
    // A control is offered only when Undo can actually run: the latest record,
    // Published, consistent with disk, and carrying the ownership receipts Undo
    // requires (legacy records without them are review-only).
    latest_record(records, target).filter(|j| {
        j.state == UpdateTransactionState::Published
            && j.has_ownership_evidence()
            && terminal_consistent(j).ok() == Some(true)
    })
}

/// A discovery pointer is not mutation authority: every record still validates
/// its derived root, paths, content and identities before any operation.
fn remembered_directory() -> Result<PathBuf, UpdateExecutionError> {
    crate::app_dirs::data_dir()
        .map(|p| p.join("emulator-update-roots"))
        .map_err(io_err)
}
fn remember_installation_root(root: &Path) -> Result<(), UpdateExecutionError> {
    let directory = remembered_directory()?;
    fs::create_dir_all(&directory).map_err(io_err)?;
    let name: String = Sha256::digest(root.as_os_str().as_encoded_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = directory.join(format!("{name}.json"));
    match safety::Slot::open(&path)?.create() {
        Ok(mut f) => {
            f.write_all(&serde_json::to_vec(root).map_err(io_err)?)
                .map_err(io_err)?;
            f.sync_all().map_err(io_err)?;
            sync_directory(&directory)
        }
        Err(UpdateExecutionError::Io(_)) if path.is_file() => {
            let mut bytes = Vec::new();
            safety::Slot::open(&path)?
                .read()?
                .take(MAX_METADATA_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(io_err)?;
            if bytes.len() > MAX_METADATA_BYTES {
                return Err(io_err("root pointer too large"));
            }
            let prior: PathBuf = serde_json::from_slice(&bytes).map_err(io_err)?;
            if prior != root {
                return Err(io_err("root pointer identity changed"));
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}
pub fn remembered_installation_roots() -> Result<Vec<PathBuf>, UpdateExecutionError> {
    let directory = remembered_directory()?;
    let entries = match fs::read_dir(directory) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(e)),
    };
    let mut roots = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_err)?;
        if entry.path().extension() != Some(std::ffi::OsStr::new("json")) {
            continue;
        }
        if roots.len() >= 4096 {
            return Err(io_err(
                "too many update roots; discovery was not silently truncated",
            ));
        }
        let slot = safety::Slot::open(&entry.path())?;
        let mut bytes = Vec::new();
        slot.read()?
            .take(MAX_METADATA_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_err)?;
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(io_err("root pointer too large"));
        }
        let root: PathBuf = serde_json::from_slice(&bytes).map_err(io_err)?;
        roots.push(root);
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// Recovery holds exactly the same kernel target lock as Apply and Undo.
/// All executable moves are no-clobber; staging is retained, never deleted.
pub fn recover_update<Q>(
    record: &Path,
    mut quiescence: Q,
) -> Result<UpdateJournal, UpdateExecutionError>
where
    Q: FnMut() -> QuiescenceEvidence,
{
    let reviewed = load_journal(record)?;
    let paths = safety::Paths::journal(record, &reviewed)?;
    let lock = acquire_lock(&paths.directory, &paths.target, &reviewed.transaction_id)?;
    let mut journal = load_journal(record)?;
    if journal != reviewed {
        return Err(UpdateExecutionError::Record(
            "record changed while recovery acquired its lock".into(),
        ));
    }
    safety::Paths::journal(record, &journal)?;
    lock.check()?;
    paths.check()?;
    use UpdateTransactionState as S;
    if matches!(journal.state, S::Published | S::RolledBack | S::Failed) {
        return if terminal_consistent(&journal)? {
            Ok(journal)
        } else {
            Err(UpdateExecutionError::NeedsReconciliation("terminal record conflicts with installation, backup or displaced state; all evidence was preserved".into()))
        };
    }
    if matches!(journal.state, S::NeedsReconciliation | S::Stale) {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "record requires explicit manual review; no evidence was discarded".into(),
        ));
    }
    if !journal.has_ownership_evidence() {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "this record predates executable ownership receipts; recovery will not move files on content equality alone. Review it manually; nothing was changed".into(),
        ));
    }
    require_stopped(quiescence())?;
    let target = observe(&paths.target)?;
    let backup = observe(&paths.backup)?;
    let original = Observed::Hash(journal.original_sha256.clone());
    let published = Observed::Hash(journal.published_sha256.clone());
    let refuse = || {
        UpdateExecutionError::NeedsReconciliation("target, backup or displaced evidence contradicts the recorded transaction; nothing was overwritten or deleted".into())
    };
    match journal.state {
        S::Planned | S::Downloading | S::Verifying | S::Staged | S::Applying | S::BackupMoved => {
            if target == Observed::Missing
                && backup == original
                && matches!(journal.state, S::Applying | S::BackupMoved)
            {
                restore_verified(
                    &journal,
                    &paths,
                    &lock,
                    &paths.backup,
                    &journal.original_sha256,
                    journal.original_identity,
                    &mut quiescence,
                )?;
                crashed_boundary(MutationBoundary::RecoveryMoved)?;
            } else if target == published && backup == original && journal.state == S::BackupMoved {
                if !matches_file(
                    &paths.backup,
                    &journal.original_sha256,
                    journal.original_identity,
                )? || !matches_file(
                    &paths.target,
                    &journal.published_sha256,
                    journal.staged_identity,
                )? {
                    return Err(refuse());
                }
                journal.state = S::Published;
                journal.staged_path = None;
                persist_verified_terminal(&journal)?;
                return Ok(journal);
            } else if !(target == original
                && backup == Observed::Missing
                && matches_file(
                    &paths.target,
                    &journal.original_sha256,
                    journal.original_identity,
                )?)
            {
                return Err(refuse());
            }
            if let Some(stage) = journal.staged_path.clone() {
                retain_staging(&mut journal, &stage)?;
            }
            journal.state = S::Failed;
            journal.failure=Some("interrupted update reconciled; the original is at its install path and temporary output was preserved".into());
            persist_verified_terminal(&journal)?;
            Ok(journal)
        }
        S::Undoing => {
            if journal.displaced_path.as_ref() != Some(&paths.displaced) {
                return Err(refuse());
            }
            let displaced = observe(&paths.displaced)?;
            if target == published && backup == original && displaced == Observed::Missing {
                if !matches_file(
                    &paths.backup,
                    &journal.original_sha256,
                    journal.original_identity,
                )? || !matches_file(
                    &paths.target,
                    &journal.published_sha256,
                    journal.staged_identity,
                )? {
                    return Err(refuse());
                }
                journal.state = S::Published;
                journal.displaced_path = None;
            } else if target == Observed::Missing && backup == original && displaced == published {
                // Content equality is not ownership: the backup must be the
                // exact recorded inode before anything moves.
                if !matches_file(
                    &paths.backup,
                    &journal.original_sha256,
                    journal.original_identity,
                )? {
                    return Err(refuse());
                }
                restore_verified(
                    &journal,
                    &paths,
                    &lock,
                    &paths.displaced,
                    &journal.published_sha256,
                    journal.staged_identity,
                    &mut quiescence,
                )?;
                crashed_boundary(MutationBoundary::RecoveryMoved)?;
                journal.state = S::Published;
                journal.displaced_path = None;
            } else if target == original && backup == Observed::Missing && displaced == published {
                if !matches_file(
                    &paths.target,
                    &journal.original_sha256,
                    journal.original_identity,
                )? || !matches_file(
                    &paths.displaced,
                    &journal.published_sha256,
                    journal.staged_identity,
                )? {
                    return Err(refuse());
                }
                journal.state = S::RolledBack;
            } else {
                return Err(refuse());
            }
            persist_verified_terminal(&journal)?;
            Ok(journal)
        }
        _ => Err(refuse()),
    }
}
fn restore_verified<Q: FnMut() -> QuiescenceEvidence>(
    journal: &UpdateJournal,
    paths: &safety::Paths,
    lock: &safety::TargetLock,
    source: &Path,
    hash: &str,
    identity: Option<FileIdentity>,
    quiescence: &mut Q,
) -> Result<(), UpdateExecutionError> {
    lock.check()?;
    paths.check()?;
    require_stopped(quiescence())?;
    safety::require_single_link(source)?;
    if !matches_file(source, hash, identity)? || observe(&paths.target)? != Observed::Missing {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "recovery source changed or a new executable appeared; both were preserved".into(),
        ));
    }
    if fault_hit(Fault::FailRestoreRename) {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "injected recovery restore failure; evidence preserved".into(),
        ));
    }
    move_noreplace(source, &paths.target)?;
    if !matches_file(&paths.target, hash, identity)? {
        let _ = move_noreplace(&paths.target, source);
        return Err(UpdateExecutionError::NeedsReconciliation(format!(
            "recovery source changed during its move for {}; bytes preserved",
            journal.transaction_id
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetadataError {
    Offline(String),
    Malformed(String),
    Unsupported(String),
}

pub trait UpdateMetadataProvider {
    fn latest(
        &mut self,
        emulator: InventoryEmulator,
        channel: BuildChannel,
    ) -> Result<AvailableVersion, MetadataError>;
}

#[derive(Clone, Debug, Default)]
pub struct LocalMetadataProvider {
    pub versions: std::collections::BTreeMap<InventoryEmulator, AvailableVersion>,
    pub error: Option<MetadataError>,
}

#[derive(Clone, Debug)]
pub struct OfficialMetadataProvider {
    agent: ureq::Agent,
}

impl Default for OfficialMetadataProvider {
    fn default() -> Self {
        let config = crate::http_agent::config(|builder| {
            builder
                .https_only(true)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(METADATA_TIMEOUT))
        });
        Self {
            agent: config.new_agent(),
        }
    }
}

impl UpdateMetadataProvider for OfficialMetadataProvider {
    fn latest(
        &mut self,
        emulator: InventoryEmulator,
        channel: BuildChannel,
    ) -> Result<AvailableVersion, MetadataError> {
        let url = official_metadata_url(emulator, InstallationType::Manual)
            .ok_or_else(|| MetadataError::Unsupported("No official metadata endpoint".into()))?;
        let mut response = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header(
                "User-Agent",
                concat!("archivefs/", env!("CARGO_PKG_VERSION")),
            )
            .call()
            .map_err(|e| MetadataError::Offline(e.to_string()))?;
        if !(200..300).contains(&response.status().as_u16()) {
            return Err(MetadataError::Offline(format!(
                "official metadata returned HTTP {}",
                response.status()
            )));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take((MAX_METADATA_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| MetadataError::Offline(e.to_string()))?;
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(MetadataError::Malformed(
                "metadata response exceeded size limit".into(),
            ));
        }
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| MetadataError::Malformed(e.to_string()))?;
        let tag = json
            .get("tag_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MetadataError::Malformed("release has no tag_name".into()))?;
        let detected = if json
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            BuildChannel::Development
        } else {
            BuildChannel::Stable
        };
        let _requested_channel = channel;
        Ok(AvailableVersion {
            version: tag.trim_start_matches('v').into(),
            channel: detected,
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: url.into(),
            checked_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        })
    }
}

impl UpdateMetadataProvider for LocalMetadataProvider {
    fn latest(
        &mut self,
        emulator: InventoryEmulator,
        _channel: BuildChannel,
    ) -> Result<AvailableVersion, MetadataError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        self.versions
            .get(&emulator)
            .cloned()
            .ok_or_else(|| MetadataError::Offline("No metadata fixture available".into()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ComparableVersion {
    Semver {
        core: Vec<u64>,
        prerelease: Option<(u8, u64)>,
    },
    NumericBuild(Vec<u64>),
    DateBuild(Vec<u64>),
}

fn normalized_version(value: &str) -> Option<&str> {
    let value = value.trim();
    let value = value.strip_prefix(['v', 'V']).unwrap_or(value);
    (!value.is_empty()).then_some(value)
}

fn numeric_parts(value: &str, separator: char) -> Option<Vec<u64>> {
    value
        .split(separator)
        .map(|part| (!part.is_empty()).then(|| part.parse().ok()).flatten())
        .collect()
}

fn comparable_version(value: &str) -> Option<ComparableVersion> {
    let value = normalized_version(value)?;
    let date_parts = value.split_once('-');
    if let Some((date, time)) = date_parts
        && date.len() == 8
        && time.len() == 6
        && date.chars().all(|c| c.is_ascii_digit())
        && time.chars().all(|c| c.is_ascii_digit())
    {
        return Some(ComparableVersion::DateBuild(numeric_parts(value, '-')?));
    }
    if let Some((core, suffix)) = value.split_once('-') {
        if core.chars().all(|c| c.is_ascii_digit() || c == '.')
            && suffix.chars().all(|c| c.is_ascii_digit())
        {
            let mut parts = numeric_parts(core, '.')?;
            parts.push(suffix.parse().ok()?);
            return Some(ComparableVersion::NumericBuild(parts));
        }
        let digit_start = suffix.find(|c: char| c.is_ascii_digit());
        let (label, number) = digit_start
            .map(|index| suffix.split_at(index))
            .unwrap_or((suffix, "0"));
        let rank = match label.to_ascii_lowercase().as_str() {
            "dev" | "nightly" => 0,
            "alpha" | "a" => 1,
            "beta" | "b" => 2,
            "rc" => 3,
            _ => return None,
        };
        let core = numeric_parts(core, '.')?;
        if core.len() < 2 || number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        return Some(ComparableVersion::Semver {
            core,
            prerelease: Some((rank, number.parse().ok()?)),
        });
    }
    if value.chars().all(|c| c.is_ascii_digit() || c == '.') {
        let core = numeric_parts(value, '.')?;
        return (core.len() >= 2).then_some(ComparableVersion::Semver {
            core,
            prerelease: None,
        });
    }
    None
}

fn compare_numeric_parts(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let length = left.len().max(right.len());
    (0..length)
        .map(|index| {
            (
                left.get(index).copied().unwrap_or_default(),
                right.get(index).copied().unwrap_or_default(),
            )
        })
        .find_map(|(left, right)| (left != right).then_some(left.cmp(&right)))
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// Compare only versions whose scheme and ordering are explicit. Leading
/// `v` is insignificant. Opaque labels and revision hashes remain unknown;
/// no lexical ordering is used.
pub fn compare_versions(installed: &str, available: &str) -> Option<std::cmp::Ordering> {
    match (
        comparable_version(installed)?,
        comparable_version(available)?,
    ) {
        (
            ComparableVersion::Semver {
                core: left_core,
                prerelease: left_pre,
            },
            ComparableVersion::Semver {
                core: right_core,
                prerelease: right_pre,
            },
        ) => {
            let core_order = compare_numeric_parts(&left_core, &right_core);
            Some(if core_order == std::cmp::Ordering::Equal {
                match (left_pre, right_pre) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (Some(left), Some(right)) => left.cmp(&right),
                }
            } else {
                core_order
            })
        }
        (ComparableVersion::NumericBuild(left), ComparableVersion::NumericBuild(right))
        | (ComparableVersion::DateBuild(left), ComparableVersion::DateBuild(right)) => {
            Some(compare_numeric_parts(&left, &right))
        }
        _ => None,
    }
}

pub fn compare_installation(
    installation: &EmulatorInstallation,
    latest: Result<AvailableVersion, MetadataError>,
) -> UpdateResult {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (available_version, available_channel, source, provenance, status, warning) = match latest {
        Ok(metadata) => {
            let channel = metadata.channel;
            if installation.channel != BuildChannel::Unknown
                && channel != BuildChannel::Unknown
                && installation.channel != channel
            {
                (
                    Some(metadata.version),
                    channel,
                    metadata.source,
                    metadata.provenance,
                    UpdateStatus::ChannelMismatch,
                    Some("The available metadata describes a different release channel.".into()),
                )
            } else if installation.version.is_none() {
                (
                    Some(metadata.version),
                    channel,
                    metadata.source,
                    metadata.provenance,
                    UpdateStatus::VersionUnknown,
                    Some("Installed version is unknown, so no comparison was made.".into()),
                )
            } else if let Some(ordering) = compare_versions(
                installation.version.as_deref().unwrap_or_default(),
                &metadata.version,
            ) {
                let status = match ordering {
                    std::cmp::Ordering::Less => UpdateStatus::UpdateAvailable,
                    std::cmp::Ordering::Equal => UpdateStatus::UpToDate,
                    std::cmp::Ordering::Greater => UpdateStatus::InstalledNewer,
                };
                (
                    Some(metadata.version),
                    channel,
                    metadata.source,
                    metadata.provenance,
                    status,
                    None,
                )
            } else {
                (
                    Some(metadata.version),
                    channel,
                    metadata.source,
                    metadata.provenance,
                    UpdateStatus::ComparisonUnsupported,
                    Some("These version identifiers do not have a proven ordering.".into()),
                )
            }
        }
        Err(MetadataError::Offline(message)) => (
            None,
            BuildChannel::Unknown,
            UpdateMetadataSource::Unknown,
            "official metadata unavailable".into(),
            UpdateStatus::Offline,
            Some(message),
        ),
        Err(error) => (
            None,
            BuildChannel::Unknown,
            UpdateMetadataSource::Unknown,
            "metadata unavailable".into(),
            UpdateStatus::LatestUnknown,
            Some(format!("{error:?}")),
        ),
    };
    UpdateResult {
        emulator: installation.emulator,
        executable_path: installation.executable_path.clone(),
        installed_version: installation.version.clone(),
        installed_channel: installation.channel,
        available_version,
        available_channel,
        status,
        source,
        provenance,
        checked_unix_seconds: now,
        warning,
        save_state_warning: matches!(
            installation.save_state_risk,
            SaveStateRisk::VersionSensitive
        ) && matches!(status, UpdateStatus::UpdateAvailable),
    }
}

pub fn check_updates<P: UpdateMetadataProvider>(
    installations: &[EmulatorInstallation],
    provider: &mut P,
) -> UpdateReport {
    let mut results: Vec<_> = installations
        .iter()
        .map(|installation| {
            compare_installation(
                installation,
                provider.latest(installation.emulator, installation.channel),
            )
        })
        .collect();
    results.sort_by(|a, b| (a.emulator, &a.executable_path).cmp(&(b.emulator, &b.executable_path)));
    UpdateReport { results }
}

/// Official endpoints are intentionally metadata-only and are not used by
/// tests. Package/Flatpak installations remain on their own provenance lane.
pub fn official_metadata_url(
    emulator: InventoryEmulator,
    kind: InstallationType,
) -> Option<&'static str> {
    if matches!(
        kind,
        InstallationType::SystemPackage | InstallationType::Flatpak
    ) {
        return None;
    }
    match emulator {
        InventoryEmulator::Mame => None,
        InventoryEmulator::Dolphin => {
            Some("https://api.github.com/repos/dolphin-emu/dolphin/releases/latest")
        }
        InventoryEmulator::Rpcs3 => {
            Some("https://api.github.com/repos/RPCS3/rpcs3-binaries-linux/releases/latest")
        }
        InventoryEmulator::Pcsx2 => {
            Some("https://api.github.com/repos/PCSX2/pcsx2/releases/latest")
        }
        InventoryEmulator::Ppsspp => {
            Some("https://api.github.com/repos/hrydgard/ppsspp/releases/latest")
        }
        InventoryEmulator::DuckStation => {
            Some("https://api.github.com/repos/stenzek/duckstation/releases/latest")
        }
        InventoryEmulator::Xemu => {
            Some("https://api.github.com/repos/xemu-project/xemu/releases/latest")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_inventory::{InventoryCandidate, VersionConfidence, VersionSource};
    use std::fs;
    fn stopped() -> QuiescenceEvidence {
        QuiescenceEvidence::Stopped
    }
    fn with_fault<T>(fault: Fault, run: impl FnOnce() -> T) -> T {
        with_faults(&[fault], run)
    }
    fn with_faults<T>(faults: &[Fault], run: impl FnOnce() -> T) -> T {
        FAULT.with(|cell| *cell.borrow_mut() = faults.to_vec());
        let out = run();
        FAULT.with(|cell| cell.borrow_mut().clear());
        out
    }
    fn install(
        version: Option<&str>,
        channel: BuildChannel,
        kind: InstallationType,
    ) -> EmulatorInstallation {
        let candidate = InventoryCandidate {
            emulator: InventoryEmulator::Dolphin,
            executable_path: "/emu/dolphin".into(),
            installation_root: "/emu".into(),
            version_output: version.map(str::to_string),
            installation_type: kind,
            update_capability: crate::emulator_inventory::UpdateCapability::UpstreamRelease,
            preferred: None,
        };
        let mut item = crate::emulator_inventory::inventory_from_candidates(vec![candidate])
            .installations
            .remove(0);
        item.channel = channel;
        item.version_confidence = VersionConfidence::VerifiedCommand;
        item.version_source = VersionSource::VersionCommand;
        item
    }
    fn metadata(version: &str, channel: BuildChannel) -> AvailableVersion {
        AvailableVersion {
            version: version.into(),
            channel,
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: "fixture".into(),
            checked_unix_seconds: 1,
        }
    }
    #[test]
    fn statuses_are_channel_aware() {
        assert_eq!(
            compare_installation(
                &install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual),
                Ok(metadata("1.0", BuildChannel::Stable))
            )
            .status,
            UpdateStatus::UpToDate
        );
        assert_eq!(
            compare_installation(
                &install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual),
                Ok(metadata("2.0", BuildChannel::Stable))
            )
            .status,
            UpdateStatus::UpdateAvailable
        );
        assert_eq!(
            compare_installation(
                &install(Some("9.0"), BuildChannel::Stable, InstallationType::Manual),
                Ok(metadata("2.0", BuildChannel::Stable))
            )
            .status,
            UpdateStatus::InstalledNewer
        );
        assert_eq!(
            compare_installation(
                &install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual),
                Ok(metadata("2.0", BuildChannel::Development))
            )
            .status,
            UpdateStatus::ChannelMismatch
        );
    }
    #[test]
    fn unknown_and_offline_fail_closed() {
        assert_eq!(
            compare_installation(
                &install(None, BuildChannel::Unknown, InstallationType::Manual),
                Err(MetadataError::Offline("x".into()))
            )
            .status,
            UpdateStatus::Offline
        );
        assert_eq!(
            compare_installation(
                &opaque_install(),
                Ok(metadata("nightly", BuildChannel::Stable))
            )
            .status,
            UpdateStatus::ComparisonUnsupported
        );
    }

    fn opaque_install() -> EmulatorInstallation {
        let mut installation = install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual);
        installation.version = Some("abc123def".into());
        installation
    }

    #[test]
    fn version_comparison_is_scheme_aware_and_numeric() {
        use std::cmp::Ordering;

        assert_eq!(compare_versions("1.9", "1.10"), Some(Ordering::Less));
        assert_eq!(compare_versions("v1.20.4", "1.20.4"), Some(Ordering::Equal));
        assert_eq!(compare_versions("1.2", "1.2.0"), Some(Ordering::Equal));
        assert_eq!(
            compare_versions("1.0.0.03", "1.0.0.4"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_versions("0.0.34-17000", "0.0.34-17001"),
            Some(Ordering::Less)
        );
        assert_eq!(compare_versions("2509-1", "2510-1"), Some(Ordering::Less));
        assert_eq!(
            compare_versions("1.2.0-alpha1", "1.2.0-beta1"),
            Some(Ordering::Less)
        );
        assert_eq!(compare_versions("1.2.0-rc1", "1.2.0"), Some(Ordering::Less));
        assert_eq!(compare_versions("abc123", "def456"), None);
        assert_eq!(compare_versions("1.2.3", "0.0.34-17000"), None);
        assert_eq!(compare_versions("nightly", "1.2.3"), None);
        assert_eq!(compare_versions("", "1.2.3"), None);
    }
    #[test]
    fn update_warns_about_save_states() {
        let result = compare_installation(
            &install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual),
            Ok(metadata("2.0", BuildChannel::Stable)),
        );
        assert!(result.save_state_warning);
    }

    fn executable_install(root: &Path) -> EmulatorInstallation {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("dolphin");
        fs::write(&path, b"old emulator").unwrap();
        let mut item = install(
            Some("1.0"),
            BuildChannel::Stable,
            InstallationType::Portable,
        );
        item.executable_path = path;
        item.installation_root = root.to_path_buf();
        item.update_capability = UpdateCapability::PortableManaged;
        item
    }

    fn update_result(install: &EmulatorInstallation) -> UpdateResult {
        UpdateResult {
            emulator: install.emulator,
            executable_path: install.executable_path.clone(),
            installed_version: install.version.clone(),
            installed_channel: install.channel,
            available_version: Some("2.0".into()),
            available_channel: BuildChannel::Stable,
            status: UpdateStatus::UpdateAvailable,
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: "fixture metadata".into(),
            checked_unix_seconds: 1,
            warning: None,
            save_state_warning: true,
        }
    }

    struct FixtureDownloader {
        bytes: Vec<u8>,
    }

    impl UpdateDownloader for FixtureDownloader {
        fn download(
            &mut self,
            _url: &str,
            destination: &mut File,
        ) -> Result<(), UpdateExecutionError> {
            destination
                .write_all(&self.bytes)
                .map_err(|e| UpdateExecutionError::Io(e.to_string()))
        }
    }

    fn artifact(bytes: &[u8]) -> UpdateArtifact {
        UpdateArtifact {
            version: "2.0".into(),
            channel: BuildChannel::Stable,
            url: "https://example.invalid/dolphin".into(),
            sha256: Some(
                Sha256::digest(bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            ),
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: "fixture artifact".into(),
        }
    }

    #[test]
    fn staged_update_publishes_and_rolls_back_without_source_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let mut installation = executable_install(directory.path());
        let update = update_result(&installation);
        let bytes = b"new emulator";
        let plan = plan_staged_update(
            &installation,
            &update,
            artifact(bytes),
            QuiescenceEvidence::Stopped,
        );
        assert_eq!(plan.eligibility, UpdateExecutionEligibility::Ready);
        assert!(plan.save_state_warning);
        let source_hash = hash_file(&installation.executable_path).unwrap();
        let mut downloader = FixtureDownloader {
            bytes: bytes.to_vec(),
        };
        let journal =
            execute_staged_update(&plan, &installation, &update, stopped, &mut downloader).unwrap();
        assert_eq!(journal.state, UpdateTransactionState::Published);
        assert_eq!(fs::read(&installation.executable_path).unwrap(), bytes);
        assert_eq!(fs::read(&plan.rollback_path).unwrap(), b"old emulator");
        let restored = rollback_staged_update(&journal, stopped).unwrap();
        assert_eq!(restored.state, UpdateTransactionState::RolledBack);
        assert_eq!(
            hash_file(&installation.executable_path).unwrap(),
            source_hash
        );
        assert!(!plan.rollback_path.exists());
        installation.version = Some("2.0".into());
    }

    #[test]
    fn plan_and_execution_fail_closed_for_unsafe_or_stale_state() {
        let directory = tempfile::tempdir().unwrap();
        let installation = executable_install(directory.path());
        let update = update_result(&installation);
        let mut unverified_artifact = artifact(b"new emulator");
        unverified_artifact.sha256 = None;
        assert_eq!(
            plan_staged_update(
                &installation,
                &update,
                unverified_artifact,
                QuiescenceEvidence::Stopped
            )
            .eligibility,
            UpdateExecutionEligibility::VerificationUnavailable
        );
        assert_eq!(
            plan_staged_update(
                &installation,
                &update,
                artifact(b"new emulator"),
                QuiescenceEvidence::Running
            )
            .eligibility,
            UpdateExecutionEligibility::RunningBlocked
        );
        let mut changed = installation.clone();
        changed.version = Some("1.1".into());
        let plan = plan_staged_update(
            &installation,
            &update,
            artifact(b"new emulator"),
            QuiescenceEvidence::Stopped,
        );
        let mut downloader = FixtureDownloader {
            bytes: b"new emulator".to_vec(),
        };
        assert_eq!(
            execute_staged_update(&plan, &changed, &update, stopped, &mut downloader),
            Err(UpdateExecutionError::Stale)
        );
        assert_eq!(
            fs::read(&installation.executable_path).unwrap(),
            b"old emulator"
        );
    }

    #[test]
    fn checksum_mismatch_never_publishes() {
        let directory = tempfile::tempdir().unwrap();
        let installation = executable_install(directory.path());
        let update = update_result(&installation);
        let plan = plan_staged_update(
            &installation,
            &update,
            artifact(b"expected"),
            QuiescenceEvidence::Stopped,
        );
        let mut downloader = FixtureDownloader {
            bytes: b"wrong".to_vec(),
        };
        assert!(matches!(
            execute_staged_update(&plan, &installation, &update, stopped, &mut downloader),
            Err(UpdateExecutionError::Verification(_))
        ));
        assert_eq!(
            fs::read(&installation.executable_path).unwrap(),
            b"old emulator"
        );
        assert!(!plan.rollback_path.exists());
    }

    const OLD: &[u8] = b"old emulator";
    const NEW: &[u8] = b"new emulator";

    struct Fixture {
        _dir: tempfile::TempDir,
        installation: EmulatorInstallation,
        update: UpdateResult,
        plan: UpdateExecutionPlan,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let installation = executable_install(dir.path());
        let update = update_result(&installation);
        let plan = plan_staged_update(
            &installation,
            &update,
            artifact(NEW),
            QuiescenceEvidence::Stopped,
        );
        Fixture {
            _dir: dir,
            installation,
            update,
            plan,
        }
    }

    fn run(
        f: &Fixture,
        q: impl FnMut() -> QuiescenceEvidence,
    ) -> Result<UpdateJournal, UpdateExecutionError> {
        let mut downloader = FixtureDownloader {
            bytes: NEW.to_vec(),
        };
        execute_staged_update(&f.plan, &f.installation, &f.update, q, &mut downloader)
    }

    fn target(f: &Fixture) -> Vec<u8> {
        fs::read(&f.installation.executable_path).unwrap()
    }

    fn no_leftovers(f: &Fixture) {
        let root = f.installation.installation_root.clone();
        for entry in fs::read_dir(&root).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                !name.starts_with(".emuwiz-update-staging"),
                "staging left: {name}"
            );
        }
    }

    #[test]
    fn undo_refuses_when_executable_was_replaced_externally() {
        let f = fixture();
        let journal = run(&f, stopped).unwrap();
        fs::write(&f.installation.executable_path, b"user's own build").unwrap();
        let err = rollback_staged_update(&journal, stopped).unwrap_err();
        assert!(
            matches!(err, UpdateExecutionError::TargetChanged(_)),
            "{err:?}"
        );
        assert_eq!(target(&f), b"user's own build");
        assert_eq!(fs::read(&f.plan.rollback_path).unwrap(), OLD);
    }

    #[test]
    fn undo_refuses_symlinked_or_missing_output() {
        let f = fixture();
        let journal = run(&f, stopped).unwrap();
        let path = &f.installation.executable_path;
        fs::remove_file(path).unwrap();
        assert!(matches!(
            rollback_staged_update(&journal, stopped),
            Err(UpdateExecutionError::TargetChanged(_))
        ));
        std::os::unix::fs::symlink("/bin/true", path).unwrap();
        assert!(matches!(
            rollback_staged_update(&journal, stopped),
            Err(UpdateExecutionError::TargetChanged(_))
        ));
        assert_eq!(fs::read(&f.plan.rollback_path).unwrap(), OLD);
    }

    #[test]
    fn undo_refuses_modified_or_missing_backup() {
        let f = fixture();
        let journal = run(&f, stopped).unwrap();
        fs::write(&f.plan.rollback_path, b"tampered").unwrap();
        let err = rollback_staged_update(&journal, stopped).unwrap_err();
        assert!(
            matches!(err, UpdateExecutionError::BackupChanged(_)),
            "{err:?}"
        );
        assert_eq!(target(&f), NEW);
        fs::remove_file(&f.plan.rollback_path).unwrap();
        assert!(matches!(
            rollback_staged_update(&journal, stopped),
            Err(UpdateExecutionError::BackupChanged(_))
        ));
        assert_eq!(target(&f), NEW);
    }

    #[test]
    fn emulator_started_after_preview_blocks_apply_before_any_change() {
        let f = fixture();
        // Stopped at the first check, running by the pre-apply recheck.
        let mut calls = 0;
        let err = run(&f, || {
            calls += 1;
            if calls == 1 {
                QuiescenceEvidence::Stopped
            } else {
                QuiescenceEvidence::Running
            }
        })
        .unwrap_err();
        assert_eq!(
            err,
            UpdateExecutionError::Ineligible(UpdateExecutionEligibility::RunningBlocked)
        );
        assert_eq!(calls, 2);
        assert_eq!(target(&f), OLD);
        assert!(!f.plan.rollback_path.exists());
        no_leftovers(&f);
        // The kernel lock was released; a newly reviewed transaction can retry.
        assert!(fresh_run(&f).is_ok());
    }

    #[test]
    fn unknown_running_state_is_not_stopped() {
        let f = fixture();
        assert_eq!(
            run(&f, || QuiescenceEvidence::Unknown).unwrap_err(),
            UpdateExecutionError::QuiescenceUnknown
        );
        let mut calls = 0;
        assert_eq!(
            run(&f, || {
                calls += 1;
                if calls == 1 {
                    QuiescenceEvidence::Stopped
                } else {
                    QuiescenceEvidence::Unknown
                }
            })
            .unwrap_err(),
            UpdateExecutionError::QuiescenceUnknown
        );
        assert_eq!(target(&f), OLD);
        assert_eq!(
            plan_staged_update(
                &f.installation,
                &f.update,
                artifact(NEW),
                QuiescenceEvidence::Unknown
            )
            .eligibility,
            // Unknown is not stopped, and it is not "running" either.
            UpdateExecutionEligibility::QuiescenceUnknown
        );
        let journal = fresh_run(&f).unwrap();
        assert_eq!(
            rollback_staged_update(&journal, || QuiescenceEvidence::Unknown).unwrap_err(),
            UpdateExecutionError::QuiescenceUnknown
        );
        assert_eq!(
            rollback_staged_update(&journal, || QuiescenceEvidence::Running).unwrap_err(),
            UpdateExecutionError::Ineligible(UpdateExecutionEligibility::RunningBlocked)
        );
        assert_eq!(target(&f), NEW);
    }

    #[test]
    fn failure_between_moving_old_and_publishing_new_restores_the_original() {
        let f = fixture();
        let err = with_fault(Fault::FailPublishRename, || run(&f, stopped).unwrap_err());
        assert!(matches!(err, UpdateExecutionError::Io(_)), "{err:?}");
        assert_eq!(target(&f), OLD);
        assert!(!f.plan.rollback_path.exists());
        no_leftovers(&f);
        let record = discover_update_records(&f.installation.installation_root);
        let journal = record[0].journal.as_ref().unwrap();
        assert_eq!(journal.state, UpdateTransactionState::Failed);
        assert!(!record[0].needs_attention());
    }

    #[test]
    fn failed_restore_marks_reconciliation_and_keeps_the_preserved_original() {
        let f = fixture();
        let err = with_faults(
            &[Fault::FailPublishRename, Fault::FailRestoreRename],
            || run(&f, stopped).unwrap_err(),
        );
        assert!(
            matches!(err, UpdateExecutionError::NeedsReconciliation(_)),
            "{err:?}"
        );
        // The user's original is preserved at the backup path, never lost.
        assert_eq!(fs::read(&f.plan.rollback_path).unwrap(), OLD);
        let records = discover_update_records(&f.installation.installation_root);
        let journal = records[0].journal.as_ref().unwrap();
        assert_eq!(journal.state, UpdateTransactionState::NeedsReconciliation);
        assert!(records[0].needs_attention());
        // No new update can start over an unresolved one (the target is gone
        // and the durable incomplete record still requires review).
        assert!(matches!(
            run(&f, stopped),
            Err(UpdateExecutionError::Stale | UpdateExecutionError::Concurrent(_))
        ));
        assert!(!f.installation.executable_path.exists());
    }

    #[test]
    fn record_persistence_failure_before_replacement_changes_nothing() {
        for state in [
            UpdateTransactionState::Planned,
            UpdateTransactionState::Applying,
        ] {
            let f = fixture();
            let err = with_fault(Fault::FailPersist(state), || run(&f, stopped).unwrap_err());
            assert!(
                matches!(err, UpdateExecutionError::Io(_)),
                "{state:?}: {err:?}"
            );
            assert_eq!(target(&f), OLD, "{state:?}");
            assert!(!f.plan.rollback_path.exists());
            no_leftovers(&f);
            assert!(
                fresh_run(&f).is_ok(),
                "a newly reviewed transaction can acquire the released lock after {state:?}"
            );
        }
    }

    #[test]
    fn record_persistence_failure_after_backup_move_restores_the_original() {
        let f = fixture();
        let err = with_fault(
            Fault::FailPersist(UpdateTransactionState::BackupMoved),
            || run(&f, stopped).unwrap_err(),
        );
        assert!(matches!(err, UpdateExecutionError::Io(_)), "{err:?}");
        assert_eq!(target(&f), OLD);
        assert!(!f.plan.rollback_path.exists());
        no_leftovers(&f);
    }

    #[test]
    fn final_record_failure_reports_partial_state_and_recovery_completes_it() {
        let f = fixture();
        let err = with_fault(
            Fault::FailPersist(UpdateTransactionState::Published),
            || run(&f, stopped).unwrap_err(),
        );
        assert!(
            matches!(err, UpdateExecutionError::NeedsReconciliation(_)),
            "{err:?}"
        );
        assert_eq!(target(&f), NEW);
        // A second attempt is refused (the target is no longer the reviewed file).
        assert!(matches!(
            run(&f, stopped),
            Err(UpdateExecutionError::Stale | UpdateExecutionError::Concurrent(_))
        ));
        let records = discover_update_records(&f.installation.installation_root);
        assert_eq!(records.len(), 1);
        assert!(records[0].needs_attention());
        let recovered = recover_update(&records[0].path, stopped).unwrap();
        assert_eq!(recovered.state, UpdateTransactionState::Published);
        let undone = rollback_staged_update(&recovered, stopped).unwrap();
        assert_eq!(undone.state, UpdateTransactionState::RolledBack);
        assert_eq!(target(&f), OLD);
    }

    #[test]
    fn crash_after_old_moved_is_found_and_recovered_conservatively() {
        let f = fixture();
        let err = with_fault(Fault::Crash(UpdateTransactionState::BackupMoved), || {
            run(&f, stopped).unwrap_err()
        });
        assert_eq!(
            err,
            UpdateExecutionError::Io("simulated process crash".into())
        );
        // Restart: the target is gone, the original is preserved.
        assert!(!f.installation.executable_path.exists());
        assert!(matches!(
            run(&f, stopped),
            Err(UpdateExecutionError::Stale | UpdateExecutionError::Io(_))
        ));
        let records = discover_update_records(&f.installation.installation_root);
        assert!(records[0].needs_attention());
        // Recovery refuses while the emulator state is unknown.
        assert_eq!(
            recover_update(&records[0].path, || QuiescenceEvidence::Unknown).unwrap_err(),
            UpdateExecutionError::QuiescenceUnknown
        );
        let recovered = recover_update(&records[0].path, stopped).unwrap();
        assert_eq!(recovered.state, UpdateTransactionState::Failed);
        assert_eq!(target(&f), OLD);
        assert!(!f.plan.rollback_path.exists());
        no_leftovers(&f);
        assert!(fresh_run(&f).is_ok(), "lock released by recovery");
    }

    #[test]
    fn crash_before_any_move_recovers_without_touching_the_executable() {
        let f = fixture();
        with_fault(Fault::Crash(UpdateTransactionState::Applying), || {
            run(&f, stopped).unwrap_err()
        });
        assert_eq!(target(&f), OLD);
        let records = discover_update_records(&f.installation.installation_root);
        let recovered = recover_update(&records[0].path, stopped).unwrap();
        assert_eq!(recovered.state, UpdateTransactionState::Failed);
        assert_eq!(target(&f), OLD);
        no_leftovers(&f);
    }

    #[test]
    fn recovery_does_not_guess_when_files_match_no_recorded_outcome() {
        let f = fixture();
        with_fault(Fault::Crash(UpdateTransactionState::BackupMoved), || {
            run(&f, stopped).unwrap_err()
        });
        fs::write(&f.installation.executable_path, b"someone else's file").unwrap();
        let records = discover_update_records(&f.installation.installation_root);
        let err = recover_update(&records[0].path, stopped).unwrap_err();
        assert!(
            matches!(err, UpdateExecutionError::NeedsReconciliation(_)),
            "{err:?}"
        );
        assert_eq!(target(&f), b"someone else's file");
        assert_eq!(fs::read(&f.plan.rollback_path).unwrap(), OLD);
    }

    #[test]
    fn crash_during_undo_recovers_back_to_published() {
        let f = fixture();
        let journal = run(&f, stopped).unwrap();
        with_fault(Fault::Crash(UpdateTransactionState::Undoing), || {
            rollback_staged_update(&journal, stopped).unwrap_err()
        });
        assert_eq!(target(&f), NEW);
        let records = discover_update_records(&f.installation.installation_root);
        assert!(records[0].needs_attention());
        let recovered = recover_update(&records[0].path, stopped).unwrap();
        assert_eq!(recovered.state, UpdateTransactionState::Published);
        assert_eq!(
            rollback_staged_update(&recovered, stopped).unwrap().state,
            UpdateTransactionState::RolledBack
        );
        assert_eq!(target(&f), OLD);
    }

    #[test]
    fn successful_update_and_undo_preserve_everything_and_delete_nothing() {
        let f = fixture();
        let journal = run(&f, stopped).unwrap();
        assert_eq!(journal.state, UpdateTransactionState::Published);
        assert_eq!(
            journal.original_sha256,
            hash_file(&f.plan.rollback_path).unwrap()
        );
        assert_eq!(
            journal.published_sha256,
            hash_file(&f.installation.executable_path).unwrap()
        );
        assert_eq!(target(&f), NEW);
        // The durable record matches what was returned.
        let records = discover_update_records(&f.installation.installation_root);
        assert_eq!(records[0].journal.as_ref().unwrap(), &journal);
        assert!(!records[0].needs_attention());
        let undone = rollback_staged_update(&journal, stopped).unwrap();
        assert_eq!(undone.state, UpdateTransactionState::RolledBack);
        assert_eq!(target(&f), OLD);
        // The published executable is parked, not deleted.
        let displaced = undone.displaced_path.clone().unwrap();
        assert_eq!(fs::read(&displaced).unwrap(), NEW);
        // A second undo is refused: the record is no longer Published.
        let again = discover_update_records(&f.installation.installation_root)[0]
            .journal
            .clone()
            .unwrap();
        assert_eq!(again.state, UpdateTransactionState::RolledBack);
        assert!(rollback_staged_update(&again, stopped).is_err());
    }

    #[test]
    fn concurrent_update_attempts_are_serialised() {
        let f = fixture();
        let mut nested: Option<Result<UpdateJournal, UpdateExecutionError>> = None;
        struct Reentrant<'a> {
            fixture: &'a Fixture,
            result: &'a mut Option<Result<UpdateJournal, UpdateExecutionError>>,
        }
        impl UpdateDownloader for Reentrant<'_> {
            fn download(
                &mut self,
                _url: &str,
                destination: &mut File,
            ) -> Result<(), UpdateExecutionError> {
                // A second attempt while the first holds the lock.
                let mut other = FixtureDownloader {
                    bytes: NEW.to_vec(),
                };
                *self.result = Some(execute_staged_update(
                    &self.fixture.plan,
                    &self.fixture.installation,
                    &self.fixture.update,
                    stopped,
                    &mut other,
                ));
                destination.write_all(NEW).map_err(io_err)
            }
        }
        let mut downloader = Reentrant {
            fixture: &f,
            result: &mut nested,
        };
        let journal = execute_staged_update(
            &f.plan,
            &f.installation,
            &f.update,
            stopped,
            &mut downloader,
        )
        .unwrap();
        assert!(matches!(
            nested.unwrap(),
            Err(UpdateExecutionError::Concurrent(_))
        ));
        assert_eq!(journal.state, UpdateTransactionState::Published);
        assert_eq!(target(&f), NEW);
    }

    #[test]
    fn missing_or_corrupt_recovery_record_refuses_undo_and_recovery() {
        let f = fixture();
        let journal = run(&f, stopped).unwrap();
        let record = journal.record_path().unwrap();
        fs::write(&record, b"{ not json").unwrap();
        let entries = discover_update_records(&f.installation.installation_root);
        assert!(entries[0].journal.is_err() && entries[0].needs_attention());
        assert!(matches!(
            rollback_staged_update(&journal, stopped),
            Err(UpdateExecutionError::Record(_))
        ));
        assert!(matches!(
            recover_update(&record, stopped),
            Err(UpdateExecutionError::Record(_))
        ));
        fs::remove_file(&record).unwrap();
        assert!(matches!(
            rollback_staged_update(&journal, stopped),
            Err(UpdateExecutionError::Record(_))
        ));
        assert_eq!(target(&f), NEW);
        assert_eq!(fs::read(&f.plan.rollback_path).unwrap(), OLD);
    }

    #[test]
    fn executable_changed_between_review_and_apply_is_not_replaced() {
        let f = fixture();
        fs::write(&f.installation.executable_path, b"edited after review").unwrap();
        assert_eq!(run(&f, stopped).unwrap_err(), UpdateExecutionError::Stale);
        assert_eq!(target(&f), b"edited after review");
        assert!(!f.plan.rollback_path.exists());
    }

    #[test]
    fn existing_backup_path_is_never_overwritten() {
        let f = fixture();
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(f.plan.rollback_path.parent().unwrap())
                .unwrap();
        }
        fs::write(&f.plan.rollback_path, b"earlier backup").unwrap();
        assert!(matches!(
            run(&f, stopped),
            Err(UpdateExecutionError::NeedsReconciliation(_))
        ));
        assert_eq!(fs::read(&f.plan.rollback_path).unwrap(), b"earlier backup");
        assert_eq!(target(&f), OLD);
    }

    // ---- Independent review reproducers (expected to FAIL on the candidate) ----

    /// A process that died after taking the lock but before the first record was
    /// written leaves a lock and no journal: nothing can ever clear it.
    #[test]
    fn review_lock_left_by_a_crash_before_the_first_record_is_recoverable() {
        let f = fixture();
        let directory = f.plan.rollback_path.parent().unwrap().to_path_buf();
        // A kernel lock does not need a rollback directory or a record.
        // The state a killed process leaves: lock present, no journal.
        acquire_lock(&directory, &f.plan.target_path, "deadbeefdeadbeefdeadbeef").unwrap();
        assert!(discover_update_records(&f.installation.installation_root).is_empty());
        // Required: some supported path must let an update proceed (or at least
        // offer recovery) once the owner is provably gone. Today it is permanent.
        let outcome = run(&f, stopped);
        assert!(
            outcome.is_ok(),
            "an orphaned lock from a dead process blocks every later update: {outcome:?}"
        );
    }

    struct BlockingDownloader {
        started: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl UpdateDownloader for BlockingDownloader {
        fn download(
            &mut self,
            _url: &str,
            destination: &mut File,
        ) -> Result<(), UpdateExecutionError> {
            destination.write_all(NEW).unwrap();
            self.started.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
    }

    /// Recovery must not act on a transaction whose owner is still alive.
    #[test]
    fn review_recovery_refuses_a_transaction_that_is_still_running() {
        let f = std::sync::Arc::new(fixture());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = {
            let f = f.clone();
            std::thread::spawn(move || {
                let mut downloader = BlockingDownloader {
                    started: started_tx,
                    release: release_rx,
                };
                execute_staged_update(
                    &f.plan,
                    &f.installation,
                    &f.update,
                    stopped,
                    &mut downloader,
                )
            })
        };
        started_rx.recv().unwrap();
        let records = discover_update_records(&f.installation.installation_root);
        let recovered = recover_update(&records[0].path, stopped);
        release_tx.send(()).unwrap();
        let finished = worker.join().unwrap();
        assert!(
            recovered.is_err(),
            "recovery 'finished' ({recovered:?}) a live update; the live update then ended as {finished:?}"
        );
    }

    /// Recovery must delete only files it can prove it created.
    #[test]
    fn review_recovery_never_deletes_a_file_named_only_by_the_record() {
        let f = fixture();
        with_fault(Fault::Crash(UpdateTransactionState::Applying), || {
            run(&f, stopped).unwrap_err()
        });
        let records = discover_update_records(&f.installation.installation_root);
        let innocent = f.installation.installation_root.join("user-save-data.bin");
        fs::write(&innocent, b"irreplaceable").unwrap();
        // A damaged or hand-edited record pointing at an unrelated file.
        let mut journal = records[0].journal.clone().unwrap();
        journal.staged_path = Some(innocent.clone());
        fs::write(&records[0].path, serde_json::to_vec(&journal).unwrap()).unwrap();
        let _ = recover_update(&records[0].path, stopped);
        assert!(
            innocent.exists(),
            "recovery deleted a file it did not create"
        );
    }

    /// Crash right after the old executable moved but before BackupMoved was
    /// written: the user is left with no executable at the install path.
    #[test]
    fn review_crash_between_the_backup_rename_and_its_record_is_restored() {
        let f = fixture();
        with_fault(Fault::Crash(UpdateTransactionState::Applying), || {
            run(&f, stopped).unwrap_err()
        });
        fs::rename(&f.plan.target_path, &f.plan.rollback_path).unwrap();
        let records = discover_update_records(&f.installation.installation_root);
        let recovered = recover_update(&records[0].path, stopped);
        assert!(
            recovered.is_ok() && target(&f) == OLD,
            "recovery left the emulator missing: {recovered:?}"
        );
    }
    include!("emulator_update/safety_tests.rs");
    include!("emulator_update/closure_tests.rs");
}
