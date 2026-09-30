//! Catalogue presence is separate from mount health and game identity.
//! Reports are read-only; move candidates never authorize relinking.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::database::{Database, PersistedArchive};
use crate::emulator_environment::FsProbe;
use crate::launch::{CanonicalIdentityStatus, canonical_identity_from_game_report};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanCoverageState {
    Complete,
    Partial,
    Unavailable,
    Failed,
    Skipped,
    Removed,
    NotAttempted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceScanCoverage {
    pub source_id: i64,
    pub root_identity: Option<(u64, u64)>,
    pub root: PathBuf,
    pub state: ScanCoverageState,
    /// Intentionally excluded nested configured sources. They have their own
    /// coverage records and cannot be reconciled through their parent.
    pub excluded_roots: Vec<PathBuf>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogueHealth {
    PresentVerified,
    PresentNotVerified,
    PossiblyMoved,
    Missing,
    OrphanedSource,
    NotChecked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathObservation {
    pub probe: FsProbe,
    pub size: Option<u64>,
    pub modified: Option<i64>,
}

pub(crate) fn source_root_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.is_dir() && !metadata.file_type().is_symlink())
        .then(|| (metadata.dev(), metadata.ino()))
}

pub(crate) fn observe_path(path: &Path, directory: bool) -> PathObservation {
    let mut result = PathObservation {
        probe: FsProbe::IoError,
        size: None,
        modified: None,
    };
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            result.probe = if meta.file_type().is_symlink() {
                FsProbe::Symlink
            } else if directory && meta.is_dir() {
                FsProbe::PresentDirectory
            } else if !directory && meta.is_file() {
                FsProbe::PresentFile
            } else {
                FsProbe::WrongType
            };
            result.size = Some(meta.len());
            result.modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|t| i64::try_from(t.as_secs()).ok());
        }
        Err(e) => {
            result.probe = match e.kind() {
                std::io::ErrorKind::NotFound => FsProbe::Missing,
                std::io::ErrorKind::PermissionDenied => FsProbe::Inaccessible,
                _ => FsProbe::IoError,
            }
        }
    }
    result
}

impl PathObservation {
    pub fn is_present(&self) -> bool {
        matches!(self.probe, FsProbe::PresentFile | FsProbe::PresentDirectory)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveEvidence {
    BasenameOnly,
    BasenameAndSize,
    /// Historical exact-file SHA-256 equals a freshly hashed candidate.
    /// Still a review candidate; the path is never rewritten here.
    VerifiedSha256,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveCandidate {
    pub archive_id: i64,
    pub path: PathBuf,
    pub evidence: MoveEvidence,
    pub basename_exact: bool,
    pub same_platform: bool,
    pub same_source: bool,
    pub same_relative_path: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueHealthRow {
    pub archive: PersistedArchive,
    pub observation: PathObservation,
    pub health: CatalogueHealth,
    pub move_candidates: Vec<MoveCandidate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CatalogueHealthCounts {
    pub total: usize,
    pub present_stale_flag: usize,
    pub present_clean: usize,
    pub present_verified: usize,
    pub present_not_verified: usize,
    pub possibly_moved: usize,
    pub missing: usize,
    pub orphaned_source: usize,
    pub not_checked: usize,
    /// Cross-cutting counts include absent orphaned rows too.
    pub rows_with_move_candidates: usize,
    pub ambiguous_move_candidates: usize,
    pub strong_move_candidates: usize,
    pub rows_would_change: usize,
    pub rows_left_untouched: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueHealthReport {
    pub counts: CatalogueHealthCounts,
    pub rows: Vec<CatalogueHealthRow>,
    pub(crate) database_path: PathBuf,
}

fn name_key(name: &std::ffi::OsStr) -> Vec<u8> {
    name.as_encoded_bytes()
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect()
}

fn sha256(path: &Path, expected: &PathObservation) -> Option<String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes).ok()?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    (observe_path(path, false) == *expected)
        .then(|| hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Index only observed catalogue paths, once. No per-row directory walks and
/// no hashing unless a historical SHA-256 can actually confirm a candidate.
/// `Missing` means the recorded path is absent with no indexed candidate,
/// not that every possible storage device has been searched.
pub fn preview_catalogue_health(
    database: &Database,
    configured_roots: &[PathBuf],
) -> Result<CatalogueHealthReport> {
    let archives = database.load_archives()?;
    let sources = database.initial_scan_coverage(&[])?;
    let hashes = database.catalogue_archive_hashes()?;
    let source_state: HashMap<_, _> = sources
        .iter()
        .map(|s| {
            let configured = configured_roots.contains(&s.root);
            let available = configured
                && observe_path(&s.root, true).is_present()
                && fs::read_dir(&s.root).is_ok();
            (s.source_id, (configured, available))
        })
        .collect();
    let observations: Vec<_> = archives
        .iter()
        .map(|a| observe_path(&a.absolute_path, a.archive_kind == "arcade_set_directory"))
        .collect();
    let mut by_name = HashMap::<_, Vec<usize>>::new();
    let mut by_hash = HashMap::<_, Vec<usize>>::new();
    for (i, a) in archives
        .iter()
        .enumerate()
        .filter(|(i, _)| observations[*i].is_present())
    {
        if let Some(name) = a.absolute_path.file_name() {
            by_name.entry(name_key(name)).or_default().push(i);
        }
        if let Some(hash) = hashes.get(&a.id) {
            by_hash.entry(hash.clone()).or_default().push(i);
        }
    }
    let mut hash_cache = HashMap::new();
    let mut counts = CatalogueHealthCounts::default();
    let mut rows = Vec::with_capacity(archives.len());
    for (i, archive) in archives.iter().enumerate() {
        let observation = &observations[i];
        let mut candidates = BTreeMap::new();
        if observation.probe == FsProbe::Missing {
            if let Some(indices) = archive
                .absolute_path
                .file_name()
                .and_then(|n| by_name.get(&name_key(n)))
            {
                for &j in indices {
                    candidates.insert(
                        j,
                        if archive.size_bytes.is_some()
                            && archive.size_bytes == observations[j].size
                        {
                            MoveEvidence::BasenameAndSize
                        } else {
                            MoveEvidence::BasenameOnly
                        },
                    );
                }
            }
            if let Some(expected) = hashes.get(&archive.id) {
                let mut indices: Vec<_> = candidates.keys().copied().collect();
                indices.extend(by_hash.get(expected).into_iter().flatten().copied());
                indices.sort_unstable();
                indices.dedup();
                for j in indices {
                    if observations[j].probe != FsProbe::PresentFile
                        || archive
                            .size_bytes
                            .is_some_and(|size| observations[j].size != Some(size))
                    {
                        continue;
                    }
                    let actual = hash_cache
                        .entry(archives[j].absolute_path.clone())
                        .or_insert_with(|| sha256(&archives[j].absolute_path, &observations[j]));
                    if actual.as_ref() == Some(expected) {
                        candidates.insert(j, MoveEvidence::VerifiedSha256);
                    }
                }
            }
        }
        let move_candidates: Vec<_> = candidates
            .into_iter()
            .map(|(j, evidence)| MoveCandidate {
                archive_id: archives[j].id,
                path: archives[j].absolute_path.clone(),
                evidence,
                basename_exact: archive.absolute_path.file_name()
                    == archives[j].absolute_path.file_name(),
                same_platform: archive.platform.is_some()
                    && archive.platform == archives[j].platform,
                same_source: archive.source_folder_id == archives[j].source_folder_id,
                same_relative_path: archive.relative_path == archives[j].relative_path,
            })
            .collect();
        let source = source_state.get(&archive.source_folder_id);
        let health = if source.is_some_and(|(configured, _)| !configured) {
            CatalogueHealth::OrphanedSource
        } else if observation.is_present() {
            let fresh = archive.size_bytes == observation.size
                && archive.modified_time_unix_seconds == observation.modified;
            let verified = fresh
                && archive.identity_report.as_ref().is_some_and(|r| {
                    matches!(
                        canonical_identity_from_game_report(r).0,
                        CanonicalIdentityStatus::Resolved(_)
                    )
                });
            if verified {
                CatalogueHealth::PresentVerified
            } else {
                CatalogueHealth::PresentNotVerified
            }
        } else if observation.probe != FsProbe::Missing
            || !source.is_some_and(|(_, available)| *available)
        {
            CatalogueHealth::NotChecked
        } else if !move_candidates.is_empty() {
            CatalogueHealth::PossiblyMoved
        } else {
            CatalogueHealth::Missing
        };
        if observation.is_present() {
            if archive.last_verified_missing_at.is_some() {
                counts.present_stale_flag += 1;
            } else {
                counts.present_clean += 1;
            }
        }
        match health {
            CatalogueHealth::PresentVerified => counts.present_verified += 1,
            CatalogueHealth::PresentNotVerified => counts.present_not_verified += 1,
            CatalogueHealth::PossiblyMoved => counts.possibly_moved += 1,
            CatalogueHealth::Missing => counts.missing += 1,
            CatalogueHealth::OrphanedSource => counts.orphaned_source += 1,
            CatalogueHealth::NotChecked => counts.not_checked += 1,
        }
        counts.rows_with_move_candidates += usize::from(!move_candidates.is_empty());
        let unique_paths: std::collections::HashSet<_> =
            move_candidates.iter().map(|c| &c.path).collect();
        counts.ambiguous_move_candidates += usize::from(unique_paths.len() > 1);
        counts.strong_move_candidates += usize::from(
            move_candidates
                .iter()
                .any(|c| c.evidence == MoveEvidence::VerifiedSha256),
        );
        rows.push(CatalogueHealthRow {
            archive: archive.clone(),
            observation: observation.clone(),
            health,
            move_candidates,
        });
    }
    counts.total = rows.len();
    counts.rows_would_change = counts.present_stale_flag;
    counts.rows_left_untouched = counts.total - counts.rows_would_change;
    Ok(CatalogueHealthReport {
        counts,
        rows,
        database_path: database.path().to_path_buf(),
    })
}
