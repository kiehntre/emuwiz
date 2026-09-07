//! Bounded, read-only WHDLoad `.slave` discovery inside LHA/LZH archives.
//!
//! This adds no WHDLoad parser and no LHA parser: it reuses
//! [`crate::dat::archive::lha`] for bounded archive-member listing/reading
//! (through the existing optional 7-Zip backend, fd-pinned, never extracted
//! to disk) and [`crate::identity_source::whdload::parse_whdload_slave`] for
//! the actual `.slave` HUNK structure. This module only glues the two
//! together: it finds `.slave`/`.islave`-named members, bounded-reads each
//! one, and hands the bytes to the existing parser - the same shape as
//! [`crate::amiga_disk::filesystem::discover_whdload_slaves`] uses for a
//! `.slave` embedded in an ADF/HDF filesystem, just for an LHA/LZH member
//! instead of a filesystem file.
//!
//! An archive filename is never treated as a game identity, and finding
//! *some* file named `.slave` is never enough on its own - only bytes that
//! pass [`parse_whdload_slave`] become a candidate. Multiple valid
//! candidates are never resolved to a single "winner": every one is
//! returned, and the caller (see `ingestion::discovery`) reports the
//! archive as ambiguous rather than guessing.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::dat::archive::lha::{LhaArchiveSource, LhaError, LhaProvider};
use crate::dat::archive::limits::ArchiveLimits;
use crate::dat::archive::ArchiveMemberSource;
use crate::identity_source::whdload::{
    parse_whdload_slave, ParsedWHDLoadSlave, SlaveArtifact, SlaveHashes,
};
use crate::safe_read::TrustedRoots;

/// How long the optional 7-Zip backend is given to list and extract members
/// from one archive. Mirrors `ingestion::container`'s own
/// `MEMBER_LIST_TIMEOUT` for archive-member listing; extraction of a single
/// small `.slave` member needs no more.
pub const WHDLOAD_ARCHIVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Largest number of `.slave`/`.islave`-named members inspected from one
/// archive. Real WHDLoad packages carry at most a handful of slave
/// revisions/variants; this is a safety bound against a pathological
/// archive with thousands of same-suffix entries, not a realistic ceiling.
pub const MAX_SLAVE_CANDIDATES: usize = 32;

/// Largest bytes accepted for one candidate `.slave` member before it is
/// even bounded-read. Mirrors the standalone `.slave` file parser's own
/// ceiling (`identity_source::whdload::slave`'s `MAX_SLAVE_FILE_BYTES`) so
/// an archive member costs no more to inspect than a loose file would.
pub const MAX_INDIVIDUAL_SLAVE_BYTES: u64 = 16 * 1024 * 1024;

/// Largest aggregate bytes bounded-read across every candidate slave in one
/// archive, independent of each candidate's own size - guards against an
/// archive with many individually acceptable slave-sized members.
pub const MAX_TOTAL_SLAVE_BYTES: u64 = 64 * 1024 * 1024;

/// One valid, structurally-parsed WHDLoad slave found inside an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSlaveCandidate {
    /// The member's path exactly as stored in the archive - display and
    /// provenance only, never used to imply a game identity.
    pub member_path: String,
    /// The parsed slave and hashes of exactly the archive-member bytes.
    pub artifact: SlaveArtifact,
}

/// A `.slave`/`.islave`-named member that was *not* accepted as a
/// candidate - either because its bytes did not parse as a valid WHDLoad
/// slave, or because it could not safely be read at all. Kept as a
/// diagnostic (never silently dropped) so a malformed member never hides
/// behind a valid one elsewhere in the same archive (see the module's own
/// multi-slave contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSlaveDiagnostic {
    pub member_path: String,
    pub detail: String,
}

/// The complete, deliberately non-resolved result of inspecting one
/// LHA/LZH archive for WHDLoad content. No candidate is ever selected as
/// "the" game here - see [`ArchiveSlaveCandidate`]'s own contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhdloadArchiveDiscovery {
    pub archive_path: PathBuf,
    /// Every member whose bytes passed `parse_whdload_slave`. Empty means
    /// "not a WHDLoad archive", not "malformed archive" - see `diagnostics`
    /// for why any `.slave`-named member was rejected.
    pub candidates: Vec<ArchiveSlaveCandidate>,
    /// `.slave`/`.islave`-named members that did not become a candidate.
    pub diagnostics: Vec<ArchiveSlaveDiagnostic>,
    /// Non-member-specific notes: limits reached, traversal refused, etc.
    pub warnings: Vec<String>,
    /// Total members the archive reports, independent of how many were
    /// `.slave`-named or valid - used for empty-archive / completeness
    /// reporting (see `ingestion::discovery`).
    pub total_members: usize,
}

/// Why an archive could not be inspected at all. Distinct from "inspected,
/// found no valid slave" ([`WhdloadArchiveDiscovery::candidates`] empty),
/// which is not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhdloadArchiveError {
    /// No local 7-Zip advertising an LHA/LZH decoder was found. This is a
    /// missing optional capability, not archive corruption - a caller
    /// should fail soft (treat the archive as unverified, never as
    /// definitely-not-WHDLoad).
    BackendUnavailable,
    /// The archive path itself could not be opened/read, or the backend
    /// refused it (not really an LHA/LZH, encrypted, oversized, malformed).
    Open { detail: String },
}

impl std::fmt::Display for WhdloadArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BackendUnavailable => f.write_str("no LHA/LZH-capable 7-Zip backend was found"),
            Self::Open { detail } => write!(f, "{detail}"),
        }
    }
}
impl std::error::Error for WhdloadArchiveError {}

fn is_slave_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".slave") || lower.ends_with(".islave")
}

/// Inspect `path` (an LHA/LZH archive) for embedded WHDLoad `.slave`
/// members. Bounded and read-only throughout: members are listed once
/// (already bounded by [`ArchiveLimits::default`]'s member-count ceiling),
/// each `.slave`-named member is bounded-read into memory (never to disk,
/// never more than `MAX_INDIVIDUAL_SLAVE_BYTES`/`MAX_TOTAL_SLAVE_BYTES`),
/// and only bytes that parse via [`parse_whdload_slave`] become a
/// candidate.
pub fn discover_whdload_slaves_in_archive(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<WhdloadArchiveDiscovery, WhdloadArchiveError> {
    let provider = LhaProvider::discover(WHDLOAD_ARCHIVE_TIMEOUT).map_err(|error| match error {
        LhaError::BackendNotFound => WhdloadArchiveError::BackendUnavailable,
        other => WhdloadArchiveError::Open {
            detail: other.to_string(),
        },
    })?;
    let parent = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .ok_or_else(|| WhdloadArchiveError::Open {
            detail: "archive path has no readable parent directory".to_string(),
        })?;
    let trusted = TrustedRoots::from_paths([parent]);
    let source = provider
        .open(
            path,
            &trusted,
            ArchiveLimits::default(),
            WHDLOAD_ARCHIVE_TIMEOUT,
        )
        .map_err(|error| WhdloadArchiveError::Open {
            detail: error.to_string(),
        })?;
    Ok(inspect_source(path, &source, cancel))
}

fn inspect_source(
    path: &Path,
    source: &LhaArchiveSource,
    cancel: &AtomicBool,
) -> WhdloadArchiveDiscovery {
    let total_members = source.member_count();
    let mut candidates = Vec::new();
    let mut diagnostics = Vec::new();
    let mut warnings = Vec::new();
    let mut inspected = 0_usize;
    let mut total_bytes = 0_u64;

    let slave_members: Vec<(String, u64)> = source
        .member_infos()
        .filter(|member| is_slave_name(member.path))
        .map(|member| (member.path.to_string(), member.logical_size))
        .collect();

    for (member_path, logical_size) in slave_members {
        use std::sync::atomic::Ordering;
        if cancel.load(Ordering::Relaxed) {
            warnings.push("cancelled before every .slave-named member was inspected".to_string());
            break;
        }
        if inspected >= MAX_SLAVE_CANDIDATES {
            warnings.push(format!(
                "slave-candidate limit {MAX_SLAVE_CANDIDATES} reached; remaining .slave-named \
                 members were not inspected"
            ));
            break;
        }
        inspected += 1;
        if logical_size > MAX_INDIVIDUAL_SLAVE_BYTES {
            diagnostics.push(ArchiveSlaveDiagnostic {
                member_path: member_path.clone(),
                detail: format!(
                    "{logical_size} bytes, over the individual limit of \
                     {MAX_INDIVIDUAL_SLAVE_BYTES} bytes"
                ),
            });
            continue;
        }
        let Some(next_total) = total_bytes.checked_add(logical_size) else {
            diagnostics.push(ArchiveSlaveDiagnostic {
                member_path: member_path.clone(),
                detail: "candidate-byte accounting overflow".to_string(),
            });
            continue;
        };
        if next_total > MAX_TOTAL_SLAVE_BYTES {
            warnings.push(format!(
                "total candidate-byte limit {MAX_TOTAL_SLAVE_BYTES} reached at {member_path}; \
                 remaining .slave-named members were not read"
            ));
            break;
        }
        let bytes = match source.read_member(&member_path, MAX_INDIVIDUAL_SLAVE_BYTES, cancel) {
            Ok(bytes) => bytes,
            Err(error) => {
                diagnostics.push(ArchiveSlaveDiagnostic {
                    member_path: member_path.clone(),
                    detail: format!("could not be read: {error}"),
                });
                continue;
            }
        };
        total_bytes = next_total;
        let parsed = match parse_whdload_slave(&bytes) {
            Ok(parsed) => parsed,
            Err(error) => {
                diagnostics.push(ArchiveSlaveDiagnostic {
                    member_path: member_path.clone(),
                    detail: format!("not a valid WHDLoad slave: {error}"),
                });
                continue;
            }
        };
        candidates.push(ArchiveSlaveCandidate {
            member_path: member_path.clone(),
            artifact: archive_artifact(path.to_path_buf(), member_path, bytes, parsed),
        });
    }

    WhdloadArchiveDiscovery {
        archive_path: path.to_path_buf(),
        candidates,
        diagnostics,
        warnings,
        total_members,
    }
}

/// Convert a validated archive-member slave to a [`SlaveArtifact`] whose
/// `path`/`name` name the *archive* and the *member* respectively - mirrors
/// `amiga_disk::filesystem::embedded_artifact`'s convention for an ADF/HDF-
/// embedded slave. The archive's own container identity (`archive_path`)
/// and the slave's own bytes-derived hashes remain two separate pieces of
/// evidence; this never substitutes one for the other.
fn archive_artifact(
    archive_path: PathBuf,
    member_path: String,
    bytes: Vec<u8>,
    parsed: ParsedWHDLoadSlave,
) -> SlaveArtifact {
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    sha1.update(&bytes);
    sha256.update(&bytes);
    SlaveArtifact {
        path: archive_path,
        name: member_path,
        size_bytes: bytes.len() as u64,
        parsed,
        hashes: SlaveHashes {
            sha1: hexadecimal(sha1.finalize().as_slice()),
            sha256: hexadecimal(sha256.finalize().as_slice()),
        },
    }
}

fn hexadecimal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests;
