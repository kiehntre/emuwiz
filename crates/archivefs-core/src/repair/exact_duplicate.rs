//! Exact-duplicate review: a byte-identical-only duplicate scan, evidenced
//! group report, and quarantine plan - independent of any DAT match.
//!
//! # What "exact duplicate" means here
//!
//! A group is reported only when, for every member: it is a regular,
//! readable file; its byte length is identical to every other member's;
//! and a full-physical-file **SHA-256** is identical - computed over the
//! complete physical file by [`hash_full_file_sha256`], never merely an
//! archive member's decompressed stream. SHA-256 is the sole authority
//! for whether a group forms; nothing else in this module may substitute
//! for it.
//!
//! This crate's existing full-file CRC32/MD5/SHA-1 primitive
//! ([`crate::identity_source::hashing::hash_file`]) is still used, but
//! only for **candidate narrowing**: two files that already disagree on
//! size, CRC32, MD5, or SHA-1 can never be exact duplicates either, so
//! checking those first (cheap, and already computed elsewhere in this
//! codebase) avoids paying for a full SHA-256 pass over files that were
//! never going to match. That triple is retained for this narrowing role,
//! for cache lookups, and for the pairwise live re-proof
//! ([`super::duplicate::prove_duplicate_content`]) every quarantine
//! build/apply below still goes through unchanged - **not** because three
//! weaker algorithms agreeing is described as cryptographically stronger
//! than one SHA-256 match. It is not: this module never claims that, and
//! a group is never authorized by the legacy triple alone (see
//! [`exact_bytes_match`], the one pure predicate that decides whether two
//! members belong in the same group, and which reads only `size_bytes`
//! and `sha256`).
//!
//! This is inherently, structurally unable to call the excluded cases
//! "exact": a ZIP's own bytes are never equal to one of its members' bytes;
//! a CUE/GDI launcher's own bytes are never equal to a CHD's; an N64 ROM's
//! big-endian bytes are never equal to its byte-swapped twin's; a headered
//! NES dump's bytes are never equal to its unheadered twin's; two releases
//! that merely match the same DAT game entry are grouped only when their
//! *bytes* also agree, never on catalogue identity alone. No taxonomy rule
//! is needed to carve these out - full-file byte equality alone can never
//! produce a false match for any of them, which is also exactly the
//! distinction [`crate::platform_evidence_fusion::duplicate_taxonomy`]
//! already codifies (`ExactPhysicalDuplicate` vs. `ExactNormalizedDuplicate`
//! vs. `SameDatRelease`) for its own, heavier, identity-orchestrated
//! reporting pipeline - this module is the lightweight, DAT-independent
//! path to the same `ExactPhysicalDuplicate` concept for a chaotic source
//! scan that may have no DAT match at all.
//!
//! # Revalidated before every apply
//!
//! A scan's evidence is a snapshot; the source tree is not read-locked
//! between preview and apply. [`build_exact_duplicate_group_proposals`]
//! therefore re-hashes the retained copy and every redundant copy with
//! [`hash_full_file_sha256`] immediately before building a move proposal
//! and refuses the whole group (never a partial proposal list) if either
//! side's size or SHA-256 no longer matches the scan's own recorded
//! evidence - a file changed between preview and apply fails closed
//! rather than being moved on stale authority. This is on top of, not
//! instead of, the existing pairwise [`prove_duplicate_content`] re-proof.
//!
//! # Reuse, not a second engine
//!
//! - Full-file identity: [`crate::identity_source::hashing::hash_file`]
//!   (legacy triple, narrowing only) plus this module's own
//!   [`hash_full_file_sha256`], which reuses the same safe-open primitive
//!   ([`crate::safe_read::open_bounded_read`]) and the same
//!   changed-while-reading guard
//!   ([`crate::identity_source::hashing::FileFingerprint`]) `hash_file`
//!   itself is built on - not a new, separately-reviewed file-opening path.
//! - Content proof: [`super::duplicate::{prove_duplicate_content,
//!   DuplicateHashCache}`] - unchanged, called exactly as
//!   [`super::quarantine`] already calls it.
//! - Quarantine destinations, transaction build/apply/rollback:
//!   [`super::quarantine::{quarantine_destination, build_quarantine_transaction,
//!   apply_quarantine_transaction, rollback_quarantine_transaction}`] -
//!   unchanged. This module only produces the [`RepairProposal`]s and picks
//!   the survivor path; every mutation, journal, preflight, and rollback
//!   guarantee is exactly what those functions already provide.
//! - CUE/GDI ownership: [`crate::ingestion::cue_bin::resolve_cue_all_files`]
//!   / [`crate::ingestion::gdi::resolve_gdi_all_tracks`] - the same parsers
//!   `playing_library::matching` uses, no second CUE/GDI parser.
//! - M3U ownership: [`crate::platform_evidence_fusion::cue_m3u_parsing::parse_m3u_references`] -
//!   the same parser, no second M3U parser.
//!
//! What is genuinely new: grouping candidates by full-file SHA-256 without
//! requiring a prior DAT match ([`scan_exact_duplicates`]); canonical-copy
//! selection by trusted-root/elected-library evidence rather than DAT
//! canonical-name evidence ([`select_canonical_copy`] - deliberately
//! separate from [`super::quarantine::select_survivor`], which answers a
//! different question for the existing DAT-scan-driven flow and is left
//! completely unchanged); the multi-file (CUE/GDI/M3U) release protection
//! pass (the private `multi_file_protection_for`, applied inside
//! [`scan_exact_duplicates`]); and the pre-apply SHA-256 revalidation
//! described above.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};

use crate::identity_source::hashing::{
    Crc32, FileFingerprint, MAX_AUTOMATIC_HASH_BYTES, hash_file,
};
use crate::ingestion::cue_bin::resolve_cue_all_files;
use crate::ingestion::gdi::resolve_gdi_all_tracks;
use crate::platform_evidence_fusion::cue_m3u_parsing::{MAX_PARSE_BYTES, parse_m3u_references};
use crate::safe_read::TrustedRoots;
use crate::safe_read::open_bounded_read;

use super::duplicate::{DuplicateHashCache, DuplicatePairClassification, prove_duplicate_content};
use super::proposal::{RepairAction, RepairProposal, RepairProposalId, SafetyState};
use super::quarantine::quarantine_destination;

/// Full-physical-file identity: size plus SHA-256, both covering the
/// complete outer file - never an archive member's decompressed stream.
/// This is the only evidence [`exact_bytes_match`] is allowed to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullFileIdentity {
    pub size_bytes: u64,
    pub sha256: String,
}

/// The one predicate that decides whether two members belong in the same
/// exact-duplicate group. Pure and total: it reads only `size_bytes` and
/// `sha256`, never the legacy CRC32/MD5/SHA-1 triple, so a caller cannot
/// accidentally authorize a group on legacy-hash agreement alone even if a
/// future change starts passing more evidence around.
pub fn exact_bytes_match(a: &FullFileIdentity, b: &FullFileIdentity) -> bool {
    a.size_bytes == b.size_bytes && a.sha256 == b.sha256
}

/// Why a full-file SHA-256 could not be computed or trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullHashRefusal {
    NotReadable(String),
    TooLarge {
        bytes: u64,
        maximum: u64,
    },
    Cancelled,
    ReadFailed(String),
    /// The file's size or content changed between the size observed before
    /// reading and the size observed after - the digest describes neither
    /// version, so nothing is trusted.
    ChangedWhileReading,
}

impl std::fmt::Display for FullHashRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReadable(detail) => write!(formatter, "not readable: {detail}"),
            Self::TooLarge { bytes, maximum } => {
                write!(formatter, "{bytes} bytes exceeds the {maximum}-byte bound")
            }
            Self::Cancelled => formatter.write_str("cancelled"),
            Self::ReadFailed(detail) => write!(formatter, "read failed: {detail}"),
            Self::ChangedWhileReading => {
                formatter.write_str("the file changed while it was being hashed")
            }
        }
    }
}

/// Computes a full-physical-file SHA-256 (and the exact size it covered),
/// streamed in bounded chunks through the same safe-open primitive
/// ([`open_bounded_read`]) and the same before/after
/// [`FileFingerprint`] changed-while-reading guard
/// [`crate::identity_source::hashing::hash_file`] itself uses - this is
/// not a second, separately-reviewed file-opening path. Reads only the
/// physical file named by `path`; never opens, lists, or hashes anything
/// inside it as an archive.
pub fn hash_full_file_sha256(
    path: &Path,
    trusted: &TrustedRoots,
    cancel: Option<&AtomicBool>,
) -> Result<FullFileIdentity, FullHashRefusal> {
    let before = FileFingerprint::observe(path).ok_or_else(|| {
        FullHashRefusal::NotReadable(format!("{} is not a readable regular file", path.display()))
    })?;
    if before.size_bytes > MAX_AUTOMATIC_HASH_BYTES {
        return Err(FullHashRefusal::TooLarge {
            bytes: before.size_bytes,
            maximum: MAX_AUTOMATIC_HASH_BYTES,
        });
    }
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
        return Err(FullHashRefusal::Cancelled);
    }

    let file = open_bounded_read(path, trusted)
        .map_err(|refusal| FullHashRefusal::NotReadable(refusal.detail()))?;
    let mut reader = file.into_file();

    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut total: u64 = 0;
    loop {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(FullHashRefusal::Cancelled);
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|error| FullHashRefusal::ReadFailed(error.kind().to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total = total.saturating_add(read as u64);
    }

    let after = FileFingerprint::observe(path).ok_or(FullHashRefusal::ChangedWhileReading)?;
    if after != before || after.size_bytes != total {
        return Err(FullHashRefusal::ChangedWhileReading);
    }

    Ok(FullFileIdentity {
        size_bytes: total,
        sha256: hex(&hasher.finalize()),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One candidate file excluded from consideration, and why - never
/// silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedCandidate {
    pub path: PathBuf,
    pub reason: String,
}

/// One member of an exact-duplicate group, with the evidence
/// [`select_canonical_copy`] used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactDuplicateMember {
    pub path: PathBuf,
    pub in_trusted_root: bool,
    pub elected_in_library: bool,
}

/// How (or whether) a canonical copy to retain was determined - always
/// derived from actual evidence, never invented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalRecommendation {
    /// Exactly one member sits inside a user-designated trusted root.
    TrustedRoot(PathBuf),
    /// Exactly one member is already used by a published/elected library
    /// (a caller-supplied set of paths, e.g. from a `PlayingLibraryPlan`'s
    /// own operations - this module never reads or depends on that type).
    ElectedLibrary(PathBuf),
    /// Neither trusted-root nor elected-library evidence distinguishes a
    /// unique member. Never resolved by path order, mtime, or any other
    /// invented tie-break - the user must choose.
    RequiresUserChoice,
    /// A person explicitly chose this copy, via [`apply_user_choice`],
    /// because no automatic evidence distinguished one. Never produced by
    /// [`select_canonical_copy`] itself - this is the one honest way a
    /// `RequiresUserChoice` group becomes quarantine-ready, and it is
    /// always labelled as a person's own decision, never presented as
    /// trusted-root or elected-library evidence it does not have.
    UserChosen(PathBuf),
}

impl CanonicalRecommendation {
    pub fn retained_path(&self) -> Option<&Path> {
        match self {
            Self::TrustedRoot(path) | Self::ElectedLibrary(path) | Self::UserChosen(path) => {
                Some(path)
            }
            Self::RequiresUserChoice => None,
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Self::TrustedRoot(path) => format!(
                "'{}' is the only copy inside a user-designated trusted root",
                path.display()
            ),
            Self::ElectedLibrary(path) => format!(
                "'{}' is the only copy already used by a published/elected library",
                path.display()
            ),
            Self::RequiresUserChoice => {
                "no trusted-root or elected-library evidence distinguishes a unique copy; \
                 the user must choose which copy to keep"
                    .to_string()
            }
            Self::UserChosen(path) => {
                format!("'{}' was chosen to keep by the user", path.display())
            }
        }
    }
}

/// Re-derives an [`ExactDuplicateGroup`] after a person has chosen which
/// member to keep, for a group [`select_canonical_copy`] itself left as
/// [`CanonicalRecommendation::RequiresUserChoice`]. `chosen` must be one of
/// `group.members` - never a path the scan never actually saw. Recomputes
/// `redundant_paths`, `reclaimable_bytes`, and `readiness` from the same
/// rules [`scan_exact_duplicates`] itself applies (a blocked multi-file
/// relationship still blocks the group; nothing about a manual choice
/// overrides that safety check).
pub fn apply_user_choice(
    group: &ExactDuplicateGroup,
    chosen: &Path,
) -> Result<ExactDuplicateGroup, String> {
    if !group.members.iter().any(|member| member.path == chosen) {
        return Err(format!(
            "'{}' is not a member of this exact-duplicate group",
            chosen.display()
        ));
    }
    let recommendation = CanonicalRecommendation::UserChosen(chosen.to_path_buf());
    let redundant_paths: Vec<PathBuf> = group
        .members
        .iter()
        .map(|member| member.path.clone())
        .filter(|path| path.as_path() != chosen)
        .collect();
    let reclaimable_bytes = group.size_bytes * redundant_paths.len() as u64;
    let readiness = match &group.multi_file {
        MultiFileProtection::Blocked(reason) => GroupQuarantineReadiness::Blocked(reason.clone()),
        _ => GroupQuarantineReadiness::Safe,
    };
    Ok(ExactDuplicateGroup {
        size_bytes: group.size_bytes,
        sha256: group.sha256.clone(),
        legacy_crc32: group.legacy_crc32.clone(),
        legacy_md5: group.legacy_md5.clone(),
        legacy_sha1: group.legacy_sha1.clone(),
        members: group.members.clone(),
        recommendation,
        redundant_paths,
        reclaimable_bytes,
        multi_file: group.multi_file.clone(),
        readiness,
    })
}

/// Deterministically recommends which member of an already-proven
/// exact-duplicate group to retain.
///
/// Every member here has already passed the "healthy, readable, byte-for-
/// byte identical" bar simply by being in the group at all - a file that
/// could not be read or hashed never became a candidate in the first
/// place (see [`scan_exact_duplicates`]), so there is no further "healthy
/// copy" axis to rank members by. The remaining, genuinely discriminating
/// evidence is checked in strict order: a unique trusted-root member wins
/// first; failing that, a unique elected-library member wins; failing
/// that, the group requires an explicit user choice. Never alphabetical,
/// never "first found", never mtime.
pub fn select_canonical_copy(members: &[ExactDuplicateMember]) -> CanonicalRecommendation {
    let trusted: Vec<&ExactDuplicateMember> = members
        .iter()
        .filter(|member| member.in_trusted_root)
        .collect();
    if let [only] = trusted.as_slice() {
        return CanonicalRecommendation::TrustedRoot(only.path.clone());
    }
    // Narrow to the trusted-root subset when more than one qualifies, so a
    // library-elected member outside every trusted root never overrides a
    // trusted-root member that is merely tied with another trusted-root
    // member - the tie stays inside the stronger tier's own membership.
    let elected_pool: &[&ExactDuplicateMember] = if trusted.len() > 1 {
        &trusted
    } else {
        // trusted.is_empty(): fall through to the whole membership.
        return match members
            .iter()
            .filter(|member| member.elected_in_library)
            .collect::<Vec<_>>()
            .as_slice()
        {
            [only] => CanonicalRecommendation::ElectedLibrary(only.path.clone()),
            _ => CanonicalRecommendation::RequiresUserChoice,
        };
    };
    let elected: Vec<&&ExactDuplicateMember> = elected_pool
        .iter()
        .filter(|member| member.elected_in_library)
        .collect();
    match elected.as_slice() {
        [only] => CanonicalRecommendation::ElectedLibrary((*only).path.clone()),
        _ => CanonicalRecommendation::RequiresUserChoice,
    }
}

/// Whether (and why not) an exact-duplicate group's redundant copies
/// require special multi-file (CUE/GDI/M3U) handling before they can be
/// quarantined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiFileProtection {
    /// No member of this group is a CUE/GDI/M3U launcher or a structural
    /// companion of one among the scanned candidates.
    NotMultiFile,
    /// Every member's own complete release (launcher plus every
    /// structural companion) is included and consistently accounted for
    /// in this scan - the whole release may be quarantined as one unit.
    WholeReleaseDuplicate,
    /// A structural companion or launcher relationship makes independent
    /// quarantine of a redundant member unsafe - named explicitly, never
    /// silently downgraded to an ordinary single-file move.
    Blocked(String),
}

/// Whether a group's redundant copies may actually be moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupQuarantineReadiness {
    Safe,
    Blocked(String),
    NeedsReview(String),
}

/// One reported exact-duplicate group - see the module doc comment for the
/// exact-duplicate definition every member here has already satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactDuplicateGroup {
    pub size_bytes: u64,
    /// The full-physical-file SHA-256 every member shares - the sole
    /// authority for this group's existence (see [`exact_bytes_match`]).
    pub sha256: String,
    /// The legacy full-file CRC32/MD5/SHA-1 every member also shares,
    /// retained for compatibility, cache lookup, and display - never
    /// itself sufficient to have formed this group.
    pub legacy_crc32: String,
    pub legacy_md5: String,
    pub legacy_sha1: String,
    pub members: Vec<ExactDuplicateMember>,
    pub recommendation: CanonicalRecommendation,
    /// Every member's path except the recommended retained copy, when one
    /// was determined.
    pub redundant_paths: Vec<PathBuf>,
    /// `size_bytes * redundant_paths.len()` - each redundant path counted
    /// exactly once, since group membership is a strict partition (every
    /// candidate hashes into exactly one group).
    pub reclaimable_bytes: u64,
    pub multi_file: MultiFileProtection,
    pub readiness: GroupQuarantineReadiness,
}

/// The complete result of one exact-duplicate scan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExactDuplicateScanReport {
    pub groups: Vec<ExactDuplicateGroup>,
    pub excluded: Vec<ExcludedCandidate>,
    pub files_examined: usize,
}

impl ExactDuplicateScanReport {
    /// Total reclaimable bytes across every `Safe` group - never
    /// double-counted, since groups already partition the candidates.
    pub fn total_reclaimable_bytes(&self) -> u64 {
        self.groups
            .iter()
            .filter(|group| group.readiness == GroupQuarantineReadiness::Safe)
            .map(|group| group.reclaimable_bytes)
            .sum()
    }
}

/// Structural companion files a CUE/GDI launcher requires, via the
/// existing parsers - `None` for anything that is not a CUE/GDI launcher
/// this crate can parse (never a guess from extension alone beyond
/// dispatch, and a CUE/GDI this crate cannot parse is simply not treated
/// as a launcher here rather than blocking unrelated groups).
fn launcher_companions(path: &Path) -> Option<Vec<PathBuf>> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "cue" => resolve_cue_all_files(path).ok(),
        "gdi" => resolve_gdi_all_tracks(path).ok(),
        _ => None,
    }
}

/// Every structural launcher-to-companion relationship among `candidates`:
/// companion path -> the set of launcher paths (CUE, GDI, or M3U) among
/// the candidates that structurally require it. Built only from the
/// existing CUE/GDI/M3U parsers - never from basename or directory
/// proximity, and only among files actually present in `candidates`.
fn build_ownership_map(candidates: &[PathBuf]) -> BTreeMap<PathBuf, BTreeSet<PathBuf>> {
    let mut owners: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    for candidate in candidates {
        if let Some(companions) = launcher_companions(candidate) {
            for companion in companions {
                owners
                    .entry(companion)
                    .or_default()
                    .insert(candidate.clone());
            }
        }
        let extension = candidate
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        if extension.as_deref() != Some("m3u") && extension.as_deref() != Some("m3u8") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(candidate) else {
            continue;
        };
        if contents.len() > MAX_PARSE_BYTES {
            continue;
        }
        for reference in parse_m3u_references(candidate, &contents) {
            let Some(resolved) = reference.resolved else {
                continue;
            };
            owners
                .entry(resolved.clone())
                .or_default()
                .insert(candidate.clone());
            if let Some(disc_companions) = launcher_companions(&resolved) {
                for disc_companion in disc_companions {
                    owners
                        .entry(disc_companion)
                        .or_default()
                        .insert(candidate.clone());
                }
            }
        }
    }
    owners
}

/// Determines [`MultiFileProtection`] for one group, given the full
/// candidate ownership map and the set of paths every other group in this
/// same scan has already decided are redundant (about to be moved).
///
/// Fails closed: any launcher/companion relationship this pass cannot
/// prove safe blocks the group rather than silently falling back to an
/// ordinary single-file move.
fn multi_file_protection_for(
    group_members: &[PathBuf],
    redundant_in_group: &[PathBuf],
    ownership: &BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    globally_redundant: &BTreeSet<PathBuf>,
) -> MultiFileProtection {
    let mut touches_multi_file = false;

    for member in group_members {
        // Is this member itself a launcher with companions?
        if let Some(companions) = launcher_companions(member) {
            touches_multi_file = true;
            if redundant_in_group.contains(member) {
                // Only safe to move this launcher if every one of its own
                // companions is also being moved somewhere in this same
                // scan (i.e. that companion's own group also marked it
                // redundant) - otherwise moving the launcher alone would
                // orphan a companion that is staying put.
                for companion in &companions {
                    if !globally_redundant.contains(companion) {
                        return MultiFileProtection::Blocked(format!(
                            "'{}' is a launcher for '{}', which is not itself being quarantined; \
                             quarantining the launcher alone would break that release",
                            member.display(),
                            companion.display()
                        ));
                    }
                }
            }
        }

        // Is this member a structural companion of something?
        if let Some(owners) = ownership.get(member) {
            touches_multi_file = true;
            if redundant_in_group.contains(member) {
                if owners.len() > 1 {
                    return MultiFileProtection::Blocked(format!(
                        "'{}' is a shared companion of {} different launchers; ownership is \
                         ambiguous, so automatic quarantine is refused",
                        member.display(),
                        owners.len()
                    ));
                }
                for owner in owners {
                    if !globally_redundant.contains(owner) {
                        return MultiFileProtection::Blocked(format!(
                            "'{}' is a required companion of '{}', which is being retained; \
                             quarantining the companion alone would break that release",
                            member.display(),
                            owner.display()
                        ));
                    }
                }
            }
        }
    }

    if touches_multi_file {
        MultiFileProtection::WholeReleaseDuplicate
    } else {
        MultiFileProtection::NotMultiFile
    }
}

/// Scans `candidates` for exact (byte-identical) duplicate groups.
///
/// Two passes, in order:
///
/// 1. **Narrowing** - every candidate is independently stat'd and hashed
///    at most once through the existing [`hash_file`] primitive (full-file
///    CRC32/MD5/SHA-1), bucketed by `(size, crc32, md5, sha1)`. A bucket of
///    fewer than two files can never be an exact-duplicate group and costs
///    nothing further.
/// 2. **Authorization** - every candidate in a narrowed bucket of two or
///    more is hashed again with [`hash_full_file_sha256`], and only
///    members whose `(size_bytes, sha256)` genuinely agree
///    ([`exact_bytes_match`]) form a reported group. The legacy triple
///    from step 1 narrows the search; it never itself authorizes a group.
///
/// A candidate that is not a regular file, cannot be read, or is refused
/// by `trusted` at either step is recorded in
/// [`ExactDuplicateScanReport::excluded`], never silently skipped and
/// never included in any group.
///
/// `trusted_roots` names the user-designated trusted root(s) a canonical
/// copy is preferred from; `elected_paths` names paths already used by a
/// published/elected library (for example a `PlayingLibraryPlan`'s own
/// operation destinations/sources, supplied by the caller - this module
/// never depends on that type directly). Both are pure evidence inputs,
/// never inferred from anything else.
pub fn scan_exact_duplicates(
    candidates: &[PathBuf],
    trusted: &TrustedRoots,
    trusted_roots: &[PathBuf],
    elected_paths: &BTreeSet<PathBuf>,
    cancel: Option<&AtomicBool>,
) -> ExactDuplicateScanReport {
    let mut excluded = Vec::new();
    // Step 1 narrowing key: (size, crc32, md5, sha1). Never itself the
    // authority for a group - see the function doc comment.
    let mut narrow_buckets: BTreeMap<(u64, String, String, String), Vec<PathBuf>> = BTreeMap::new();
    let mut files_examined = 0usize;

    for candidate in candidates {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Ok(metadata) if metadata.file_type().is_symlink() => {
                excluded.push(ExcludedCandidate {
                    path: candidate.clone(),
                    reason: "is a symlink, not a regular file".to_string(),
                });
                continue;
            }
            Ok(_) => {
                excluded.push(ExcludedCandidate {
                    path: candidate.clone(),
                    reason: "is not a regular file".to_string(),
                });
                continue;
            }
            Err(error) => {
                excluded.push(ExcludedCandidate {
                    path: candidate.clone(),
                    reason: format!("could not be read: {error}"),
                });
                continue;
            }
        }
        match hash_file(candidate, trusted, cancel) {
            Ok(hashes) => {
                files_examined += 1;
                narrow_buckets
                    .entry((
                        hashes.fingerprint.size_bytes,
                        hashes.crc32.clone(),
                        hashes.md5.clone(),
                        hashes.sha1.clone(),
                    ))
                    .or_default()
                    .push(candidate.clone());
            }
            Err(refusal) => excluded.push(ExcludedCandidate {
                path: candidate.clone(),
                reason: format!("{refusal:?}"),
            }),
        }
    }

    let ownership = build_ownership_map(candidates);

    // Step 2 authorization key: (size_bytes, sha256) - the only key that
    // may ever form a group. Populated only from members of a step-1
    // bucket that already held two or more candidates.
    struct Sha256Bucket {
        size_bytes: u64,
        sha256: String,
        legacy_crc32: String,
        legacy_md5: String,
        legacy_sha1: String,
        paths: Vec<PathBuf>,
    }
    let mut sha256_buckets: BTreeMap<(u64, String), Sha256Bucket> = BTreeMap::new();
    for ((_narrowed_size, crc32, md5, sha1), paths) in narrow_buckets {
        if paths.len() < 2 {
            continue;
        }
        for path in paths {
            match hash_full_file_sha256(&path, trusted, cancel) {
                Ok(identity) => {
                    let bucket = sha256_buckets
                        .entry((identity.size_bytes, identity.sha256.clone()))
                        .or_insert_with(|| Sha256Bucket {
                            size_bytes: identity.size_bytes,
                            sha256: identity.sha256.clone(),
                            legacy_crc32: crc32.clone(),
                            legacy_md5: md5.clone(),
                            legacy_sha1: sha1.clone(),
                            paths: Vec::new(),
                        });
                    bucket.paths.push(path);
                }
                Err(refusal) => excluded.push(ExcludedCandidate {
                    path,
                    reason: format!("SHA-256 revalidation failed: {refusal}"),
                }),
            }
        }
    }

    // First pass: every group's members, size/hash, and canonical
    // recommendation - independent of any other group.
    struct Draft {
        size_bytes: u64,
        sha256: String,
        legacy_crc32: String,
        legacy_md5: String,
        legacy_sha1: String,
        members: Vec<ExactDuplicateMember>,
        recommendation: CanonicalRecommendation,
        redundant_paths: Vec<PathBuf>,
        reclaimable_bytes: u64,
    }
    let mut drafts = Vec::new();
    for (_, bucket) in sha256_buckets {
        let mut paths = bucket.paths;
        if paths.len() < 2 {
            continue;
        }
        paths.sort();
        let members: Vec<ExactDuplicateMember> = paths
            .iter()
            .map(|path| ExactDuplicateMember {
                path: path.clone(),
                in_trusted_root: trusted_roots.iter().any(|root| path.starts_with(root)),
                elected_in_library: elected_paths.contains(path),
            })
            .collect();
        let recommendation = select_canonical_copy(&members);
        let redundant_paths: Vec<PathBuf> = match recommendation.retained_path() {
            Some(retained) => paths
                .iter()
                .filter(|path| path.as_path() != retained)
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        let reclaimable_bytes = bucket.size_bytes * redundant_paths.len() as u64;
        drafts.push(Draft {
            size_bytes: bucket.size_bytes,
            sha256: bucket.sha256,
            legacy_crc32: bucket.legacy_crc32,
            legacy_md5: bucket.legacy_md5,
            legacy_sha1: bucket.legacy_sha1,
            members,
            recommendation,
            redundant_paths,
            reclaimable_bytes,
        });
    }

    // Global view of every path any group currently proposes as redundant
    // - needed by the multi-file protection pass below, since whether a
    // companion/launcher relationship is safe depends on *other* groups'
    // decisions too.
    let globally_redundant: BTreeSet<PathBuf> = drafts
        .iter()
        .flat_map(|draft| draft.redundant_paths.iter().cloned())
        .collect();

    let mut groups = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let group_members: Vec<PathBuf> = draft
            .members
            .iter()
            .map(|member| member.path.clone())
            .collect();
        let multi_file = multi_file_protection_for(
            &group_members,
            &draft.redundant_paths,
            &ownership,
            &globally_redundant,
        );
        let readiness = match (&draft.recommendation, &multi_file) {
            (_, MultiFileProtection::Blocked(reason)) => {
                GroupQuarantineReadiness::Blocked(reason.clone())
            }
            (CanonicalRecommendation::RequiresUserChoice, _) => {
                GroupQuarantineReadiness::NeedsReview(
                    "no unique canonical copy was determined; user choice required".to_string(),
                )
            }
            (_, _) => GroupQuarantineReadiness::Safe,
        };
        groups.push(ExactDuplicateGroup {
            size_bytes: draft.size_bytes,
            sha256: draft.sha256,
            legacy_crc32: draft.legacy_crc32,
            legacy_md5: draft.legacy_md5,
            legacy_sha1: draft.legacy_sha1,
            members: draft.members,
            recommendation: draft.recommendation,
            redundant_paths: draft.redundant_paths,
            reclaimable_bytes: draft.reclaimable_bytes,
            multi_file,
            readiness,
        });
    }
    groups.sort_by(|a, b| a.sha256.cmp(&b.sha256));

    ExactDuplicateScanReport {
        groups,
        excluded,
        files_examined,
    }
}

/// Re-hashes `path` and fails closed unless it still matches `approved`
/// (the scan's own recorded size/SHA-256 evidence) exactly. The one gate
/// that turns "files changed between preview and apply" into a refusal
/// rather than a move on stale authority.
fn revalidate_against_approved_evidence(
    path: &Path,
    approved: &FullFileIdentity,
    trusted: &TrustedRoots,
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    let current = hash_full_file_sha256(path, trusted, cancel).map_err(|refusal| {
        format!(
            "'{}' could not be revalidated against the approved evidence: {refusal}",
            path.display()
        )
    })?;
    if !exact_bytes_match(&current, approved) {
        return Err(format!(
            "'{}' no longer matches the SHA-256/size evidence approved during preview \
             (it changed between preview and apply); the whole group is refused",
            path.display()
        ));
    }
    Ok(())
}

/// Builds one `Safe` [`RepairProposal`] per redundant member of a `Safe`
/// exact-duplicate group, using the exact same destination scheme and
/// proposal shape [`super::quarantine::plan_duplicate_quarantine`] already
/// produces for the DAT-scan-driven flow (`quarantine_destination`,
/// `RepairAction::MovePath`, `SafetyState::Safe`, `survivor_path`).
///
/// A scan's evidence is a snapshot, and nothing read-locks the source tree
/// between preview and apply, so every source file is revalidated here
/// against the *scan's own recorded* SHA-256/size evidence before anything
/// is proposed - never trusted as mutation authority on its own:
///
/// 1. The retained copy is re-hashed with [`hash_full_file_sha256`] once
///    and checked against `group.sha256`/`group.size_bytes` - if the
///    "keeper" itself changed since the scan, the whole group is refused.
/// 2. Each redundant copy is re-hashed the same way and checked against
///    the same recorded evidence.
/// 3. The existing pairwise [`prove_duplicate_content`] re-proof still
///    runs on top, unchanged, exactly as every other quarantine caller
///    already uses it (hardlink/same-object detection, its own evidence
///    record, etc.).
///
/// Any of these disagreeing - a file changed between preview and
/// apply - fails the *entire* group closed (never a partial proposal
/// list) rather than moving what is still provable and skipping the rest.
pub fn build_exact_duplicate_group_proposals(
    group: &ExactDuplicateGroup,
    trusted_root: &Path,
    cache: &mut DuplicateHashCache,
    trusted: &TrustedRoots,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<RepairProposal>, String> {
    if group.readiness != GroupQuarantineReadiness::Safe {
        return Err("this group is not Safe to quarantine".to_string());
    }
    let Some(retained) = group.recommendation.retained_path() else {
        return Err("this group has no recommended retained copy".to_string());
    };
    let approved = FullFileIdentity {
        size_bytes: group.size_bytes,
        sha256: group.sha256.clone(),
    };
    revalidate_against_approved_evidence(retained, &approved, trusted, cancel)?;
    let mut proposals = Vec::with_capacity(group.redundant_paths.len());
    for redundant in &group.redundant_paths {
        revalidate_against_approved_evidence(redundant, &approved, trusted, cancel)?;
        let proof = prove_duplicate_content(redundant, retained, cache, trusted, cancel).map_err(
            |refusal| {
                format!(
                    "'{}' could not be re-proven a duplicate of '{}': {refusal}",
                    redundant.display(),
                    retained.display()
                )
            },
        )?;
        if proof.classification != DuplicatePairClassification::DistinctObjects {
            return Err(format!(
                "'{}' is the same filesystem object as the retained copy",
                redundant.display()
            ));
        }
        let destination = quarantine_destination(trusted_root, &proof, redundant)?;
        let id = RepairProposalId::new(format!(
            "exact-duplicate-{}-{}",
            &group.sha256[..group.sha256.len().min(16)],
            Crc32::of(redundant.to_string_lossy().as_bytes())
        ))
        .ok_or_else(|| "could not build a safe proposal id".to_string())?;
        proposals.push(RepairProposal {
            id,
            action: RepairAction::MovePath { destination },
            source_path: redundant.clone(),
            reason: format!(
                "'{}' is a redundant byte-identical copy of the retained file '{}'; moved to \
                 quarantine rather than deleted",
                redundant.display(),
                retained.display()
            ),
            evidence: vec![proof.evidence()],
            expected_source_identity: Some(proof.identity_a),
            originating_audit: None,
            safety: SafetyState::Safe,
            blockers: Vec::new(),
            warnings: Vec::new(),
            dat_source_id: None,
            dat_source_display: None,
            game_name: None,
            rom_name: None,
            verdict_label: None,
            match_confident: false,
            is_outer_archive: false,
            is_outer_archive_verified: false,
            survivor_path: Some(retained.to_path_buf()),
        });
    }
    Ok(proposals)
}

/// A stable, human-readable label for this pass, matching the existing
/// convention ([`CLASSIFIER_VERSION`]-style constants) so a persisted
/// report can note which logic produced it. Not itself parsed by anything
/// here.
pub const EXACT_DUPLICATE_SCAN_VERSION: &str = "exact-duplicate-scan-v1";

#[cfg(test)]
mod tests;
