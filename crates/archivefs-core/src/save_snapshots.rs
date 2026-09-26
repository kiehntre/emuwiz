//! Local immutable save snapshots and conservative restore previews.
//!
//! Snapshot creation copies and hashes local save data into an EmuWiz data
//! directory. Restore planning is read-only. Generic restore apply remains
//! disabled until a complete filesystem transaction is available.

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
}
