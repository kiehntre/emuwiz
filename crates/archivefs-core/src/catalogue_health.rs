//! Catalogue presence is separate from mount health and game identity.
//! Reports are read-only; move candidates never authorize relinking.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
mod path_probe;
pub(crate) use path_probe::{BoundRoot, NestedState, directory_mount};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::database::{Database, PersistedArchive};
use crate::emulator_environment::FsProbe;
use crate::launch::{CanonicalIdentityStatus, canonical_identity_from_game_report};

/// Accepted root/volume fingerprint, not a game-content identity. Different
/// filesystem IDs cannot inherit a source's destructive scan authority even
/// if a reused device number and inode happen to collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRootBinding {
    pub device: u64,
    pub inode: u64,
    pub filesystem_type: i64,
    pub filesystem_id: Vec<u8>,
}
impl SourceRootBinding {
    pub fn inspect(path: &Path) -> Option<Self> {
        BoundRoot::open(path).map(|root| root.binding)
    }
}

impl SourceRootBinding {
    /// A plain-language name for the filesystem type, for review screens only.
    /// Unknown types are shown as their hexadecimal magic number.
    pub fn filesystem_name(&self) -> String {
        match self.filesystem_type as u64 & 0xffff_ffff {
            0xef53 => "ext2/3/4".into(),
            0x9123_683e => "btrfs".into(),
            0x5846_5342 => "XFS".into(),
            0x2011_bab0 => "exFAT".into(),
            0x4d44 => "FAT/VFAT".into(),
            0x5346_544e | 0x7366_746e => "NTFS".into(),
            0x6969 => "NFS".into(),
            0xff53_4d42 | 0xfe53_4d42 => "SMB/CIFS".into(),
            0x6573_5546 => "FUSE".into(),
            0x0102_1994 => "tmpfs".into(),
            0x482b => "HFS+".into(),
            0x3153_464a => "JFS".into(),
            other => format!("type 0x{other:x}"),
        }
    }

    /// Short filesystem identifier for comparing two reviews by eye.
    pub fn filesystem_id_hex(&self) -> String {
        self.filesystem_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Why a source's catalogue authority is blocked until a person reviews it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RebindReason {
    /// The source was scanned before storage continuity was tracked, so it has
    /// never been bound. Upgrading never binds it automatically.
    NeverBound,
    /// The folder exists but is on different storage than the one reviewed.
    BackingChanged,
}

/// Source-level catalogue state, from the backend only. A source can establish
/// new Missing evidence only when it is [`SourceHealthState::Healthy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceHealthState {
    /// Bound, reachable, and its latest scan for this generation was complete.
    Healthy,
    /// Bound (or brand new) but no scan has covered the current generation.
    NeedsScan,
    PartialScan,
    /// Scanned, but coverage cannot be trusted (failed scan, unproven nested
    /// mount, or an unfinished one).
    CoverageIncomplete,
    SourceUnavailable,
    RebindRequired,
    /// The source's role keeps it out of game scanning.
    NotGameScanned,
}

impl SourceHealthState {
    pub fn can_establish_missing(self) -> bool {
        self == Self::Healthy
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceHealth {
    pub source_id: i64,
    pub path: PathBuf,
    pub state: SourceHealthState,
    /// Set exactly when `state` is `RebindRequired`.
    pub rebind: Option<RebindReason>,
    /// Accepted binding generation; 0 means the source has never been bound.
    pub generation: i64,
    pub detail: Option<String>,
}

/// Everything a person needs to decide whether a folder is still the storage
/// they reviewed before. Produced read-only. Confirming it re-checks every
/// field; any change means the review is out of date and must be repeated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRebindReview {
    pub source_id: i64,
    pub path: PathBuf,
    /// The accepted generation this review was made against (0 = never bound).
    pub generation: i64,
    /// What the source was bound to, if it was ever bound.
    pub recorded: Option<SourceRootBinding>,
    /// What the folder is on right now.
    pub current: SourceRootBinding,
    pub reason: RebindReason,
    pub archive_count: i64,
    pub last_successful_scan_at: Option<String>,
}

/// Prefix of every refusal caused by a source that changed after review.
pub const REBIND_REVIEW_AGAIN: &str = "the source changed since it was reviewed; review it again";

/// A nested mount root exactly as the scan walker saw it. Recording it as the
/// accepted boundary requires the same mount to still be there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedBoundaryObservation {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub mount_id: u64,
}

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
    pub(crate) binding: Vec<(u64, u64)>,
    pub(crate) modified_ns: Option<i64>,
}

pub(crate) fn source_root_identity(path: &Path) -> Option<(u64, u64)> {
    let observed = observe_path(path, true);
    observed
        .is_present()
        .then(|| observed.binding.last().copied())
        .flatten()
}

pub(crate) fn observe_path(path: &Path, directory: bool) -> PathObservation {
    path_probe::probe(path, directory)
}

pub(crate) fn observe_owned(
    root: &Path,
    identity: Option<(u64, u64)>,
    path: &Path,
    directory: bool,
) -> PathObservation {
    let Some(bound) = path_probe::BoundRoot::open(root) else {
        return path_probe::unsafe_observation();
    };
    if Some(bound.identity) != identity {
        return path_probe::unsafe_observation();
    }
    bound.probe(path, directory)
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
    /// Bounded review detail; does not discard the existence/ambiguity of a group.
    pub move_candidates_truncated: bool,
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
    /// Source-level continuity findings that qualify the row classifications.
    pub diagnostics: Vec<String>,
    pub(crate) database_path: PathBuf,
    pub(crate) epoch: Option<i64>,
    pub(crate) sources: Vec<SourceScanCoverage>,
    pub(crate) source_bindings: HashMap<i64, SourceRootBinding>,
    pub(crate) configured_roots: Vec<PathBuf>,
}

fn name_key(name: &std::ffi::OsStr) -> Vec<u8> {
    name.as_encoded_bytes()
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect()
}

fn sha256(root: &path_probe::BoundRoot, path: &Path, expected: &PathObservation) -> Option<String> {
    let mut file = root.read_file(path, expected)?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    let mut total = 0u64;
    let expected_size = expected.size?;
    loop {
        let n = file.read(&mut bytes).ok()?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64)?;
        if total > expected_size {
            return None;
        }
        hash.update(&bytes[..n]);
    }
    (total == expected_size && root.probe(path, false) == *expected)
        .then(|| hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub(crate) fn validate_source_ownership(roots: &[PathBuf]) -> Result<()> {
    crate::validate_configured_source_roots(roots)?;
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        if let Some(binding) = SourceRootBinding::inspect(root)
            && !seen.insert((
                binding.device,
                binding.inode,
                binding.filesystem_type,
                binding.filesystem_id,
            ))
        {
            return Err(crate::ArchiveFsError::Config(
                "duplicate source namespace through filesystem alias".into(),
            ));
        }
    }
    Ok(())
}

/// Index only observed catalogue paths, once. No per-row directory walks and
/// no hashing unless a historical SHA-256 can actually confirm a candidate.
/// `Missing` means the recorded path is absent with no indexed candidate,
/// not that every possible storage device has been searched.
pub fn preview_catalogue_health(
    database: &Database,
    configured_roots: &[PathBuf],
) -> Result<CatalogueHealthReport> {
    validate_source_ownership(configured_roots)?;
    let epoch = database.catalogue_health_epoch()?;
    let archives = database.load_archives()?;
    let mut sources = database.initial_scan_coverage(&[])?;
    for source in &mut sources {
        source.root_identity = source_root_identity(&source.root);
        if let Some((binding, _)) = database.catalogue_source_binding(source.source_id)?
            && SourceRootBinding::inspect(&source.root)
                .and_then(|r| serde_json::to_string(&r).ok())
                .as_deref()
                != Some(binding.as_str())
        {
            source.root_identity = None;
        }
        source.excluded_roots = configured_roots
            .iter()
            .filter(|r| *r != &source.root && r.starts_with(&source.root))
            .cloned()
            .collect();
    }
    let hashes = database.catalogue_archive_hashes()?;
    let bound_roots: HashMap<_, _> = sources
        .iter()
        .filter_map(|s| {
            let root = path_probe::BoundRoot::open(&s.root)?;
            (Some(root.identity) == s.root_identity).then_some((s.source_id, root))
        })
        .collect();
    // Remembered nested mounts that are gone or replaced make everything
    // beneath them unprovable: report that as NotChecked, exactly as the scan
    // refuses to write Missing evidence there, and say why.
    let mut diagnostics = Vec::new();
    let mut unproven_prefixes = HashMap::<i64, Vec<PathBuf>>::new();
    for source in &mut sources {
        if let Some(root) = bound_roots.get(&source.source_id) {
            let unproven = database.preview_unproven_nested_boundaries(source.source_id, root)?;
            if unproven.is_empty() {
                continue;
            }
            let detail = unproven
                .iter()
                .map(|(path, reason)| {
                    format!("nested filesystem boundary {} ({reason})", path.display())
                })
                .collect::<Vec<_>>()
                .join("; ");
            diagnostics.push(format!(
                "{detail} is not proven continuous; entries beneath it are not checked"
            ));
            source.state = ScanCoverageState::Partial;
            source.diagnostic = Some(detail);
            unproven_prefixes.insert(
                source.source_id,
                unproven.into_iter().map(|(path, _)| path).collect(),
            );
        }
    }
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
    let mut observations: Vec<_> = archives
        .iter()
        .map(|a| {
            let source = sources.iter().find(|s| s.source_id == a.source_folder_id);
            if source.is_some_and(|s| {
                s.excluded_roots
                    .iter()
                    .any(|r| a.absolute_path.starts_with(r))
            }) {
                return path_probe::unsafe_observation();
            }
            if unproven_prefixes
                .get(&a.source_folder_id)
                .is_some_and(|p| p.iter().any(|r| a.absolute_path.starts_with(r)))
            {
                return path_probe::unsafe_observation();
            }
            bound_roots
                .get(&a.source_folder_id)
                .map(|root| root.probe(&a.absolute_path, a.archive_kind == "arcade_set_directory"))
                .unwrap_or_else(path_probe::unsafe_observation)
        })
        .collect();
    let invalid_sources: std::collections::HashSet<_> = bound_roots
        .iter()
        .filter(|(_, root)| !root.current())
        .map(|(id, _)| *id)
        .collect();
    for (archive, observation) in archives.iter().zip(&mut observations) {
        if invalid_sources.contains(&archive.source_folder_id) {
            *observation = path_probe::unsafe_observation();
        }
    }
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
    let group_ambiguous: HashMap<_, _> = by_name
        .iter()
        .map(|(name, indices)| {
            let first = indices.first().map(|i| &archives[*i].absolute_path);
            (
                name.clone(),
                indices
                    .iter()
                    .any(|i| Some(&archives[*i].absolute_path) != first),
            )
        })
        .collect();
    const MAX_MOVE_DETAILS: usize = 256;
    const HASH_BYTE_BUDGET: u64 = 256 * 1024 * 1024;
    let mut hash_bytes_remaining = HASH_BYTE_BUDGET;
    let mut hash_cache = HashMap::new();
    let mut counts = CatalogueHealthCounts::default();
    let mut rows = Vec::with_capacity(archives.len());
    for (i, archive) in archives.iter().enumerate() {
        let observation = &observations[i];
        let mut candidates = BTreeMap::new();
        let mut truncated = false;
        if observation.probe == FsProbe::Missing {
            if let Some(indices) = archive
                .absolute_path
                .file_name()
                .and_then(|n| by_name.get(&name_key(n)))
            {
                truncated |= indices.len() > MAX_MOVE_DETAILS;
                for &j in indices.iter().take(MAX_MOVE_DETAILS) {
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
                if let Some(group) = by_hash.get(expected) {
                    truncated |= group.len() > MAX_MOVE_DETAILS;
                    indices.extend(group.iter().take(MAX_MOVE_DETAILS).copied());
                }
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
                        .or_insert_with(|| {
                            let size = observations[j].size?;
                            if size > hash_bytes_remaining {
                                return None;
                            }
                            hash_bytes_remaining -= size;
                            sha256(
                                bound_roots.get(&archives[j].source_folder_id)?,
                                &archives[j].absolute_path,
                                &observations[j],
                            )
                        });
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
        counts.ambiguous_move_candidates += usize::from(
            unique_paths.len() > 1
                || (observation.probe == FsProbe::Missing
                    && archive
                        .absolute_path
                        .file_name()
                        .and_then(|n| group_ambiguous.get(&name_key(n)))
                        .copied()
                        .unwrap_or(false)),
        );
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
            move_candidates_truncated: truncated,
        });
    }
    counts.total = rows.len();
    counts.rows_would_change = counts.present_stale_flag;
    counts.rows_left_untouched = counts.total - counts.rows_would_change;
    Ok(CatalogueHealthReport {
        counts,
        rows,
        diagnostics,
        database_path: database.path().to_path_buf(),
        epoch,
        source_bindings: bound_roots
            .into_iter()
            .map(|(id, root)| (id, root.binding))
            .collect(),
        sources,
        configured_roots: configured_roots.to_vec(),
    })
}
