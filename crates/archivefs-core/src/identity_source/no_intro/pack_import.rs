//! Transactional, browser-assisted No-Intro pack import.
//!
//! DAT-o-MATIC is intentionally transport-free here: the user supplies a ZIP
//! downloaded through the official site. This module validates the ZIP and
//! its DAT contents, then publishes one content-addressed snapshot. It never
//! performs HTTP, scrapes a page, or interprets a filename as authority.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use super::import::{ImportedNoIntroSource, NoIntroImportError, import_no_intro_dat};

/// Official download page for the user/browser handoff. This is metadata
/// only; no request is made by this module.
pub const NO_INTRO_DATOMATIC_DOWNLOAD_PAGE: &str = "https://datomatic.no-intro.org/";
pub const NO_INTRO_PACK_SCHEMA_VERSION: u32 = 1;
pub const NO_INTRO_PACK_MAX_BYTES: u64 = 512 * 1024 * 1024;
pub const NO_INTRO_PACK_MAX_MEMBERS: usize = 10_000;
pub const NO_INTRO_PACK_MAX_MEMBER_NAME_BYTES: usize = 4 * 1024;
pub const NO_INTRO_PACK_MAX_DAT_BYTES: u64 = 64 * 1024 * 1024;
pub const NO_INTRO_PACK_MAX_TOTAL_DAT_BYTES: u64 = 256 * 1024 * 1024;
const PACK_DIRECTORY: &str = "no_intro_pack";

#[derive(Debug)]
pub enum NoIntroPackImportError {
    Io {
        path: PathBuf,
        error: io::Error,
    },
    InvalidArchive {
        detail: String,
    },
    LimitExceeded {
        detail: String,
    },
    Traversal {
        member: String,
    },
    CorruptDat {
        member: String,
        error: NoIntroImportError,
    },
    IncompleteDat {
        member: String,
        detail: String,
    },
    State(String),
}

impl fmt::Display for NoIntroPackImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::InvalidArchive { detail } => write!(f, "invalid No-Intro pack ZIP: {detail}"),
            Self::LimitExceeded { detail } => write!(f, "No-Intro pack limit exceeded: {detail}"),
            Self::Traversal { member } => write!(f, "unsafe ZIP member path: {member}"),
            Self::CorruptDat { member, error } => {
                write!(
                    f,
                    "No-Intro DAT member {member} could not be validated: {error}"
                )
            }
            Self::IncompleteDat { member, detail } => {
                write!(f, "No-Intro DAT member {member} is incomplete: {detail}")
            }
            Self::State(detail) => write!(f, "No-Intro pack state error: {detail}"),
        }
    }
}

impl std::error::Error for NoIntroPackImportError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoIntroPackImportStatus {
    Unchanged,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedNoIntroPackMember {
    pub member: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct NoIntroPackImportReport {
    pub status: NoIntroPackImportStatus,
    pub pack_sha256: String,
    pub snapshot_path: PathBuf,
    pub accepted: Vec<ImportedNoIntroSource>,
    pub rejected: Vec<RejectedNoIntroPackMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoIntroPackState {
    schema_version: u32,
    pack_sha256: String,
    accepted_members: Vec<NoIntroPackStateMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoIntroPackStateMember {
    member: String,
    artifact_sha256: String,
    system_name: String,
    variant: super::import::NoIntroVariant,
    upstream_version: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct PackLimits;

impl PackLimits {
    fn validate_archive(&self, path: &Path, bytes: u64) -> Result<(), NoIntroPackImportError> {
        if bytes > NO_INTRO_PACK_MAX_BYTES {
            return Err(NoIntroPackImportError::LimitExceeded {
                detail: format!("ZIP is {bytes} bytes; maximum is {NO_INTRO_PACK_MAX_BYTES}"),
            });
        }
        if !path.is_absolute() {
            return Err(NoIntroPackImportError::InvalidArchive {
                detail: "pack path must be absolute".to_string(),
            });
        }
        Ok(())
    }
}

/// Imports a user-provided No-Intro ZIP into the production app-data area.
pub fn import_no_intro_pack(
    path: &Path,
) -> Result<NoIntroPackImportReport, NoIntroPackImportError> {
    let root = crate::app_dirs::data_path(PACK_DIRECTORY).map_err(|error| {
        NoIntroPackImportError::State(format!("cannot resolve application data path: {error}"))
    })?;
    import_no_intro_pack_at(path, &root)
}

/// Testable/local-pack seam. `storage_root` is the complete app-owned store
/// for this source and may point at a temporary directory in tests.
pub fn import_no_intro_pack_at(
    path: &Path,
    storage_root: &Path,
) -> Result<NoIntroPackImportReport, NoIntroPackImportError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| NoIntroPackImportError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(NoIntroPackImportError::InvalidArchive {
            detail: "pack must be a regular, non-symlink file".to_string(),
        });
    }
    PackLimits.validate_archive(path, metadata.len())?;
    let pack_sha256 = sha256_file(path).map_err(|error| NoIntroPackImportError::Io {
        path: path.to_path_buf(),
        error,
    })?;

    ensure_directory(storage_root)?;
    let state_path = storage_root.join("state.json");
    if let Ok(state) = load_state(&state_path) {
        if state.schema_version == NO_INTRO_PACK_SCHEMA_VERSION && state.pack_sha256 == pack_sha256
        {
            let snapshot_path = storage_root.join("snapshots").join(&pack_sha256);
            if snapshot_is_complete(&snapshot_path, &state.accepted_members) {
                let accepted = load_sources(&snapshot_path, &state.accepted_members)?;
                return Ok(NoIntroPackImportReport {
                    status: NoIntroPackImportStatus::Unchanged,
                    pack_sha256,
                    snapshot_path,
                    accepted,
                    rejected: Vec::new(),
                });
            }
        }
    }

    let staging = storage_root.join(format!(".staging-{}-{}", std::process::id(), unique_id()));
    let result = build_staged_snapshot(path, &staging);
    let (accepted, rejected) = match result {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };

    let snapshot_path = storage_root.join("snapshots").join(&pack_sha256);
    fs::create_dir_all(snapshot_path.parent().unwrap())
        .map_err(|error| io_error(&snapshot_path, error))?;
    if snapshot_path.exists() {
        if !snapshot_matches_sources(&snapshot_path, &accepted) {
            let _ = fs::remove_dir_all(&staging);
            return Err(NoIntroPackImportError::State(
                "content-addressed snapshot directory already exists but does not match the +                 imported pack"
                    .to_string(),
            ));
        }
        let _ = fs::remove_dir_all(&staging);
    } else if let Err(error) = fs::rename(&staging, &snapshot_path) {
        let _ = fs::remove_dir_all(&staging);
        return Err(io_error(&snapshot_path, error));
    }

    let accepted_members = accepted
        .iter()
        .map(|source| NoIntroPackStateMember {
            member: source.artifact_name.clone(),
            artifact_sha256: source.artifact_sha256.clone(),
            system_name: source.system_name.clone(),
            variant: source.variant,
            upstream_version: source.upstream_version.clone(),
        })
        .collect();
    let state = NoIntroPackState {
        schema_version: NO_INTRO_PACK_SCHEMA_VERSION,
        pack_sha256: pack_sha256.clone(),
        accepted_members,
    };
    let body = serde_json::to_string_pretty(&state)
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?;
    if let Err(error) = crate::atomic_write_text(&state_path, &format!("{body}\n")) {
        return Err(NoIntroPackImportError::State(error.to_string()));
    }
    prune_old_snapshots(storage_root, &pack_sha256);

    let accepted = accepted
        .into_iter()
        .enumerate()
        .map(|(index, mut source)| {
            source.artifact_path = snapshot_path.join("dats").join(format!("{index}.dat"));
            source
        })
        .collect();
    Ok(NoIntroPackImportReport {
        status: NoIntroPackImportStatus::Updated,
        pack_sha256,
        snapshot_path,
        accepted,
        rejected,
    })
}

/// Loads the currently published pack after a process restart. The files are
/// re-parsed through the ordinary No-Intro importer, so a damaged snapshot is
/// never silently returned as usable evidence.
pub fn load_current_no_intro_pack_at(
    storage_root: &Path,
) -> Result<Option<Vec<ImportedNoIntroSource>>, NoIntroPackImportError> {
    let state_path = storage_root.join("state.json");
    let state = match load_state(&state_path) {
        Ok(state) => state,
        Err(NoIntroPackImportError::Io { error, .. })
            if error.kind() == io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if state.schema_version != NO_INTRO_PACK_SCHEMA_VERSION {
        return Err(NoIntroPackImportError::State(format!(
            "unsupported No-Intro pack schema version {}",
            state.schema_version
        )));
    }
    let snapshot = storage_root.join("snapshots").join(&state.pack_sha256);
    if !snapshot_is_complete(&snapshot, &state.accepted_members) {
        return Err(NoIntroPackImportError::State(
            "published No-Intro pack snapshot is incomplete".to_string(),
        ));
    }
    Ok(Some(load_sources(&snapshot, &state.accepted_members)?))
}

pub fn load_current_no_intro_pack()
-> Result<Option<Vec<ImportedNoIntroSource>>, NoIntroPackImportError> {
    let root = crate::app_dirs::data_path(PACK_DIRECTORY).map_err(|error| {
        NoIntroPackImportError::State(format!("cannot resolve application data path: {error}"))
    })?;
    load_current_no_intro_pack_at(&root)
}

fn build_staged_snapshot(
    pack_path: &Path,
    staging: &Path,
) -> Result<(Vec<ImportedNoIntroSource>, Vec<RejectedNoIntroPackMember>), NoIntroPackImportError> {
    fs::create_dir_all(staging.join("dats")).map_err(|error| io_error(staging, error))?;
    let file = File::open(pack_path).map_err(|error| io_error(pack_path, error))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| NoIntroPackImportError::InvalidArchive {
            detail: error.to_string(),
        })?;
    if archive.len() > NO_INTRO_PACK_MAX_MEMBERS {
        return Err(NoIntroPackImportError::LimitExceeded {
            detail: format!(
                "archive has {} members; maximum is {NO_INTRO_PACK_MAX_MEMBERS}",
                archive.len()
            ),
        });
    }

    let mut total_dat_bytes = 0_u64;
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for index in 0..archive.len() {
        let (member, is_dir, size) = {
            let raw = archive.by_index_raw(index).map_err(|error| {
                NoIntroPackImportError::InvalidArchive {
                    detail: error.to_string(),
                }
            })?;
            (raw.name().to_string(), raw.is_dir(), raw.size())
        };
        validate_member_name(&member)?;
        if is_dir || !is_dat_name(&member) {
            continue;
        }
        if size > NO_INTRO_PACK_MAX_DAT_BYTES {
            return Err(NoIntroPackImportError::LimitExceeded {
                detail: format!("DAT member {member} is {size} bytes"),
            });
        }
        total_dat_bytes = total_dat_bytes.saturating_add(size);
        if total_dat_bytes > NO_INTRO_PACK_MAX_TOTAL_DAT_BYTES {
            return Err(NoIntroPackImportError::LimitExceeded {
                detail: format!("DAT members exceed {NO_INTRO_PACK_MAX_TOTAL_DAT_BYTES} bytes"),
            });
        }
        let output_name = format!("{index}.dat");
        let output_path = staging.join("dats").join(&output_name);
        let mut input =
            archive
                .by_index(index)
                .map_err(|error| NoIntroPackImportError::InvalidArchive {
                    detail: error.to_string(),
                })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| io_error(&output_path, error))?;
        let mut limited = (&mut input).take(NO_INTRO_PACK_MAX_DAT_BYTES + 1);
        io::copy(&mut limited, &mut output).map_err(|error| io_error(&output_path, error))?;
        output
            .flush()
            .map_err(|error| io_error(&output_path, error))?;
        output
            .sync_all()
            .map_err(|error| io_error(&output_path, error))?;

        match import_no_intro_dat(&output_path) {
            Ok(mut source) => {
                if source
                    .dat
                    .source
                    .parse_warnings
                    .iter()
                    .any(|warning| warning.to_ascii_lowercase().contains("truncated"))
                {
                    return Err(NoIntroPackImportError::IncompleteDat {
                        member,
                        detail: "parser reported a truncated document".to_string(),
                    });
                }
                source.artifact_name = member.clone();
                source.artifact_path = output_path;
                accepted.push(source);
            }
            Err(NoIntroImportError::NotNoIntro {
                detected_ecosystem, ..
            }) => {
                let _ = fs::remove_file(&output_path);
                rejected.push(RejectedNoIntroPackMember {
                    member,
                    reason: format!("content identifies as {detected_ecosystem:?}, not No-Intro"),
                });
            }
            Err(error) => {
                return Err(NoIntroPackImportError::CorruptDat { member, error });
            }
        }
    }
    Ok((accepted, rejected))
}

fn load_sources(
    snapshot: &Path,
    members: &[NoIntroPackStateMember],
) -> Result<Vec<ImportedNoIntroSource>, NoIntroPackImportError> {
    let mut sources = Vec::new();
    for (index, member) in members.iter().enumerate() {
        let path = snapshot.join("dats").join(format!("{index}.dat"));
        let mut source =
            import_no_intro_dat(&path).map_err(|error| NoIntroPackImportError::CorruptDat {
                member: member.member.clone(),
                error,
            })?;
        source.artifact_name = member.member.clone();
        source.artifact_path = path;
        sources.push(source);
    }
    Ok(sources)
}

fn snapshot_is_complete(snapshot: &Path, members: &[NoIntroPackStateMember]) -> bool {
    snapshot.is_dir()
        && members.iter().enumerate().all(|(index, member)| {
            let path = snapshot.join("dats").join(format!("{index}.dat"));
            path.is_file() && sha256_file(&path).is_ok_and(|sha| sha == member.artifact_sha256)
        })
}

fn snapshot_matches_sources(snapshot: &Path, sources: &[ImportedNoIntroSource]) -> bool {
    snapshot.is_dir()
        && sources.iter().enumerate().all(|(index, source)| {
            let path = snapshot.join("dats").join(format!("{index}.dat"));
            path.is_file() && sha256_file(&path).is_ok_and(|sha| sha == source.artifact_sha256)
        })
}

fn prune_old_snapshots(storage_root: &Path, current_sha256: &str) {
    let snapshots = storage_root.join("snapshots");
    let Ok(entries) = fs::read_dir(&snapshots) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) != Some(current_sha256) {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn validate_member_name(member: &str) -> Result<(), NoIntroPackImportError> {
    if member.is_empty() || member.len() > NO_INTRO_PACK_MAX_MEMBER_NAME_BYTES {
        return Err(NoIntroPackImportError::Traversal {
            member: member.to_string(),
        });
    }
    if member.contains('\\') {
        return Err(NoIntroPackImportError::Traversal {
            member: member.to_string(),
        });
    }
    let path = Path::new(member);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(NoIntroPackImportError::Traversal {
            member: member.to_string(),
        });
    }
    Ok(())
}

fn is_dat_name(member: &str) -> bool {
    Path::new(member)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("dat"))
}

fn ensure_directory(path: &Path) -> Result<(), NoIntroPackImportError> {
    fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(NoIntroPackImportError::State(format!(
            "storage path is not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn load_state(path: &Path) -> Result<NoIntroPackState, NoIntroPackImportError> {
    let text = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    serde_json::from_str(&text).map_err(|error| NoIntroPackImportError::State(error.to_string()))
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn unique_id() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn io_error(path: &Path, error: io::Error) -> NoIntroPackImportError {
    NoIntroPackImportError::Io {
        path: path.to_path_buf(),
        error,
    }
}

#[cfg(test)]
mod tests;
