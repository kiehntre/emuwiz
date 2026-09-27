//! Local immutable save snapshots and conservative restore previews.
//!
//! Snapshot creation copies and hashes local save data into an EmuWiz data
//! directory. Generic restore apply is intentionally limited to explicitly
//! bound, single-file native saves.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_SNAPSHOT_FILES: usize = 4_096;
pub const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_SNAPSHOT_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveArtifactType {
    MemoryCard,
    Sram,
    Eeprom,
    FlashSave,
    SaveDirectory,
    SaveState,
    Nvram,
    Vmu,
    PsMemoryCard,
    PspSavedata,
    DolphinMemoryCard,
    Other,
    Unknown,
}

impl SaveArtifactType {
    pub fn is_save_state(self) -> bool {
        self == Self::SaveState
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveProvenance {
    EmulatorProfile,
    ConfiguredPath,
    UserSpecified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorUseStatus {
    NotDetected,
    Running,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveLocation {
    pub path: PathBuf,
    pub emulator: Option<String>,
    pub profile: Option<String>,
    pub artifact_type: SaveArtifactType,
    pub provenance: SaveProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveArtifact {
    pub relative_path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub modified_unix_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveSnapshotCompleteness {
    Complete,
    Partial,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveSnapshotManifest {
    pub format_version: u32,
    pub snapshot_id: String,
    pub game_identity: Option<String>,
    pub platform: Option<String>,
    pub emulator: Option<String>,
    pub emulator_profile: Option<String>,
    pub original_save_path: PathBuf,
    pub artifact_type: SaveArtifactType,
    pub provenance: SaveProvenance,
    pub snapshot_unix_seconds: u64,
    pub source_size_bytes: u64,
    pub artifacts: Vec<SaveArtifact>,
    pub completeness: SaveSnapshotCompleteness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveSnapshot {
    pub manifest: SaveSnapshotManifest,
    pub storage_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveSnapshotRequest {
    pub location: SaveLocation,
    pub game_identity: Option<String>,
    pub platform: Option<String>,
    pub storage_root: PathBuf,
    pub available_space_bytes: Option<u64>,
    pub snapshot_id: Option<String>,
    pub now_unix_seconds: Option<u64>,
    pub emulator_use: EmulatorUseStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveSnapshotReadiness {
    Ready,
    EmulatorRunning,
    EmulatorUseUnknown,
    SourceUnavailable,
    InsufficientSpace { available: u64, required: u64 },
    PathUnsafe(String),
    SourceChanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveRestoreConflict {
    CurrentSaveChanged,
    CurrentSaveNewer,
    SourcePathMismatch,
    EmulatorMismatch,
    ProfileMismatch,
    SnapshotIncomplete,
    SnapshotUnreadable,
    EmulatorUseUnknown,
    EmulatorRunning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRestorePlan {
    pub readiness: SaveSnapshotReadiness,
    pub conflicts: Vec<SaveRestoreConflict>,
    pub files_to_add: Vec<PathBuf>,
    pub files_to_replace: Vec<PathBuf>,
    pub unchanged_files: Vec<PathBuf>,
    pub current_save_newer: bool,
    pub automatic_pre_restore_snapshot_required: bool,
    pub apply_supported: bool,
    pub required_space_bytes: u64,
}

/// The deliberately small set of native save artifacts that the generic
/// executor is allowed to replace.  Memory cards, save states, directories,
/// and opaque containers remain preview-only even when represented by one
/// filesystem path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenericSingleFileSaveFamily {
    Sram,
    Eeprom,
    FlashSave,
    Nvram,
}

impl GenericSingleFileSaveFamily {
    fn from_artifact_type(value: SaveArtifactType) -> Option<Self> {
        match value {
            SaveArtifactType::Sram => Some(Self::Sram),
            SaveArtifactType::Eeprom => Some(Self::Eeprom),
            SaveArtifactType::FlashSave => Some(Self::FlashSave),
            SaveArtifactType::Nvram => Some(Self::Nvram),
            _ => None,
        }
    }
}

/// Strong target binding captured by the restore preview.  The path and
/// snapshot association are mandatory; emulator/profile and game identity
/// are checked whenever either side supplies them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTargetBinding {
    pub target_path: PathBuf,
    pub artifact_type: SaveArtifactType,
    pub emulator: Option<String>,
    pub profile: Option<String>,
    pub game_identity: Option<String>,
    pub snapshot_id: String,
}

impl SaveTargetBinding {
    pub fn from_snapshot(snapshot: &SaveSnapshot) -> Self {
        Self {
            target_path: snapshot.manifest.original_save_path.clone(),
            artifact_type: snapshot.manifest.artifact_type,
            emulator: snapshot.manifest.emulator.clone(),
            profile: snapshot.manifest.emulator_profile.clone(),
            game_identity: snapshot.manifest.game_identity.clone(),
            snapshot_id: snapshot.manifest.snapshot_id.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveQuiescenceRequirement {
    ConfirmedClosed,
    Running,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveRestoreRefusal {
    SnapshotIncomplete,
    SnapshotUnreadable,
    SnapshotHashChanged,
    SnapshotAssociationMismatch,
    TargetPathUnsafe,
    TargetSymlinkOrSpecial,
    TargetBindingMismatch,
    UnsupportedArtifactType(SaveArtifactType),
    MultiFileRestoreNotSupported,
    SharedContainerRestoreNotSupported,
    SaveStateRestoreNotSupported,
    EmulatorRunning,
    EmulatorStateUnknown,
    DestinationChanged,
    BackupFailed,
    StagingFailed,
    StagedHashMismatch,
    PublicationFailed,
    PostWriteVerificationFailed,
    RollbackFailed,
    ReceiptWriteFailed,
    CrossFilesystemStaging,
}

impl std::fmt::Display for SaveRestoreRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::SnapshotIncomplete => "the snapshot is incomplete",
            Self::SnapshotUnreadable => "the snapshot could not be verified",
            Self::SnapshotHashChanged => "the snapshot changed since preview",
            Self::SnapshotAssociationMismatch => {
                "the snapshot is not associated with the selected target"
            }
            Self::TargetPathUnsafe => "the save target path is unsafe",
            Self::TargetSymlinkOrSpecial => "the save target is a symlink or special file",
            Self::TargetBindingMismatch => "the selected game or emulator binding does not match",
            Self::UnsupportedArtifactType(_) => "this save type is not allowed for generic restore",
            Self::MultiFileRestoreNotSupported => {
                "multi-file restore requires atomic set publication and is not yet supported"
            }
            Self::SharedContainerRestoreNotSupported => {
                "this save uses a shared memory card or container and cannot be restored generically"
            }
            Self::SaveStateRestoreNotSupported => {
                "save states require emulator-specific compatibility checks"
            }
            Self::EmulatorRunning => "close the emulator before restoring this save",
            Self::EmulatorStateUnknown => {
                "EmuWiz cannot prove that the emulator is closed; close it before restoring this save"
            }
            Self::DestinationChanged => {
                "the destination changed since preview; review the restore again"
            }
            Self::BackupFailed => "a verified backup could not be created",
            Self::StagingFailed => "the restore staging file could not be prepared",
            Self::StagedHashMismatch => "the staged restore bytes did not match the snapshot",
            Self::PublicationFailed => "the restore could not be published atomically",
            Self::PostWriteVerificationFailed => "the restored save failed post-write verification",
            Self::RollbackFailed => "restore failed and rollback could not be completed safely",
            Self::ReceiptWriteFailed => "the restore receipt could not be recorded",
            Self::CrossFilesystemStaging => {
                "safe atomic staging requires the same filesystem as the target"
            }
        };
        f.write_str(message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveRestorePreflight {
    pub snapshot_id: String,
    pub snapshot_sha256: String,
    pub target_binding: SaveTargetBinding,
    pub target_fingerprint: Option<String>,
    pub target_exists: bool,
    pub quiescence: SaveQuiescenceRequirement,
    pub refusals: Vec<SaveRestoreRefusal>,
}

impl SaveRestorePreflight {
    pub fn ready(&self) -> bool {
        self.refusals.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveRestoreReceipt {
    pub transaction_id: String,
    pub snapshot_id: String,
    pub snapshot_sha256: String,
    pub target_binding: SaveTargetBinding,
    pub pre_restore_target_sha256: Option<String>,
    pub backup_snapshot_id: Option<String>,
    pub backup_snapshot_path: Option<PathBuf>,
    pub post_restore_expected_sha256: String,
    pub post_restore_actual_sha256: String,
    pub quiescence: SaveQuiescenceRequirement,
    pub applied_unix_seconds: u64,
    pub target_was_missing: bool,
    pub rollback_eligible: bool,
    pub receipt_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRestoreApplyOptions {
    pub backup_root: PathBuf,
    pub receipt_root: PathBuf,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRestoreResult {
    pub receipt: SaveRestoreReceipt,
    pub backup_snapshot: Option<SaveSnapshot>,
}

#[derive(Debug)]
pub enum SaveRestoreError {
    Refused(SaveRestoreRefusal),
    Snapshot(SaveSnapshotError),
    Io { path: PathBuf, detail: String },
}

impl std::fmt::Display for SaveRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) => write!(f, "save restore refused: {reason}"),
            Self::Snapshot(error) => write!(f, "save restore snapshot failed: {error}"),
            Self::Io { path, detail } => {
                write!(f, "save restore I/O at {}: {detail}", path.display())
            }
        }
    }
}
impl std::error::Error for SaveRestoreError {}

impl From<SaveSnapshotError> for SaveRestoreError {
    fn from(value: SaveSnapshotError) -> Self {
        Self::Snapshot(value)
    }
}

#[derive(Debug)]
pub enum SaveSnapshotError {
    Io { path: PathBuf, detail: String },
    InvalidPath { path: PathBuf, detail: String },
    SourceChanged(PathBuf),
    InsufficientSpace { available: u64, required: u64 },
    EmulatorRunning,
    EmulatorUseUnknown,
    TooManyFiles,
    TooLarge,
    Serialization(String),
}

impl std::fmt::Display for SaveSnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, detail } => {
                write!(f, "save snapshot I/O at {}: {detail}", path.display())
            }
            Self::InvalidPath { path, detail } => {
                write!(f, "unsafe save path {}: {detail}", path.display())
            }
            Self::SourceChanged(path) => write!(
                f,
                "save changed while being snapshotted: {}",
                path.display()
            ),
            Self::InsufficientSpace {
                available,
                required,
            } => write!(
                f,
                "not enough snapshot space: {available} available, {required} required"
            ),
            Self::EmulatorRunning => write!(
                f,
                "close the emulator before snapshotting or restoring this save"
            ),
            Self::EmulatorUseUnknown => write!(
                f,
                "the emulator may still be using this save; close it before continuing"
            ),
            Self::TooManyFiles => write!(f, "save snapshot contains too many files"),
            Self::TooLarge => write!(f, "save snapshot is larger than the safety limit"),
            Self::Serialization(detail) => write!(f, "save snapshot manifest error: {detail}"),
        }
    }
}
impl std::error::Error for SaveSnapshotError {}

pub fn default_snapshot_root() -> crate::Result<PathBuf> {
    Ok(crate::app_dirs::data_dir()?.join("snapshots"))
}

pub fn assess_snapshot_readiness(
    location: &SaveLocation,
    available_space_bytes: Option<u64>,
    required_space_bytes: u64,
    emulator_use: EmulatorUseStatus,
) -> SaveSnapshotReadiness {
    if !location.path.is_absolute() {
        return SaveSnapshotReadiness::PathUnsafe("path must be absolute".into());
    }
    match emulator_use {
        EmulatorUseStatus::Running => SaveSnapshotReadiness::EmulatorRunning,
        EmulatorUseStatus::Unknown => SaveSnapshotReadiness::EmulatorUseUnknown,
        EmulatorUseStatus::NotDetected => {
            available_space_bytes.map_or(SaveSnapshotReadiness::Ready, |available| {
                if available < required_space_bytes {
                    SaveSnapshotReadiness::InsufficientSpace {
                        available,
                        required: required_space_bytes,
                    }
                } else {
                    SaveSnapshotReadiness::Ready
                }
            })
        }
    }
}

pub fn create_snapshot(request: &SaveSnapshotRequest) -> Result<SaveSnapshot, SaveSnapshotError> {
    let source = validate_source(&request.location.path)?;
    match request.emulator_use {
        EmulatorUseStatus::Running => return Err(SaveSnapshotError::EmulatorRunning),
        EmulatorUseStatus::Unknown => return Err(SaveSnapshotError::EmulatorUseUnknown),
        EmulatorUseStatus::NotDetected => {}
    }
    let entries = collect_files(&source)?;
    let source_size = entries
        .iter()
        .map(|(_, metadata)| metadata.len())
        .sum::<u64>();
    if source_size > MAX_SNAPSHOT_BYTES {
        return Err(SaveSnapshotError::TooLarge);
    }
    if let Some(available) = request.available_space_bytes
        && available < source_size
    {
        return Err(SaveSnapshotError::InsufficientSpace {
            available,
            required: source_size,
        });
    }
    let snapshot_id = request.snapshot_id.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            request.now_unix_seconds.unwrap_or_else(now_unix_seconds),
            short_path_token(&source)
        )
    });
    let game_dir = request.storage_root.join(
        request
            .game_identity
            .as_deref()
            .map(safe_component)
            .unwrap_or_else(|| "unknown-game".into()),
    );
    let final_path = game_dir.join(&snapshot_id);
    if final_path.exists() {
        return Err(SaveSnapshotError::InvalidPath {
            path: final_path,
            detail: "snapshot id already exists".into(),
        });
    }
    fs::create_dir_all(&game_dir).map_err(|error| io_error(&game_dir, error))?;
    let staging = game_dir.join(format!(".{snapshot_id}.partial-{}", std::process::id()));
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    fs::create_dir_all(staging.join("files")).map_err(|error| io_error(&staging, error))?;
    let result = build_snapshot_files(&source, &staging.join("files"), &entries);
    let result = match result {
        Ok(artifacts) => {
            let manifest = SaveSnapshotManifest {
                format_version: 1,
                snapshot_id: snapshot_id.clone(),
                game_identity: request.game_identity.clone(),
                platform: request.platform.clone(),
                emulator: request.location.emulator.clone(),
                emulator_profile: request.location.profile.clone(),
                original_save_path: request.location.path.clone(),
                artifact_type: request.location.artifact_type,
                provenance: request.location.provenance,
                snapshot_unix_seconds: request.now_unix_seconds.unwrap_or_else(now_unix_seconds),
                source_size_bytes: source_size,
                artifacts,
                completeness: SaveSnapshotCompleteness::Complete,
            };
            let bytes = serde_json::to_vec_pretty(&manifest)
                .map_err(|error| SaveSnapshotError::Serialization(error.to_string()))?;
            write_file(&staging.join("manifest.json"), &bytes)?;
            fs::rename(&staging, &final_path).map_err(|error| io_error(&staging, error))?;
            Ok(SaveSnapshot {
                manifest,
                storage_path: final_path,
            })
        }
        Err(error) => Err(error),
    };
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

pub fn verify_snapshot(snapshot: &SaveSnapshot) -> Result<(), SaveSnapshotError> {
    if snapshot.manifest.completeness != SaveSnapshotCompleteness::Complete {
        return Err(SaveSnapshotError::InvalidPath {
            path: snapshot.storage_path.clone(),
            detail: "snapshot is not complete".into(),
        });
    }
    for artifact in &snapshot.manifest.artifacts {
        let path = safe_join(
            &snapshot.storage_path.join("files"),
            &artifact.relative_path,
        )?;
        let (size, hash, _) = hash_and_metadata(&path)?;
        if size != artifact.size_bytes || hash != artifact.sha256 {
            return Err(SaveSnapshotError::SourceChanged(path));
        }
    }
    Ok(())
}

pub fn build_restore_plan(
    snapshot: &SaveSnapshot,
    destination: &SaveLocation,
    emulator_use: EmulatorUseStatus,
) -> SaveRestorePlan {
    let mut plan = SaveRestorePlan {
        readiness: SaveSnapshotReadiness::Ready,
        conflicts: Vec::new(),
        files_to_add: Vec::new(),
        files_to_replace: Vec::new(),
        unchanged_files: Vec::new(),
        current_save_newer: false,
        automatic_pre_restore_snapshot_required: true,
        apply_supported: false,
        required_space_bytes: snapshot.manifest.source_size_bytes,
    };
    if snapshot.manifest.completeness != SaveSnapshotCompleteness::Complete {
        plan.conflicts.push(SaveRestoreConflict::SnapshotIncomplete);
    }
    if snapshot.manifest.original_save_path != destination.path {
        plan.conflicts.push(SaveRestoreConflict::SourcePathMismatch);
    }
    if snapshot.manifest.emulator != destination.emulator {
        plan.conflicts.push(SaveRestoreConflict::EmulatorMismatch);
    }
    if snapshot.manifest.emulator_profile != destination.profile {
        plan.conflicts.push(SaveRestoreConflict::ProfileMismatch);
    }
    match emulator_use {
        EmulatorUseStatus::Running => plan.conflicts.push(SaveRestoreConflict::EmulatorRunning),
        EmulatorUseStatus::Unknown => plan.conflicts.push(SaveRestoreConflict::EmulatorUseUnknown),
        EmulatorUseStatus::NotDetected => {}
    }
    let current = collect_files(&destination.path)
        .unwrap_or_default()
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    for artifact in &snapshot.manifest.artifacts {
        let path = destination_artifact_path(&destination.path, &artifact.relative_path);
        match current.get(&artifact.relative_path) {
            None => plan.files_to_add.push(artifact.relative_path.clone()),
            Some(_) => match hash_and_metadata(&path) {
                Ok((size, hash, _modified))
                    if size == artifact.size_bytes && hash == artifact.sha256 =>
                {
                    plan.unchanged_files.push(artifact.relative_path.clone())
                }
                Ok((_, _, modified)) => {
                    plan.files_to_replace.push(artifact.relative_path.clone());
                    if modified
                        .is_some_and(|value| value > artifact.modified_unix_seconds.unwrap_or(0))
                    {
                        plan.current_save_newer = true;
                        plan.conflicts.push(SaveRestoreConflict::CurrentSaveNewer);
                    } else {
                        plan.conflicts.push(SaveRestoreConflict::CurrentSaveChanged);
                    }
                }
                Err(_) => plan.conflicts.push(SaveRestoreConflict::SnapshotUnreadable),
            },
        }
    }
    if !plan.conflicts.is_empty() {
        plan.readiness = SaveSnapshotReadiness::SourceChanged;
    }
    plan
}

/// Build a strict, single-file restore preflight.  This is intentionally
/// separate from the broad preview planner: callers must opt into the typed
/// target binding and explicit quiescence state before Apply is possible.
pub fn build_single_file_restore_preflight(
    snapshot: &SaveSnapshot,
    binding: &SaveTargetBinding,
    quiescence: SaveQuiescenceRequirement,
) -> SaveRestorePreflight {
    let mut refusals = Vec::new();
    let snapshot_sha256 = snapshot
        .manifest
        .artifacts
        .first()
        .map(|artifact| artifact.sha256.clone())
        .unwrap_or_default();

    if snapshot.manifest.completeness != SaveSnapshotCompleteness::Complete {
        refusals.push(SaveRestoreRefusal::SnapshotIncomplete);
    }
    if snapshot.manifest.artifacts.len() != 1 {
        refusals.push(SaveRestoreRefusal::MultiFileRestoreNotSupported);
    }
    if GenericSingleFileSaveFamily::from_artifact_type(snapshot.manifest.artifact_type).is_none() {
        let refusal = match snapshot.manifest.artifact_type {
            SaveArtifactType::MemoryCard
            | SaveArtifactType::PsMemoryCard
            | SaveArtifactType::DolphinMemoryCard
            | SaveArtifactType::Vmu => SaveRestoreRefusal::SharedContainerRestoreNotSupported,
            SaveArtifactType::SaveState => SaveRestoreRefusal::SaveStateRestoreNotSupported,
            SaveArtifactType::SaveDirectory => SaveRestoreRefusal::MultiFileRestoreNotSupported,
            artifact_type => SaveRestoreRefusal::UnsupportedArtifactType(artifact_type),
        };
        refusals.push(refusal);
    }
    if binding.snapshot_id != snapshot.manifest.snapshot_id
        || binding.target_path != snapshot.manifest.original_save_path
        || binding.artifact_type != snapshot.manifest.artifact_type
        || binding.emulator != snapshot.manifest.emulator
        || binding.profile != snapshot.manifest.emulator_profile
        || (snapshot.manifest.game_identity.is_some()
            && binding.game_identity != snapshot.manifest.game_identity)
    {
        refusals.push(SaveRestoreRefusal::SnapshotAssociationMismatch);
    }
    if !safe_target_path(&binding.target_path) {
        refusals.push(SaveRestoreRefusal::TargetPathUnsafe);
    }
    let target_exists = match fs::symlink_metadata(&binding.target_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            refusals.push(SaveRestoreRefusal::TargetSymlinkOrSpecial);
            true
        }
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(_) => {
            refusals.push(SaveRestoreRefusal::TargetPathUnsafe);
            false
        }
    };
    if let Some(parent) = binding.target_path.parent()
        && !safe_existing_parent(parent)
    {
        refusals.push(SaveRestoreRefusal::TargetPathUnsafe);
    }
    let target_fingerprint =
        if target_exists && !refusals.contains(&SaveRestoreRefusal::TargetSymlinkOrSpecial) {
            match hash_and_metadata(&binding.target_path) {
                Ok((_, hash, _)) => Some(hash),
                Err(_) => {
                    refusals.push(SaveRestoreRefusal::TargetPathUnsafe);
                    None
                }
            }
        } else {
            None
        };
    match quiescence {
        SaveQuiescenceRequirement::ConfirmedClosed => {}
        SaveQuiescenceRequirement::Running => refusals.push(SaveRestoreRefusal::EmulatorRunning),
        SaveQuiescenceRequirement::Unknown => {
            refusals.push(SaveRestoreRefusal::EmulatorStateUnknown)
        }
    }
    if verify_snapshot(snapshot).is_err() {
        refusals.push(SaveRestoreRefusal::SnapshotUnreadable);
    }

    SaveRestorePreflight {
        snapshot_id: snapshot.manifest.snapshot_id.clone(),
        snapshot_sha256,
        target_binding: binding.clone(),
        target_fingerprint,
        target_exists,
        quiescence,
        refusals,
    }
}

/// Apply one exact single-file native save.  The function creates and verifies
/// a pre-restore snapshot before touching the destination, stages beside the
/// destination's filesystem, publishes with rename, and records a durable
/// receipt.  It never applies a directory, memory card, save state, or opaque
/// artifact.
pub fn apply_single_file_restore(
    snapshot: &SaveSnapshot,
    preflight: &SaveRestorePreflight,
    options: &SaveRestoreApplyOptions,
) -> Result<SaveRestoreResult, SaveRestoreError> {
    if !preflight.ready() {
        return Err(SaveRestoreError::Refused(
            preflight
                .refusals
                .first()
                .cloned()
                .unwrap_or(SaveRestoreRefusal::TargetBindingMismatch),
        ));
    }
    let current = build_single_file_restore_preflight(
        snapshot,
        &preflight.target_binding,
        preflight.quiescence,
    );
    if current.snapshot_sha256 != preflight.snapshot_sha256
        || current.target_fingerprint != preflight.target_fingerprint
        || current.target_exists != preflight.target_exists
    {
        return Err(SaveRestoreError::Refused(
            if current.snapshot_sha256 != preflight.snapshot_sha256 {
                SaveRestoreRefusal::SnapshotHashChanged
            } else {
                SaveRestoreRefusal::DestinationChanged
            },
        ));
    }
    if !current.ready() {
        return Err(SaveRestoreError::Refused(
            current
                .refusals
                .first()
                .cloned()
                .unwrap_or(SaveRestoreRefusal::DestinationChanged),
        ));
    }

    let target = &preflight.target_binding.target_path;
    let parent = target
        .parent()
        .ok_or_else(|| SaveRestoreError::Refused(SaveRestoreRefusal::TargetPathUnsafe))?;
    ensure_directory(parent)?;
    ensure_directory(&options.backup_root)
        .map_err(|_| SaveRestoreError::Refused(SaveRestoreRefusal::BackupFailed))?;
    ensure_directory(&options.receipt_root)
        .map_err(|_| SaveRestoreError::Refused(SaveRestoreRefusal::ReceiptWriteFailed))?;
    verify_snapshot(snapshot)?;

    let transaction_id = format!(
        "save-restore-{}-{}",
        options.now_unix_seconds,
        short_path_token(target)
    );
    let backup_snapshot = if preflight.target_exists {
        let backup_request = SaveSnapshotRequest {
            location: SaveLocation {
                path: target.clone(),
                emulator: preflight.target_binding.emulator.clone(),
                profile: preflight.target_binding.profile.clone(),
                artifact_type: preflight.target_binding.artifact_type,
                provenance: SaveProvenance::ConfiguredPath,
            },
            game_identity: preflight.target_binding.game_identity.clone(),
            platform: None,
            storage_root: options.backup_root.clone(),
            available_space_bytes: None,
            snapshot_id: Some(format!("{transaction_id}-preimage")),
            now_unix_seconds: Some(options.now_unix_seconds),
            emulator_use: EmulatorUseStatus::NotDetected,
        };
        let backup = create_snapshot(&backup_request)
            .map_err(|_| SaveRestoreError::Refused(SaveRestoreRefusal::BackupFailed))?;
        verify_snapshot(&backup)
            .map_err(|_| SaveRestoreError::Refused(SaveRestoreRefusal::BackupFailed))?;
        Some(backup)
    } else {
        None
    };

    if !live_target_matches(preflight) {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::DestinationChanged,
        ));
    }

    let source = snapshot_file_path(snapshot)?;
    let stage = parent.join(format!(
        ".{}.emuwiz-restore-stage-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("save"),
        short_path_token(Path::new(&transaction_id))
    ));
    if stage.exists() {
        return Err(SaveRestoreError::Refused(SaveRestoreRefusal::StagingFailed));
    }
    if let Err(error) = copy_and_verify(&source, &stage, &preflight.snapshot_sha256) {
        let _ = fs::remove_file(&stage);
        return Err(error);
    }
    if !live_target_matches(preflight) {
        let _ = fs::remove_file(&stage);
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::DestinationChanged,
        ));
    }
    if let Err(error) = fs::rename(&stage, target) {
        let _ = fs::remove_file(&stage);
        return Err(SaveRestoreError::Io {
            path: target.clone(),
            detail: format!("atomic publication failed: {error}"),
        });
    }
    sync_parent(parent).map_err(|error| SaveRestoreError::Io {
        path: parent.to_path_buf(),
        detail: error,
    })?;

    let actual = match hash_and_metadata(target).map(|(_, hash, _)| hash) {
        Ok(actual) => actual,
        Err(error) => {
            let rollback =
                rollback_after_failed_restore(target, preflight, backup_snapshot.as_ref());
            return Err(if rollback.is_ok() {
                SaveRestoreError::Snapshot(error)
            } else {
                SaveRestoreError::Refused(SaveRestoreRefusal::RollbackFailed)
            });
        }
    };
    if actual != preflight.snapshot_sha256 {
        let rollback = rollback_after_failed_restore(target, preflight, backup_snapshot.as_ref());
        return Err(if rollback.is_ok() {
            SaveRestoreError::Refused(SaveRestoreRefusal::PostWriteVerificationFailed)
        } else {
            SaveRestoreError::Refused(SaveRestoreRefusal::RollbackFailed)
        });
    }

    let receipt_path = options.receipt_root.join(format!("{transaction_id}.json"));
    let receipt = SaveRestoreReceipt {
        transaction_id,
        snapshot_id: preflight.snapshot_id.clone(),
        snapshot_sha256: preflight.snapshot_sha256.clone(),
        target_binding: preflight.target_binding.clone(),
        pre_restore_target_sha256: preflight.target_fingerprint.clone(),
        backup_snapshot_id: backup_snapshot
            .as_ref()
            .map(|backup| backup.manifest.snapshot_id.clone()),
        backup_snapshot_path: backup_snapshot
            .as_ref()
            .map(|backup| backup.storage_path.clone()),
        post_restore_expected_sha256: preflight.snapshot_sha256.clone(),
        post_restore_actual_sha256: actual,
        quiescence: preflight.quiescence,
        applied_unix_seconds: options.now_unix_seconds,
        target_was_missing: !preflight.target_exists,
        rollback_eligible: true,
        receipt_path,
    };
    persist_receipt(&receipt).map_err(|error| {
        let rollback = rollback_after_failed_restore(target, preflight, backup_snapshot.as_ref());
        if rollback.is_ok() {
            error
        } else {
            SaveRestoreError::Refused(SaveRestoreRefusal::RollbackFailed)
        }
    })?;
    Ok(SaveRestoreResult {
        receipt,
        backup_snapshot,
    })
}

/// Undo a completed generic single-file restore only while the destination
/// still contains the bytes created by that transaction.
pub fn undo_single_file_restore(receipt: &SaveRestoreReceipt) -> Result<(), SaveRestoreError> {
    if !receipt.rollback_eligible {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::DestinationChanged,
        ));
    }
    let target = &receipt.target_binding.target_path;
    if !safe_target_path(target)
        || target
            .parent()
            .is_none_or(|parent| !safe_existing_parent(parent))
    {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::TargetPathUnsafe,
        ));
    }
    if fs::symlink_metadata(target)
        .map(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
        .unwrap_or(true)
    {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::TargetSymlinkOrSpecial,
        ));
    }
    let current = hash_and_metadata(target)
        .map(|(_, hash, _)| hash)
        .map_err(SaveRestoreError::Snapshot)?;
    if current != receipt.post_restore_actual_sha256 {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::DestinationChanged,
        ));
    }
    if receipt.target_was_missing {
        fs::remove_file(target).map_err(|error| SaveRestoreError::Io {
            path: target.clone(),
            detail: error.to_string(),
        })?;
        sync_parent(
            target
                .parent()
                .ok_or_else(|| SaveRestoreError::Refused(SaveRestoreRefusal::TargetPathUnsafe))?,
        )
        .map_err(|error| SaveRestoreError::Io {
            path: target.clone(),
            detail: error,
        })?;
        return Ok(());
    }
    let backup_root = receipt
        .backup_snapshot_path
        .as_deref()
        .ok_or(SaveRestoreError::Refused(SaveRestoreRefusal::BackupFailed))?;
    let backup = backup_root.join("files").join(
        target
            .file_name()
            .ok_or_else(|| SaveRestoreError::Refused(SaveRestoreRefusal::TargetPathUnsafe))?,
    );
    if fs::symlink_metadata(&backup)
        .map(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
        .unwrap_or(true)
    {
        return Err(SaveRestoreError::Refused(SaveRestoreRefusal::BackupFailed));
    }
    let backup_hash = hash_and_metadata(&backup)
        .map(|(_, hash, _)| hash)
        .map_err(SaveRestoreError::Snapshot)?;
    if receipt.pre_restore_target_sha256.as_deref() != Some(backup_hash.as_str()) {
        return Err(SaveRestoreError::Refused(SaveRestoreRefusal::BackupFailed));
    }
    let parent = target
        .parent()
        .ok_or_else(|| SaveRestoreError::Refused(SaveRestoreRefusal::TargetPathUnsafe))?;
    let stage = parent.join(format!(
        ".{}.emuwiz-undo-stage-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("save"),
        short_path_token(&receipt.receipt_path)
    ));
    copy_and_verify(&backup, &stage, &backup_hash)?;
    fs::rename(&stage, target).map_err(|error| SaveRestoreError::Io {
        path: target.clone(),
        detail: format!("atomic undo publication failed: {error}"),
    })?;
    sync_parent(parent).map_err(|error| SaveRestoreError::Io {
        path: parent.to_path_buf(),
        detail: error,
    })?;
    let restored = hash_and_metadata(target)
        .map(|(_, hash, _)| hash)
        .map_err(SaveRestoreError::Snapshot)?;
    if restored != backup_hash {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::PostWriteVerificationFailed,
        ));
    }
    Ok(())
}

fn snapshot_file_path(snapshot: &SaveSnapshot) -> Result<PathBuf, SaveRestoreError> {
    let artifact = snapshot
        .manifest
        .artifacts
        .first()
        .ok_or(SaveRestoreError::Refused(
            SaveRestoreRefusal::SnapshotUnreadable,
        ))?;
    safe_join(
        &snapshot.storage_path.join("files"),
        &artifact.relative_path,
    )
    .map_err(SaveRestoreError::Snapshot)
}

fn safe_target_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
}

fn safe_existing_parent(path: &Path) -> bool {
    let mut current = Some(path);
    while let Some(candidate) = current {
        let Ok(metadata) = fs::symlink_metadata(candidate) else {
            return false;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return false;
        }
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        current = Some(parent);
    }
    true
}

fn live_target_matches(preflight: &SaveRestorePreflight) -> bool {
    match (
        preflight.target_exists,
        fs::symlink_metadata(&preflight.target_binding.target_path),
    ) {
        (false, Err(error)) if error.kind() == io::ErrorKind::NotFound => true,
        (true, Ok(metadata)) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            hash_and_metadata(&preflight.target_binding.target_path)
                .map(|(_, hash, _)| Some(hash) == preflight.target_fingerprint)
                .unwrap_or(false)
        }
        _ => false,
    }
}

fn ensure_directory(path: &Path) -> Result<(), SaveRestoreError> {
    fs::create_dir_all(path).map_err(|error| SaveRestoreError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| SaveRestoreError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::TargetPathUnsafe,
        ));
    }
    Ok(())
}

fn copy_and_verify(
    source: &Path,
    destination: &Path,
    expected_sha256: &str,
) -> Result<(), SaveRestoreError> {
    let mut input = File::open(source).map_err(|error| SaveRestoreError::Io {
        path: source.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| SaveRestoreError::Io {
            path: destination.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| SaveRestoreError::Io {
                path: source.to_path_buf(),
                detail: error.to_string(),
            })?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| SaveRestoreError::Io {
                path: destination.to_path_buf(),
                detail: error.to_string(),
            })?;
        digest.update(&buffer[..count]);
    }
    output.sync_all().map_err(|error| SaveRestoreError::Io {
        path: destination.to_path_buf(),
        detail: error.to_string(),
    })?;
    let actual = hex_digest(digest);
    if actual != expected_sha256 {
        let _ = fs::remove_file(destination);
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::StagedHashMismatch,
        ));
    }
    Ok(())
}

fn rollback_after_failed_restore(
    target: &Path,
    preflight: &SaveRestorePreflight,
    backup: Option<&SaveSnapshot>,
) -> Result<(), SaveRestoreError> {
    let current = fs::symlink_metadata(target).map_err(|error| SaveRestoreError::Io {
        path: target.to_path_buf(),
        detail: error.to_string(),
    })?;
    if current.file_type().is_symlink() || !current.is_file() {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::TargetSymlinkOrSpecial,
        ));
    }
    if hash_and_metadata(target)
        .map(|(_, hash, _)| hash != preflight.snapshot_sha256)
        .unwrap_or(true)
    {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::DestinationChanged,
        ));
    }
    if !preflight.target_exists {
        if hash_and_metadata(target)
            .map(|(_, hash, _)| hash == preflight.snapshot_sha256)
            .unwrap_or(false)
        {
            fs::remove_file(target).map_err(|error| SaveRestoreError::Io {
                path: target.to_path_buf(),
                detail: error.to_string(),
            })?;
        }
        return Ok(());
    }
    let backup = backup.ok_or(SaveRestoreError::Refused(SaveRestoreRefusal::BackupFailed))?;
    let source = snapshot_file_path(backup)?;
    let parent = target
        .parent()
        .ok_or_else(|| SaveRestoreError::Refused(SaveRestoreRefusal::TargetPathUnsafe))?;
    let stage = parent.join(format!(
        ".{}.emuwiz-rollback-stage-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("save"),
        short_path_token(target)
    ));
    copy_and_verify(
        &source,
        &stage,
        preflight.target_fingerprint.as_deref().unwrap_or_default(),
    )?;
    fs::rename(stage, target).map_err(|error| SaveRestoreError::Io {
        path: target.to_path_buf(),
        detail: error.to_string(),
    })?;
    Ok(())
}

fn persist_receipt(receipt: &SaveRestoreReceipt) -> Result<(), SaveRestoreError> {
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|error| SaveRestoreError::Io {
        path: receipt.receipt_path.clone(),
        detail: error.to_string(),
    })?;
    let staging = receipt.receipt_path.with_extension("json.partial");
    write_file(&staging, &bytes).map_err(SaveRestoreError::Snapshot)?;
    fs::rename(&staging, &receipt.receipt_path).map_err(|error| SaveRestoreError::Io {
        path: receipt.receipt_path.clone(),
        detail: error.to_string(),
    })?;
    Ok(())
}

fn sync_parent(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())
}

fn validate_source(path: &Path) -> Result<PathBuf, SaveSnapshotError> {
    if !path.is_absolute() {
        return Err(SaveSnapshotError::InvalidPath {
            path: path.to_path_buf(),
            detail: "path must be absolute".into(),
        });
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(SaveSnapshotError::InvalidPath {
            path: path.to_path_buf(),
            detail: "symlink source is refused".into(),
        });
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(SaveSnapshotError::InvalidPath {
            path: path.to_path_buf(),
            detail: "not a regular file or directory".into(),
        });
    }
    Ok(path.to_path_buf())
}

fn collect_files(source: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>, SaveSnapshotError> {
    let metadata = fs::symlink_metadata(source).map_err(|error| io_error(source, error))?;
    if metadata.is_file() {
        return Ok(vec![(
            PathBuf::from(source.file_name().unwrap_or_default()),
            metadata,
        )]);
    }
    let mut files = Vec::new();
    collect_directory(source, source, 0, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn collect_directory(
    source: &Path,
    directory: &Path,
    depth: usize,
    files: &mut Vec<(PathBuf, fs::Metadata)>,
) -> Result<(), SaveSnapshotError> {
    if depth > MAX_SNAPSHOT_DEPTH {
        return Err(SaveSnapshotError::TooManyFiles);
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|error| io_error(directory, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_error(directory, error))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if files.len() >= MAX_SNAPSHOT_FILES {
            return Err(SaveSnapshotError::TooManyFiles);
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(SaveSnapshotError::InvalidPath {
                path,
                detail: "symlink inside save is refused".into(),
            });
        }
        if metadata.is_dir() {
            collect_directory(source, &path, depth + 1, files)?;
        } else if metadata.is_file() {
            files.push((
                path.strip_prefix(source).unwrap_or(&path).to_path_buf(),
                metadata,
            ));
        }
    }
    Ok(())
}

fn destination_artifact_path(destination: &Path, relative: &Path) -> PathBuf {
    if destination.is_file() {
        destination.to_path_buf()
    } else {
        destination.join(relative)
    }
}

fn build_snapshot_files(
    source: &Path,
    destination: &Path,
    entries: &[(PathBuf, fs::Metadata)],
) -> Result<Vec<SaveArtifact>, SaveSnapshotError> {
    let mut artifacts = Vec::with_capacity(entries.len());
    for (relative, before) in entries {
        let source_path = if source.is_file() {
            source.to_path_buf()
        } else {
            safe_join(source, relative)?
        };
        let destination_path = safe_join(destination, relative)?;
        if let Some(parent) = destination_path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        let mut input = File::open(&source_path).map_err(|error| io_error(&source_path, error))?;
        let mut output =
            File::create(&destination_path).map_err(|error| io_error(&destination_path, error))?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let count = input
                .read(&mut buffer)
                .map_err(|error| io_error(&source_path, error))?;
            if count == 0 {
                break;
            }
            output
                .write_all(&buffer[..count])
                .map_err(|error| io_error(&destination_path, error))?;
            digest.update(&buffer[..count]);
            size = size.saturating_add(count as u64);
            if size > MAX_SNAPSHOT_BYTES {
                return Err(SaveSnapshotError::TooLarge);
            }
        }
        output
            .sync_all()
            .map_err(|error| io_error(&destination_path, error))?;
        let after = fs::metadata(&source_path).map_err(|error| io_error(&source_path, error))?;
        if after.len() != before.len() || after.modified().ok() != before.modified().ok() {
            return Err(SaveSnapshotError::SourceChanged(source_path));
        }
        let modified_unix_seconds = after
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_secs());
        artifacts.push(SaveArtifact {
            relative_path: if source.is_file() {
                PathBuf::from(source.file_name().unwrap_or_default())
            } else {
                relative.clone()
            },
            size_bytes: size,
            sha256: hex_digest(digest),
            modified_unix_seconds,
        });
    }
    Ok(artifacts)
}

fn hash_and_metadata(path: &Path) -> Result<(u64, String, Option<u64>), SaveSnapshotError> {
    let metadata = fs::metadata(path).map_err(|error| io_error(path, error))?;
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((
        size,
        hex_digest(digest),
        metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_secs()),
    ))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), SaveSnapshotError> {
    let mut file = File::create(path).map_err(|error| io_error(path, error))?;
    file.write_all(bytes)
        .map_err(|error| io_error(path, error))?;
    file.sync_all().map_err(|error| io_error(path, error))
}

fn safe_join(root: &Path, relative: &Path) -> Result<PathBuf, SaveSnapshotError> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(SaveSnapshotError::InvalidPath {
            path: relative.to_path_buf(),
            detail: "relative path escapes root".into(),
        });
    }
    Ok(root.join(relative))
}

fn io_error(path: &Path, error: io::Error) -> SaveSnapshotError {
    SaveSnapshotError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    }
}
fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
fn short_path_token(path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(path.to_string_lossy().as_bytes());
    hex_digest(digest)[..12].to_string()
}
fn hex_digest(digest: Sha256) -> String {
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    fn request(source: &Path, storage: &Path) -> SaveSnapshotRequest {
        SaveSnapshotRequest {
            location: SaveLocation {
                path: source.to_path_buf(),
                emulator: Some("TestEmulator".into()),
                profile: Some("test-profile".into()),
                artifact_type: SaveArtifactType::Sram,
                provenance: SaveProvenance::UserSpecified,
            },
            game_identity: Some("game-1".into()),
            platform: Some("Test".into()),
            storage_root: storage.to_path_buf(),
            available_space_bytes: Some(1024 * 1024),
            snapshot_id: Some("snapshot-1".into()),
            now_unix_seconds: Some(100),
            emulator_use: EmulatorUseStatus::NotDetected,
        }
    }

    #[test]
    fn single_file_snapshot_hashes_and_verifies() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save.srm");
        fs::write(&source, b"save").unwrap();
        let snapshot = create_snapshot(&request(&source, &dir.path().join("snapshots"))).unwrap();
        assert_eq!(snapshot.manifest.artifacts.len(), 1);
        verify_snapshot(&snapshot).unwrap();
        assert!(snapshot.storage_path.join("manifest.json").is_file());
    }

    #[test]
    fn directory_snapshot_is_deterministic_and_save_states_remain_distinct() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("savedata");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("slot10"), b"10").unwrap();
        fs::write(source.join("slot2"), b"2").unwrap();
        let mut first = request(&source, &dir.path().join("a"));
        first.location.artifact_type = SaveArtifactType::SaveDirectory;
        first.snapshot_id = Some("first".into());
        let mut second = request(&source, &dir.path().join("b"));
        second.location.artifact_type = SaveArtifactType::SaveState;
        second.snapshot_id = Some("second".into());
        let one = create_snapshot(&first).unwrap();
        let two = create_snapshot(&second).unwrap();
        let paths = one
            .manifest
            .artifacts
            .iter()
            .map(|a| &a.relative_path)
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            two.manifest
                .artifacts
                .iter()
                .map(|a| &a.relative_path)
                .collect::<Vec<_>>()
        );
        assert!(two.manifest.artifact_type.is_save_state());
    }

    #[test]
    fn symlinks_and_insufficient_space_refuse_without_a_complete_snapshot() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save");
        fs::write(&source, b"save").unwrap();
        let link = dir.path().join("link");
        symlink(&source, &link).unwrap();
        assert!(matches!(
            create_snapshot(&request(&link, &dir.path().join("snapshots"))),
            Err(SaveSnapshotError::InvalidPath { .. })
        ));
        let mut low = request(&source, &dir.path().join("snapshots"));
        low.available_space_bytes = Some(0);
        assert!(matches!(
            create_snapshot(&low),
            Err(SaveSnapshotError::InsufficientSpace { .. })
        ));
    }

    #[test]
    fn restore_preview_reports_change_and_requires_pre_restore_snapshot() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save.srm");
        fs::write(&source, b"old").unwrap();
        let snapshot = create_snapshot(&request(&source, &dir.path().join("snapshots"))).unwrap();
        fs::write(&source, b"new").unwrap();
        let plan = build_restore_plan(
            &snapshot,
            &request(&source, &dir.path().join("snapshots")).location,
            EmulatorUseStatus::NotDetected,
        );
        assert!(plan.automatic_pre_restore_snapshot_required);
        assert!(!plan.apply_supported);
        assert!(!plan.files_to_replace.is_empty());
    }

    #[test]
    fn running_or_unknown_emulator_is_not_safe() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save");
        fs::write(&source, b"save").unwrap();
        let mut request = request(&source, &dir.path().join("snapshots"));
        request.emulator_use = EmulatorUseStatus::Running;
        assert!(matches!(
            create_snapshot(&request),
            Err(SaveSnapshotError::EmulatorRunning)
        ));
        assert_eq!(
            assess_snapshot_readiness(&request.location, Some(10), 1, EmulatorUseStatus::Unknown),
            SaveSnapshotReadiness::EmulatorUseUnknown
        );
    }

    #[test]
    fn profile_mismatch_is_explicit() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save");
        fs::write(&source, b"save").unwrap();
        let snapshot = create_snapshot(&request(&source, &dir.path().join("snapshots"))).unwrap();
        let mut destination = request(&source, &dir.path().join("snapshots")).location;
        destination.profile = Some("other".into());
        let plan = build_restore_plan(&snapshot, &destination, EmulatorUseStatus::NotDetected);
        assert!(
            plan.conflicts
                .contains(&SaveRestoreConflict::ProfileMismatch)
        );
    }

    fn restore_options(root: &Path) -> SaveRestoreApplyOptions {
        SaveRestoreApplyOptions {
            backup_root: root.join("backups"),
            receipt_root: root.join("receipts"),
            now_unix_seconds: 200,
        }
    }

    #[test]
    fn single_file_restore_backs_up_publishes_and_undoes_exact_bytes() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save.srm");
        fs::write(&source, b"before").unwrap();
        let snapshot = create_snapshot(&request(&source, &dir.path().join("snapshots"))).unwrap();
        fs::write(&source, b"changed").unwrap();
        let binding = SaveTargetBinding::from_snapshot(&snapshot);
        let preflight = build_single_file_restore_preflight(
            &snapshot,
            &binding,
            SaveQuiescenceRequirement::ConfirmedClosed,
        );
        assert!(preflight.ready());
        let result =
            apply_single_file_restore(&snapshot, &preflight, &restore_options(dir.path())).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"before");
        assert!(result.receipt.rollback_eligible);
        assert!(result.receipt.backup_snapshot_path.is_some());
        assert!(result.receipt.receipt_path.is_file());
        undo_single_file_restore(&result.receipt).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"changed");
    }

    #[test]
    fn single_file_restore_missing_target_is_removed_by_undo() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save.srm");
        fs::write(&source, b"save").unwrap();
        let snapshot = create_snapshot(&request(&source, &dir.path().join("snapshots"))).unwrap();
        fs::remove_file(&source).unwrap();
        let preflight = build_single_file_restore_preflight(
            &snapshot,
            &SaveTargetBinding::from_snapshot(&snapshot),
            SaveQuiescenceRequirement::ConfirmedClosed,
        );
        let result =
            apply_single_file_restore(&snapshot, &preflight, &restore_options(dir.path())).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"save");
        undo_single_file_restore(&result.receipt).unwrap();
        assert!(!source.exists());
    }

    #[test]
    fn single_file_restore_refuses_stale_destination_and_external_undo() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save.srm");
        fs::write(&source, b"before").unwrap();
        let snapshot = create_snapshot(&request(&source, &dir.path().join("snapshots"))).unwrap();
        fs::write(&source, b"changed-before-preview").unwrap();
        let preflight = build_single_file_restore_preflight(
            &snapshot,
            &SaveTargetBinding::from_snapshot(&snapshot),
            SaveQuiescenceRequirement::ConfirmedClosed,
        );
        fs::write(&source, b"changed-after-preview").unwrap();
        assert!(matches!(
            apply_single_file_restore(&snapshot, &preflight, &restore_options(dir.path())),
            Err(SaveRestoreError::Refused(
                SaveRestoreRefusal::DestinationChanged
            ))
        ));

        let fresh = build_single_file_restore_preflight(
            &snapshot,
            &SaveTargetBinding::from_snapshot(&snapshot),
            SaveQuiescenceRequirement::ConfirmedClosed,
        );
        let result =
            apply_single_file_restore(&snapshot, &fresh, &restore_options(dir.path())).unwrap();
        fs::write(&source, b"external-change").unwrap();
        assert!(matches!(
            undo_single_file_restore(&result.receipt),
            Err(SaveRestoreError::Refused(
                SaveRestoreRefusal::DestinationChanged
            ))
        ));
    }

    #[test]
    fn single_file_restore_refuses_unsafe_families_and_quiescence() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("card.mcr");
        fs::write(&source, b"card").unwrap();
        let mut card_request = request(&source, &dir.path().join("snapshots"));
        card_request.location.artifact_type = SaveArtifactType::MemoryCard;
        let card = create_snapshot(&card_request).unwrap();
        let card_preflight = build_single_file_restore_preflight(
            &card,
            &SaveTargetBinding::from_snapshot(&card),
            SaveQuiescenceRequirement::ConfirmedClosed,
        );
        assert!(
            card_preflight
                .refusals
                .contains(&SaveRestoreRefusal::SharedContainerRestoreNotSupported)
        );

        let mut state_request = request(&source, &dir.path().join("states"));
        state_request.location.artifact_type = SaveArtifactType::SaveState;
        let state = create_snapshot(&state_request).unwrap();
        let state_preflight = build_single_file_restore_preflight(
            &state,
            &SaveTargetBinding::from_snapshot(&state),
            SaveQuiescenceRequirement::Unknown,
        );
        assert!(
            state_preflight
                .refusals
                .contains(&SaveRestoreRefusal::SaveStateRestoreNotSupported)
        );
        assert!(
            state_preflight
                .refusals
                .contains(&SaveRestoreRefusal::EmulatorStateUnknown)
        );
    }

    #[test]
    fn single_file_restore_refuses_changed_snapshot_and_symlink_target() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("save.srm");
        fs::write(&source, b"save").unwrap();
        let snapshot = create_snapshot(&request(&source, &dir.path().join("snapshots"))).unwrap();
        let snapshot_file = snapshot.storage_path.join("files").join("save.srm");
        fs::write(snapshot_file, b"tampered").unwrap();
        let changed = build_single_file_restore_preflight(
            &snapshot,
            &SaveTargetBinding::from_snapshot(&snapshot),
            SaveQuiescenceRequirement::ConfirmedClosed,
        );
        assert!(
            changed
                .refusals
                .contains(&SaveRestoreRefusal::SnapshotUnreadable)
        );

        let replacement = dir.path().join("replacement.srm");
        fs::write(&replacement, b"replacement").unwrap();
        let link = dir.path().join("link.srm");
        symlink(&replacement, &link).unwrap();
        let mut link_binding = SaveTargetBinding::from_snapshot(&snapshot);
        link_binding.target_path = link;
        let link_preflight = build_single_file_restore_preflight(
            &snapshot,
            &link_binding,
            SaveQuiescenceRequirement::ConfirmedClosed,
        );
        assert!(
            link_preflight
                .refusals
                .contains(&SaveRestoreRefusal::SnapshotAssociationMismatch)
        );
        assert!(
            link_preflight
                .refusals
                .contains(&SaveRestoreRefusal::TargetSymlinkOrSpecial)
        );
    }
}
