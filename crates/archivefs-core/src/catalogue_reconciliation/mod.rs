//! Read-only reconciliation of three populations that the catalogue alone
//! cannot relate: files that exist inside a configured library root, the
//! catalogue rows that describe files, and rows whose recorded file is gone.
//!
//! Nothing here writes. It never imports a file, relinks a path, deletes a row
//! or assigns a platform: it only *explains* each population, and where one row
//! and one file can be paired it returns a proposal that a later, explicit
//! operation may act on.
//!
//! # Evidence
//! A move is only proposed on a **strong** match: a cryptographic hash (SHA-1 or
//! SHA-256) the catalogue already persisted for the row equals the same
//! algorithm's hash of exactly one uncatalogued physical file, with equal size
//! and no contradicting hash. A path, a folder, a file name or a size is never
//! enough - those produce [`RowState::PossiblyMoved`] at most, which names its
//! candidates but proposes nothing. A folder name never creates a platform: a
//! physical file only ever carries the root it was found under.
//!
//! Physical hashes are an explicit, bounded, cancellable operation
//! ([`hashing`]); reconciliation itself never reads file contents.
pub mod db;
pub mod hashing;
pub mod walk;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// The most candidates any one row lists; the true count is always reported.
pub const MAX_LISTED_CANDIDATES: usize = 16;
/// The most files examined per row when looking for weak candidates, so a
/// pathological size cluster cannot make matching quadratic.
const MAX_SCANNED_PER_ROW: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StrongHashAlgorithm {
    Sha1,
    Sha256,
}

impl StrongHashAlgorithm {
    fn hex_len(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }
}

/// A cryptographic content hash. CRC32 and MD5 are deliberately not
/// representable: they are not evidence strong enough to pair files.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StrongHash {
    pub algorithm: StrongHashAlgorithm,
    pub hex: String,
}

impl StrongHash {
    /// `None` unless `hex` is a well-formed digest of `algorithm`.
    pub fn new(algorithm: StrongHashAlgorithm, hex: &str) -> Option<Self> {
        let hex = hex.trim();
        (hex.len() == algorithm.hex_len() && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(
            || Self {
                algorithm,
                hex: hex.to_ascii_lowercase(),
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnprovableReason {
    /// The source folder is not reachable or no longer a configured source.
    StorageUnavailable,
    /// The source's storage is not the storage that was reviewed.
    SourceNeedsReview,
    /// The path lies beneath a filesystem boundary that is not proven.
    NestedBoundaryUnproven,
    /// The path could not be examined (permissions or I/O error).
    Inaccessible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowPresence {
    Present,
    /// Proven absent: its source is healthy and the path does not exist.
    Missing,
    /// Absence cannot be established - never to be reported as deleted.
    Unprovable(UnprovableReason),
}

/// What the catalogue holds about one row, reduced to reconciliation facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowFacts {
    pub archive_id: i64,
    pub source_id: i64,
    pub path: PathBuf,
    pub size: Option<u64>,
    /// Persisted platform, carried through untouched for display.
    pub platform: Option<String>,
    pub presence: RowPresence,
    /// Strong hashes the catalogue already persisted for this row.
    pub hashes: Vec<StrongHash>,
}

/// One regular file found under a configured root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFacts {
    pub path: PathBuf,
    /// The configured root it was discovered under.
    pub source_id: i64,
    pub size: u64,
    /// Hashes from an explicit hashing operation, if one was run. Never
    /// computed by reconciliation itself.
    pub hashes: Vec<StrongHash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakBasis {
    /// Same file name (case-insensitive) and same size.
    NameAndSize,
    /// Same size only, and the row has a persisted hash that hashing could test.
    SizeOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveProof {
    /// The row has a persisted strong hash: hashing the listed candidates
    /// (an explicit, later operation) can settle it.
    NeedsHashing,
    /// The row has no persisted strong hash, so no hashing of the candidates
    /// can prove anything. Do not call this a move.
    NoPersistedHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbiguityReason {
    /// One old row matches several physical files exactly.
    ManyFilesOneRow,
    /// Several old rows match the same physical file exactly.
    ManyRowsOneFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// The recorded file exists.
    CataloguedPresent,
    /// The recorded file is gone and nothing relates to it.
    CatalogueFileMissing,
    /// Exactly one uncatalogued file strongly matches exactly this row. A
    /// proposal only: nothing is relinked.
    StrongMoveCandidate { to: PathBuf },
    AmbiguousMoveCandidates {
        reason: AmbiguityReason,
        candidates: Vec<PathBuf>,
        candidates_total: usize,
    },
    /// Weak evidence only. Not a move.
    PossiblyMoved {
        basis: WeakBasis,
        proof: MoveProof,
        candidates: Vec<PathBuf>,
        candidates_total: usize,
    },
    /// The persisted evidence disagrees with itself or with a physical file.
    Conflict { detail: &'static str },
    /// Not enough is known to say anything, e.g. the storage is unavailable.
    Unknown { reason: UnprovableReason },
}

/// How a companion file is known to belong to its catalogued parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionBasis {
    /// Named by a `FILE` line of the parent's CUE sheet, resolved by the
    /// ingestion CUE resolver under its path-safety rules.
    CueFileReference,
    /// Named by a track line of the parent's GDI descriptor, resolved by the
    /// ingestion GDI resolver.
    GdiTrackReference,
}

/// Files a catalogued descriptor (CUE/GDI) intentionally references, as
/// resolved by the existing ingestion resolvers. Built once per parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionLink {
    pub parent_archive_id: i64,
    pub basis: CompanionBasis,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// Exists with no row of its own, but a *present*, catalogued parent
    /// descriptor deliberately references it. Not an uncatalogued candidate and
    /// never a move target independently of its parent.
    ReferencedCompanion {
        parent_archive_id: i64,
        basis: CompanionBasis,
    },
    /// Referenced by more than one catalogued parent: ownership is ambiguous,
    /// so it is neither independent nor attributed to either.
    ContendedCompanion { parent_archive_ids: Vec<i64> },
    /// A catalogue row records this exact path.
    Catalogued { archive_id: i64 },
    /// Exists, but EmuWiz has never catalogued it. No identity or platform is
    /// implied; it is not eligible for any operation needing catalogue identity.
    Uncatalogued,
    /// The single strong target of exactly one missing row.
    StrongMoveTarget { archive_id: i64 },
    /// Strongly matched by several rows (or sharing a row with other files).
    ContendedMoveTarget { archive_ids: Vec<i64> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowOutcome {
    pub archive_id: i64,
    pub path: PathBuf,
    /// The row's own persisted platform, unchanged.
    pub platform: Option<String>,
    pub state: RowState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOutcome {
    pub path: PathBuf,
    pub source_id: i64,
    pub size: u64,
    pub state: FileState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconciliationCounts {
    pub rows: usize,
    pub files: usize,
    pub catalogued_present: usize,
    /// Files with no catalogue row, including ones that are a move target.
    pub uncatalogued_files: usize,
    /// Files with no row that a catalogued parent descriptor references.
    pub referenced_companions: usize,
    pub contended_companions: usize,
    pub missing: usize,
    pub strong_move_candidates: usize,
    pub ambiguous: usize,
    pub possibly_moved: usize,
    pub conflicts: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconciliationReport {
    /// Sorted by archive id.
    pub rows: Vec<RowOutcome>,
    /// Sorted by path.
    pub files: Vec<FileOutcome>,
    pub counts: ReconciliationCounts,
    /// Files whose hash would settle a [`MoveProof::NeedsHashing`] row. A later
    /// explicit operation may hash these; reconciliation never does.
    pub hash_requests: Vec<PathBuf>,
}

fn name_key(path: &Path) -> Vec<u8> {
    path.file_name()
        .map(|n| {
            n.as_encoded_bytes()
                .iter()
                .map(u8::to_ascii_lowercase)
                .collect()
        })
        .unwrap_or_default()
}

#[derive(PartialEq, Eq)]
enum Compat {
    /// At least one shared algorithm agrees and none disagree.
    StrongMatch,
    /// No shared algorithm: nothing can be said by hash.
    NoOverlap,
    /// A shared algorithm disagrees, or sizes differ.
    Differs,
}

fn compat(row: &RowFacts, file: &FileFacts) -> Compat {
    if row.size.is_some_and(|size| size != file.size) {
        return Compat::Differs;
    }
    let mut agreed = false;
    for hash in &row.hashes {
        if let Some(other) = file.hashes.iter().find(|h| h.algorithm == hash.algorithm) {
            if other != hash {
                return Compat::Differs;
            }
            agreed = true;
        }
    }
    if agreed {
        Compat::StrongMatch
    } else {
        Compat::NoOverlap
    }
}

fn hashes_disagree(hashes: &[StrongHash]) -> bool {
    let mut seen: HashMap<StrongHashAlgorithm, &str> = HashMap::new();
    hashes.iter().any(|hash| {
        seen.insert(hash.algorithm, &hash.hex)
            .is_some_and(|previous| previous != hash.hex)
    })
}

/// Pairs rows and files. Pure and deterministic: the result is independent of
/// the order of `rows` and `files`. Indexes make it linear apart from a bounded
/// per-row candidate scan; there is no all-pairs comparison.
pub fn reconcile(rows: &[RowFacts], files: &[FileFacts]) -> ReconciliationReport {
    reconcile_with_companions(rows, files, &[])
}

/// As [`reconcile`], additionally excluding proven companions of catalogued,
/// present parents from the uncatalogued population. A link whose parent is
/// missing, unknown or not a row at all claims nothing.
pub fn reconcile_with_companions(
    rows: &[RowFacts],
    files: &[FileFacts],
    links: &[CompanionLink],
) -> ReconciliationReport {
    let mut rows: Vec<&RowFacts> = rows.iter().collect();
    rows.sort_by_key(|row| row.archive_id);
    let mut files: Vec<&FileFacts> = files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);

    let mut row_by_path: HashMap<&Path, i64> = HashMap::new();
    for row in &rows {
        row_by_path
            .entry(row.path.as_path())
            .or_insert(row.archive_id);
    }

    // One pass over the links builds the companion index; each descriptor was
    // resolved once by the caller.
    let present_parents: std::collections::HashSet<i64> = rows
        .iter()
        .filter(|row| row.presence == RowPresence::Present)
        .map(|row| row.archive_id)
        .collect();
    let mut companions: HashMap<&Path, Vec<(i64, CompanionBasis)>> = HashMap::new();
    for link in links
        .iter()
        .filter(|l| present_parents.contains(&l.parent_archive_id))
    {
        for path in &link.files {
            let owners = companions.entry(path.as_path()).or_default();
            if !owners.iter().any(|(id, _)| *id == link.parent_archive_id) {
                owners.push((link.parent_archive_id, link.basis));
            }
        }
    }

    let mut file_state: Vec<FileState> = Vec::with_capacity(files.len());
    let mut uncatalogued: Vec<usize> = Vec::new();
    for (index, file) in files.iter().enumerate() {
        if let Some(&archive_id) = row_by_path.get(file.path.as_path()) {
            file_state.push(FileState::Catalogued { archive_id });
            continue;
        }
        match companions.get(file.path.as_path()).map(Vec::as_slice) {
            Some([(parent_archive_id, basis)]) => file_state.push(FileState::ReferencedCompanion {
                parent_archive_id: *parent_archive_id,
                basis: *basis,
            }),
            Some(owners) if owners.len() > 1 => {
                let mut ids: Vec<i64> = owners.iter().map(|(id, _)| *id).collect();
                ids.sort_unstable();
                file_state.push(FileState::ContendedCompanion {
                    parent_archive_ids: ids,
                });
            }
            _ => {
                file_state.push(FileState::Uncatalogued);
                uncatalogued.push(index);
            }
        }
    }

    // Indexes over uncatalogued files only (already in path order).
    let mut by_hash: HashMap<&StrongHash, Vec<usize>> = HashMap::new();
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut by_name_size: HashMap<(Vec<u8>, u64), Vec<usize>> = HashMap::new();
    for &index in &uncatalogued {
        let file = files[index];
        for hash in &file.hashes {
            by_hash.entry(hash).or_default().push(index);
        }
        by_size.entry(file.size).or_default().push(index);
        by_name_size
            .entry((name_key(&file.path), file.size))
            .or_default()
            .push(index);
    }

    enum Pending {
        Done(RowState),
        Strong(Vec<usize>),
    }
    let mut pending: Vec<Pending> = Vec::with_capacity(rows.len());
    let mut claims: HashMap<usize, Vec<i64>> = HashMap::new();
    let mut hash_requests: BTreeSet<&Path> = BTreeSet::new();

    for row in &rows {
        let state = match row.presence {
            RowPresence::Present => Pending::Done(RowState::CataloguedPresent),
            RowPresence::Unprovable(reason) => Pending::Done(RowState::Unknown { reason }),
            RowPresence::Missing if hashes_disagree(&row.hashes) => {
                Pending::Done(RowState::Conflict {
                    detail: "the catalogue holds different hashes of one algorithm for this row",
                })
            }
            RowPresence::Missing => {
                let mut strong: BTreeSet<usize> = BTreeSet::new();
                let mut contradiction = false;
                for hash in &row.hashes {
                    for &index in by_hash.get(hash).into_iter().flatten() {
                        match compat(row, files[index]) {
                            Compat::StrongMatch => {
                                strong.insert(index);
                            }
                            // Equal strong hash yet a different size or another
                            // hash disagreeing: the evidence contradicts itself.
                            Compat::Differs => contradiction = true,
                            Compat::NoOverlap => {}
                        }
                    }
                }
                if contradiction && strong.is_empty() {
                    Pending::Done(RowState::Conflict {
                        detail: "a file shares a strong hash with this row but contradicts its size or another hash",
                    })
                } else if !strong.is_empty() {
                    for &index in &strong {
                        claims.entry(index).or_default().push(row.archive_id);
                    }
                    Pending::Strong(strong.into_iter().collect())
                } else {
                    Pending::Done(weak_state(
                        row,
                        &files,
                        &by_size,
                        &by_name_size,
                        &mut hash_requests,
                    ))
                }
            }
        };
        pending.push(state);
    }

    let listed = |indices: &[usize]| -> Vec<PathBuf> {
        indices
            .iter()
            .take(MAX_LISTED_CANDIDATES)
            .map(|&i| files[i].path.clone())
            .collect()
    };
    let mut outcomes = Vec::with_capacity(rows.len());
    for (row, pending) in rows.iter().zip(pending) {
        let state = match pending {
            Pending::Done(state) => state,
            Pending::Strong(indices) => {
                let sole_file = indices.len() == 1;
                let sole_claim = indices
                    .iter()
                    .all(|index| claims.get(index).is_some_and(|c| c.len() == 1));
                if sole_file && sole_claim {
                    file_state[indices[0]] = FileState::StrongMoveTarget {
                        archive_id: row.archive_id,
                    };
                    RowState::StrongMoveCandidate {
                        to: files[indices[0]].path.clone(),
                    }
                } else {
                    RowState::AmbiguousMoveCandidates {
                        reason: if sole_file {
                            AmbiguityReason::ManyRowsOneFile
                        } else {
                            AmbiguityReason::ManyFilesOneRow
                        },
                        candidates_total: indices.len(),
                        candidates: listed(&indices),
                    }
                }
            }
        };
        outcomes.push(RowOutcome {
            archive_id: row.archive_id,
            path: row.path.clone(),
            platform: row.platform.clone(),
            state,
        });
    }
    // Any file claimed by more than one row, or one of several for a row, is
    // contended rather than a target.
    for (index, ids) in &claims {
        let is_target = matches!(file_state[*index], FileState::StrongMoveTarget { .. });
        if !is_target {
            let mut archive_ids = ids.clone();
            archive_ids.sort_unstable();
            archive_ids.dedup();
            file_state[*index] = FileState::ContendedMoveTarget { archive_ids };
        }
    }

    let mut counts = ReconciliationCounts {
        rows: outcomes.len(),
        files: files.len(),
        ..Default::default()
    };
    for outcome in &outcomes {
        match outcome.state {
            RowState::CataloguedPresent => counts.catalogued_present += 1,
            RowState::CatalogueFileMissing => counts.missing += 1,
            RowState::StrongMoveCandidate { .. } => counts.strong_move_candidates += 1,
            RowState::AmbiguousMoveCandidates { .. } => counts.ambiguous += 1,
            RowState::PossiblyMoved { .. } => counts.possibly_moved += 1,
            RowState::Conflict { .. } => counts.conflicts += 1,
            RowState::Unknown { .. } => counts.unknown += 1,
        }
    }
    for state in &file_state {
        match state {
            FileState::Uncatalogued
            | FileState::StrongMoveTarget { .. }
            | FileState::ContendedMoveTarget { .. } => counts.uncatalogued_files += 1,
            FileState::ReferencedCompanion { .. } => counts.referenced_companions += 1,
            FileState::ContendedCompanion { .. } => counts.contended_companions += 1,
            FileState::Catalogued { .. } => {}
        }
    }
    ReconciliationReport {
        rows: outcomes,
        files: files
            .iter()
            .zip(file_state)
            .map(|(file, state)| FileOutcome {
                path: file.path.clone(),
                source_id: file.source_id,
                size: file.size,
                state,
            })
            .collect(),
        counts,
        hash_requests: hash_requests.into_iter().map(Path::to_path_buf).collect(),
    }
}

/// A missing row with no strong match: weak candidates (never a proposal) or
/// plainly missing.
fn weak_state<'a>(
    row: &RowFacts,
    files: &[&'a FileFacts],
    by_size: &HashMap<u64, Vec<usize>>,
    by_name_size: &HashMap<(Vec<u8>, u64), Vec<usize>>,
    hash_requests: &mut BTreeSet<&'a Path>,
) -> RowState {
    // Without a recorded size there is nothing to narrow by, and a file name
    // alone is never evidence.
    let Some(size) = row.size else {
        return RowState::CatalogueFileMissing;
    };
    let has_hash = !row.hashes.is_empty();
    let usable = |index: &usize| compat(row, files[*index]) != Compat::Differs;
    let named: Vec<usize> = by_name_size
        .get(&(name_key(&row.path), size))
        .map(|all| {
            all.iter()
                .copied()
                .take(MAX_SCANNED_PER_ROW)
                .filter(usable)
                .collect()
        })
        .unwrap_or_default();
    let (basis, pool): (WeakBasis, Vec<usize>) = if !named.is_empty() {
        (WeakBasis::NameAndSize, named)
    } else if has_hash {
        let sized: Vec<usize> = by_size
            .get(&size)
            .map(|all| {
                all.iter()
                    .copied()
                    .take(MAX_SCANNED_PER_ROW)
                    .filter(usable)
                    .collect()
            })
            .unwrap_or_default();
        (WeakBasis::SizeOnly, sized)
    } else {
        return RowState::CatalogueFileMissing;
    };
    if pool.is_empty() {
        return RowState::CatalogueFileMissing;
    }
    let total = by_name_size
        .get(&(name_key(&row.path), size))
        .map_or(0, Vec::len)
        .max(pool.len());
    let candidates: Vec<PathBuf> = pool
        .iter()
        .take(MAX_LISTED_CANDIDATES)
        .map(|&i| files[i].path.clone())
        .collect();
    let proof = if has_hash {
        for &index in pool.iter().take(MAX_LISTED_CANDIDATES) {
            hash_requests.insert(files[index].path.as_path());
        }
        MoveProof::NeedsHashing
    } else {
        MoveProof::NoPersistedHash
    };
    RowState::PossiblyMoved {
        basis,
        proof,
        candidates,
        candidates_total: total,
    }
}

/// The files a catalogued CUE/GDI descriptor intentionally references, through
/// the existing ingestion resolvers (same bounded read and path-safety rules:
/// no absolute or `..` references, and the canonical result must stay beneath
/// the descriptor's own directory). `None` when `descriptor` is not a CUE or
/// GDI by extension. References that are missing or unsafe resolve to nothing.
/// Paths are returned in the spelling of the descriptor's own directory, not
/// canonicalised. Only the descriptor (at most 256 KiB) is read.
pub fn descriptor_companions(descriptor: &Path) -> Option<(CompanionBasis, Vec<PathBuf>)> {
    use crate::ingestion::{
        cue_bin::resolve_cue_all_files_lenient, gdi::resolve_gdi_all_tracks_lenient,
    };
    let extension = descriptor.extension()?.to_ascii_lowercase();
    let (basis, resolved): (CompanionBasis, Vec<PathBuf>) = if extension == "cue" {
        (
            CompanionBasis::CueFileReference,
            resolve_cue_all_files_lenient(descriptor)
                .map(|all| all.into_iter().flatten().collect())
                .unwrap_or_default(),
        )
    } else if extension == "gdi" {
        (
            CompanionBasis::GdiTrackReference,
            resolve_gdi_all_tracks_lenient(descriptor)
                .map(|all| all.into_iter().flatten().collect())
                .unwrap_or_default(),
        )
    } else {
        return None;
    };
    let Some(parent) = descriptor.parent() else {
        return Some((basis, Vec::new()));
    };
    let Ok(canonical_parent) = std::fs::canonicalize(parent) else {
        return Some((basis, Vec::new()));
    };
    let files = resolved
        .iter()
        .filter_map(|canonical| canonical.strip_prefix(&canonical_parent).ok())
        .map(|relative| parent.join(relative))
        .collect();
    Some((basis, files))
}

/// Convenience for adapters: fold explicit hash results into file facts.
pub fn attach_hashes(files: &mut [FileFacts], hashes: &BTreeMap<PathBuf, Vec<StrongHash>>) {
    for file in files {
        if let Some(found) = hashes.get(&file.path) {
            file.hashes = found.clone();
        }
    }
}

#[cfg(test)]
mod tests;
