//! Running a read-only DAT audit over a folder of local files.
//!
//! [`crate::dat::audit`] compares *known evidence* against an index and never
//! touches a file. This module is what produces that evidence for a real
//! library: it walks a folder, hashes what it finds, and hands the results to
//! the existing audit. Every verdict a run reports comes from
//! [`crate::dat::audit::audit_files`] unchanged - no category is added here,
//! and none is reinterpreted.
//!
//! # Read-only, and provably so
//!
//! The only filesystem calls in this module are `read_dir`, `symlink_metadata`,
//! and [`crate::identity_source::hashing::hash_file_reporting`], which opens
//! read-only through [`crate::safe_read`]. There is no create, write, rename,
//! remove, truncate, permission change, or symlink operation anywhere in the
//! module, and nothing is written beside the files being scanned. An audit
//! leaves a library byte-for-byte as it found it.
//!
//! # Bounded and cancellable
//!
//! - The walk stops at [`MAX_SCAN_DEPTH`] directories deep and
//!   [`MAX_SCAN_FILES`] files, reporting that it truncated rather than
//!   pretending it saw everything.
//! - Files are hashed in fixed chunks, so memory is flat regardless of how big
//!   a disc image is.
//! - The cancellation flag is checked before every file and inside every chunk,
//!   so stopping a run over a large library takes effect within one chunk
//!   rather than at the end.
//! - Progress is reported through a callback the caller supplies. The callback
//!   runs on the worker thread and must not block; a GUI sends one bounded
//!   channel message and returns.
//!
//! # What "no hash" means in a verdict
//!
//! A file too large for automatic hashing, or one the read policy refuses, is
//! still audited - by name only - and is listed separately in
//! [`DatAuditOutcome::unhashed`] with the reason. That distinction matters: a
//! `FilenameOnly` verdict for a file nobody hashed says a *name* is in the
//! catalogue, not that this file is, and the report has to be able to say which
//! of the two happened.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;

use super::{DatSourceKind, validation};
use crate::dat::archive::limits::{ArchiveLimits, MAX_ARCHIVE_RUN_LOGICAL_BYTES};
use crate::dat::archive::rar::{RarArchiveSource, RarError, RarProvider};
use crate::dat::archive::sevenz::SevenZArchiveSource;
use crate::dat::archive::zip::ZipArchiveSource;
use crate::dat::archive::{
    ArchiveMemberEvidence, ArchiveMemberSource, ArchiveMemberSourceError, ArchiveMemberStatus,
    ArchivePassCompletion, ArchivePassStopReason, ArchiveRunBudget,
};
use crate::dat::audit::{
    AuditEntry, AuditReport, AuditSummary, AuditVerdict, KnownFileEvidence, audit_files, audit_one,
};
use crate::dat::classification::{
    ContentEligibility, ContentSelectionPolicy, DatContentClassification, DatContentSummary,
    DatOriginalMetadata, summarize,
};
use crate::dat::dependency::resolve::{CollectionEvidence, resolve_collection};
use crate::dat::disk_audit::{DatDiskAudit, audit_chd_disk, is_chd_path};
use crate::dat::index::{DatDiskIndex, DatIndex, DatMemberKey, DatRomRef, MemberLocation};
use crate::dat::limits::DatLimits;
use crate::dat::model::{DatGameEntry, ParsedDat};
use crate::dat::parsers::parse_dat_file;
use crate::dat::policy::candidate::candidate_for_rom;
use crate::dat::policy::evaluate::{CandidateResolution, EffectiveDatPolicy, rank_candidates};
use crate::dat::set::{SetResolution, classify_archive_sets};
use crate::identity_source::hashing::{HashRefusal, hash_file_reporting};
use crate::safe_read::TrustedRoots;

/// How deep the scan descends below the chosen folder.
///
/// A ROM library is normally two or three levels - platform, maybe publisher,
/// then files. Eight leaves generous room for an unusual arrangement while
/// keeping the walk finite on a tree that has been made pathological.
pub const MAX_SCAN_DEPTH: usize = 8;

/// How many files one audit run will take.
///
/// This is the memory bound: each file contributes one [`KnownFileEvidence`]
/// (a handful of short strings) and one [`crate::dat::audit::AuditEntry`], so
/// the ceiling is what keeps a run over an enormous tree from growing without
/// limit. Exceeding it truncates the run and says so.
pub const MAX_SCAN_FILES: usize = 25_000;

/// How many directory entries the walk will examine, DAT-relevant or not.
pub const MAX_SCAN_ENTRIES_EXAMINED: usize = 200_000;

/// What the audit is being run against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatAuditRequest {
    pub source_id: String,
    pub source_display_name: String,
    pub dat_path: PathBuf,
    pub dat_kind: DatSourceKind,
    /// One local regular file, or a folder of local files, to compare against
    /// the catalogue. A single-file target is useful for a deliberately
    /// bounded evidence check before auditing a whole collection.
    pub scan_root: PathBuf,
    pub limits: DatLimits,
    /// The effective DAT policy, when the caller wants multi-candidate
    /// verdicts annotated with the user's preference order.
    ///
    /// `None` (the default) makes the audit behave exactly as it did before
    /// policy existed: every verdict is reported, none is preferred.
    pub policy: Option<EffectiveDatPolicy>,
    /// The audited source's canonical platform id, when assigned and
    /// recognised. Carried for provenance so a rename plan derived from the
    /// outcome can report the platform without re-reading the registry.
    pub platform: Option<String>,
}

/// One enabled, locally readable catalogue participating in a combined audit.
///
/// Callers construct this only from an existing local registry entry or a
/// validated managed snapshot projection.  It deliberately carries no URL or
/// remote-provider authority: combined auditing is wholly local and read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedDatAuditSource {
    pub source_id: String,
    pub source_display_name: String,
    pub dat_path: PathBuf,
    pub dat_kind: DatSourceKind,
    /// A canonical platform when this source was explicitly assigned one.
    /// `None` is retained honestly; a DAT header alone is not promoted to a
    /// platform assignment here.
    pub platform: Option<String>,
}

/// A read-only request to compare one library target with every enabled local
/// and managed game catalogue the caller supplies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedDatAuditRequest {
    pub sources: Vec<CombinedDatAuditSource>,
    pub scan_root: PathBuf,
    pub limits: DatLimits,
}

/// One exact catalogue observation retained for a combined-match result.
///
/// Agreement is represented by several observations with the same canonical
/// game/ROM identity.  Disagreement never gets collapsed to a first source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatAuditEvidenceSource {
    pub local_path: String,
    pub source_id: String,
    pub source_display_name: String,
    pub platform: Option<String>,
    pub game_name: String,
    pub rom_name: String,
    pub algorithm: String,
}

/// A file that was audited without hash evidence, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnhashedFile {
    pub path: String,
    pub file_name: String,
    /// A stable reason code from [`HashRefusal::code`].
    pub code: String,
    pub detail: String,
}

/// Progress from a running audit.
///
/// Every variant is cheap to construct: a run over 20,000 files must not spend
/// its time building progress messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatAuditProgress {
    /// Reading the catalogue itself.
    ReadingCatalogue { file_name: String },
    /// The catalogue is indexed and the walk is about to start.
    CatalogueReady { entries: usize, roms: usize },
    /// Walking the folder, before any hashing. `files_found` is how many have
    /// been collected so far; `current_dir` is the directory currently being
    /// walked, as full text for the display layer to shorten - never as a path
    /// that must be shown verbatim.
    Scanning {
        files_found: usize,
        current_dir: Option<String>,
    },
    /// Hashing one file. `index` is 1-based over `total`.
    Hashing {
        index: usize,
        total: usize,
        file_name: String,
    },
    /// Comparing the collected evidence against the index.
    Comparing { files: usize },
}

/// Why an audit could not produce a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatAuditError {
    /// The DAT source's own path was refused.
    DatPath(String),
    /// The folder to audit was refused.
    ScanPath(String),
    /// Every DAT file in the source failed to parse.
    NoCatalogue(String),
    /// There was nothing to compare.
    NothingToAudit(String),
    Cancelled,
}

impl std::fmt::Display for DatAuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DatPath(detail) => write!(f, "the DAT source could not be read: {detail}"),
            Self::ScanPath(detail) => write!(f, "the folder could not be read: {detail}"),
            Self::NoCatalogue(detail) => write!(f, "no usable catalogue: {detail}"),
            Self::NothingToAudit(detail) => write!(f, "{detail}"),
            Self::Cancelled => write!(f, "the audit was cancelled"),
        }
    }
}

impl std::error::Error for DatAuditError {}

/// Everything one audit run produced.
///
/// Provenance is part of the result, not something a caller has to remember:
/// the source ID, its display name, the catalogue path, and the catalogue
/// headers the run actually read are all carried here, so a report can say
/// which source produced it long after the page state has moved on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatAuditOutcome {
    pub source_id: String,
    pub source_display_name: String,
    pub dat_path: String,
    pub scan_root: String,
    /// The catalogue headers read, for provenance. One per DAT file that
    /// parsed.
    pub catalogue_names: Vec<String>,
    pub catalogue_entries: usize,
    pub catalogue_roms: usize,
    /// Orthogonal content classification. It never changes `report` or its
    /// counts; it controls only downstream selection eligibility.
    pub content: DatAuditContentOutcome,
    /// DAT files in a folder source that did not parse and so contributed
    /// nothing to the index.
    pub unreadable_catalogues: Vec<String>,
    pub report: AuditReport,
    /// Exact source observations for combined audits.  Ordinary one-source
    /// audits retain an empty list, preserving their existing model.
    #[serde(default)]
    pub evidence_sources: Vec<DatAuditEvidenceSource>,
    /// Archive-member evidence is deliberately separate from the flat
    /// physical-file report. In particular, rename planning consumes only
    /// `report` and cannot turn a member name into a filesystem rename.
    #[serde(default)]
    pub archives: Vec<DatArchiveAudit>,
    /// Stage 1 set-completeness resolutions derived from `archives`, bound to
    /// the exact `ParsedDat` instance this run indexed - see
    /// `dat::set`'s "Runtime DAT binding" doc. Also deliberately separate
    /// from `report`: never consumed by rename planning.
    #[serde(default)]
    pub sets: Vec<SetResolution>,
    pub unhashed: Vec<UnhashedFile>,
    pub files_scanned: usize,
    pub bytes_hashed: u64,
    /// Decoded archive-member bytes hashed in addition to `bytes_hashed`.
    #[serde(default)]
    pub archive_bytes_hashed: u64,
    /// The walk hit a ceiling, so this is part of the folder and not all of it.
    pub truncated: bool,
    /// The policy annotation, present only when the request supplied a policy.
    pub policy: Option<DatAuditPolicyOutcome>,
    /// The audited source's canonical platform id, when assigned and
    /// recognised. Provenance for consumers like the rename plan.
    pub platform: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatArchiveAudit {
    pub archive_path: PathBuf,
    /// Identity of the exact outer object whose completed member pass produced
    /// this evidence. Missing for failed/incomplete opens and legacy fixtures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outer_identity: Option<crate::dat::rename_apply::ObjectIdentity>,
    pub format: String,
    pub total_members: usize,
    pub completion: ArchivePassCompletion,
    pub members: Vec<DatArchiveMemberAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatArchiveMemberAudit {
    pub evidence: ArchiveMemberEvidence,
    /// DAT identity, when decoded bytes were hash-complete and the outer file
    /// remained the same object for the full pass. `None` is not ambiguity;
    /// the accompanying member status explains why matching was not attempted.
    pub verdict: Option<AuditVerdict>,
    /// Positional DAT candidates from the strongest matching cryptographic
    /// lookup. Empty means legacy evidence and retains the verdict-name
    /// fallback; filename-only evidence is never placed here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_refs: Vec<DatRomRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DatAuditContentOutcome {
    pub selection: ContentSelectionPolicy,
    pub catalogue: DatContentSummary,
    /// Classification for matched local files. Unmatched files remain in the
    /// ordinary audit report and are not assigned a fabricated content class.
    pub matches: Vec<DatContentMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatContentMatch {
    pub local_path: String,
    pub candidates: Vec<DatContentCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatContentCandidate {
    pub game_name: String,
    pub rom_name: String,
    pub classification: DatContentClassification,
    pub eligibility: ContentEligibility,
    pub original_metadata: DatOriginalMetadata,
}

impl DatAuditOutcome {
    /// One line describing what was compared, for a status row.
    pub fn headline(&self) -> String {
        format!(
            "{} files compared against {} catalogue entries from '{}'",
            self.report.summary.total, self.catalogue_entries, self.source_display_name
        )
    }
}

/// The policy annotation an audit carries when a policy was supplied.
///
/// This never changes a verdict. Every verdict the core produced stands as it
/// is; the annotation only adds, for each file whose hash matched several
/// catalogue entries, the user's *preference order* over those already-valid
/// candidates, plus the consultation order of the sources involved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatAuditPolicyOutcome {
    /// The sources consulted for this platform, in order. For a single-source
    /// audit this is just that source and its peers for the same platform.
    pub source_ordering: Vec<String>,
    /// One note per file with a multi-candidate verdict that was ranked.
    pub notes: Vec<DatPolicyNote>,
}

/// One file's policy ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatPolicyNote {
    pub local_path: String,
    /// The verdict this note accompanies (`Exact (multiple)`, …).
    pub verdict_label: String,
    pub resolution: CandidateResolution,
}

/// Runs a read-only audit.
///
/// `trusted` is passed straight to the hashing policy: it decides whether a
/// symlinked ROM may be followed, exactly as it does everywhere else in the
/// build. Pass [`TrustedRoots::none`] to refuse every symlink.
///
/// `on_progress` runs on the calling thread between units of work.
pub fn run_dat_audit(
    request: &DatAuditRequest,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(DatAuditProgress),
) -> Result<DatAuditOutcome, DatAuditError> {
    if cancelled(cancel) {
        return Err(DatAuditError::Cancelled);
    }

    // ---- 1. Read the catalogue ------------------------------------------
    let dat_files = match request.dat_kind {
        DatSourceKind::File => {
            validation::validate_dat_path(&request.dat_path, DatSourceKind::File)
                .map_err(|refusal| DatAuditError::DatPath(refusal.detail()))?;
            vec![request.dat_path.clone()]
        }
        DatSourceKind::Folder => {
            validation::discover_dat_files(&request.dat_path)
                .map_err(|refusal| DatAuditError::DatPath(refusal.detail()))?
                .files
        }
    };

    if dat_files.is_empty() {
        return Err(DatAuditError::NoCatalogue(
            "the source contains no DAT files".to_string(),
        ));
    }

    let mut catalogue_names = Vec::new();
    let mut unreadable_catalogues = Vec::new();
    let mut merged: Option<ParsedDat> = None;

    for path in &dat_files {
        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
        let file_name = file_name_of(path);
        on_progress(DatAuditProgress::ReadingCatalogue {
            file_name: file_name.clone(),
        });
        match parse_dat_file(path, request.limits) {
            Ok(parsed) => {
                catalogue_names.push(
                    parsed
                        .dat
                        .source
                        .name
                        .clone()
                        .unwrap_or_else(|| file_name.clone()),
                );
                match merged.as_mut() {
                    // Several DAT files in one folder source become one index:
                    // the user registered the folder as a single source, so a
                    // file matching any catalogue in it is a match for the
                    // source. Collisions between them are not hidden - the
                    // index keeps every candidate, and the audit reports the
                    // multiple-candidate verdicts it already has for that.
                    Some(target) => {
                        target.games.extend(parsed.dat.games);
                        target.source.entry_count = target
                            .source
                            .entry_count
                            .saturating_add(parsed.dat.source.entry_count);
                        target.source.rom_count = target
                            .source
                            .rom_count
                            .saturating_add(parsed.dat.source.rom_count);
                    }
                    None => merged = Some(parsed.dat),
                }
            }
            Err(error) => {
                unreadable_catalogues.push(format!("{file_name}: {error}"));
            }
        }
    }

    let Some(mut catalogue) = merged else {
        return Err(DatAuditError::NoCatalogue(unreadable_catalogues.join("; ")));
    };

    let index = DatIndex::build(&catalogue);
    let disk_index = DatDiskIndex::build(&catalogue);
    let catalogue_entries = catalogue.source.entry_count;
    let catalogue_roms = catalogue.source.rom_count;
    let content_selection = request
        .policy
        .as_ref()
        .map(|policy| policy.content_selection)
        .unwrap_or(ContentSelectionPolicy::AllEntries);
    let catalogue_content = summarize(&catalogue.games, content_selection);
    // Always retained now, not just when a policy is supplied: this is the
    // exact parsed instance `index` (above) was built from, and
    // `audit_archives` below binds `dat::set`'s completeness classification
    // to this same instance rather than letting anything reparse the DAT
    // file independently - see `dat::set`'s "Runtime DAT binding" doc. The
    // policy-ranking path further down reads the identical `Vec`, not a
    // second copy.
    let catalogue_games = std::mem::take(&mut catalogue.games);
    drop(catalogue);

    on_progress(DatAuditProgress::CatalogueReady {
        entries: catalogue_entries,
        roms: catalogue_roms,
    });

    // ---- 2-3. Walk and hash once ----------------------------------------
    let hashed =
        collect_loose_file_evidence(&request.scan_root, trusted, cancel, on_progress, true)?;
    let scan = hashed.scan;
    let known = hashed.known;
    let unhashed = hashed.unhashed;
    let bytes_hashed = hashed.bytes_hashed;

    // ---- 3.5 CHD disk evidence --------------------------------------------
    // Deliberately not folded into step 3's loop: a CHD's DAT identity is its
    // header's `overall_sha1` field, not a hash of the `.chd` file's own
    // bytes, so this reads a bounded header instead of hashing the file. See
    // `dat::disk_audit`'s module doc for why this never touches `DatIndex`
    // (the ROM hash index) or `KnownFileEvidence`/`audit_one`.
    let mut disk_evidence: Vec<DatDiskAudit> = Vec::new();
    for path in scan.files.iter().filter(|path| is_chd_path(path)) {
        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
        disk_evidence.push(audit_chd_disk(path, trusted, &disk_index));
    }
    // Mirrors `ArchivePassCompletion`: either an intentional ceiling
    // (`truncated`) or a silent traversal error (`!scan_complete` - an
    // unreadable directory, a directory-entry read failure, or a
    // `file_type()` failure) means some required disk's true presence is
    // unknown, so a set that actually declares one cannot be safely called
    // `Complete` (R8 for disks, scoped per-set - see `dat::set`'s "R9" doc).
    let disk_scan_complete = !scan.truncated && scan.scan_complete;

    // ---- 4. Compare ------------------------------------------------------
    if cancelled(cancel) {
        return Err(DatAuditError::Cancelled);
    }
    on_progress(DatAuditProgress::Comparing { files: known.len() });
    let report = audit_files(&known, &index);
    let (archives, archive_bytes_hashed, mut sets) = audit_archives(
        &scan.files,
        trusted,
        cancel,
        &index,
        &disk_evidence,
        disk_scan_complete,
        &catalogue_games,
        &request.source_id,
    )?;

    // ---- 4b. Resolve dependencies (Stage 2d) -----------------------------
    // Runs only now, because a dependency is satisfied by evidence that may
    // live in a completely different archive than the set it belongs to -
    // a clone's borrowed ROM sits in the parent's archive, a device's ROMs in
    // the device's, a delta CHD's parent in any `.chd` under the scan root.
    // Resolving during the archive walk would mean answering "is the provider
    // present?" before the provider had been looked at.
    //
    // `collection_scan_complete` is the switch that stops this stage
    // asserting an absence it could not have observed: any ceiling, traversal
    // error, or unfinished archive pass turns every "not found anywhere"
    // conclusion into `EvidenceUnavailable` instead of `Missing`. Positive
    // verifications are unaffected, so this can only ever weaken a negative.
    let collection_scan_complete = disk_scan_complete
        && archives
            .iter()
            .all(|archive| matches!(archive.completion, ArchivePassCompletion::Complete));
    let dependency_evidence = CollectionEvidence::build(
        &archives,
        &disk_evidence,
        &catalogue_games,
        collection_scan_complete,
    );
    // Downgrade-only: this can preserve or weaken a set's state, never
    // promote one. See `dat::dependency::apply_dependency_state`.
    resolve_collection(&mut sets, &catalogue_games, &dependency_evidence);

    let content_matches = annotate_content_matches(&report, &known, &index, content_selection);

    // ---- 5. Annotate multi-candidate verdicts with the policy -------------
    // The policy only *ranks already valid candidates*: the audit's verdicts
    // are untouched, and a preference note is added exactly for the files
    // whose cryptographic hash matched several catalogue entries.
    let policy = request.policy.as_ref().map(|policy| {
        annotate_with_policy(
            &report,
            &known,
            &index,
            &catalogue_games,
            policy,
            &request.source_id,
        )
    });

    Ok(DatAuditOutcome {
        source_id: request.source_id.clone(),
        source_display_name: request.source_display_name.clone(),
        dat_path: request.dat_path.to_string_lossy().into_owned(),
        scan_root: request.scan_root.to_string_lossy().into_owned(),
        catalogue_names,
        catalogue_entries,
        catalogue_roms,
        content: DatAuditContentOutcome {
            selection: content_selection,
            catalogue: catalogue_content,
            matches: content_matches,
        },
        unreadable_catalogues,
        files_scanned: scan.files.len(),
        truncated: scan.truncated,
        report,
        evidence_sources: Vec::new(),
        archives,
        sets,
        unhashed,
        bytes_hashed,
        archive_bytes_hashed,
        policy,
        platform: request.platform.clone(),
    })
}

/// The one shared local scan/hash pass used by both one-catalogue and
/// combined-catalogue audits. It deliberately excludes CHD and RAR outer
/// bytes from loose-ROM evidence for the same reasons documented in
/// [`run_dat_audit`]: those formats have their own bounded evidence paths and
/// must not gain a coincidental raw-container match. Combined audits also
/// keep ZIP/7z outer bytes out until their member/set rules can merge several
/// catalogues safely; each deferred file remains visible as an unhashed,
/// non-actionable report row rather than disappearing from the result.
struct HashedLocalScan {
    scan: LocalScan,
    known: Vec<KnownFileEvidence>,
    unhashed: Vec<UnhashedFile>,
    bytes_hashed: u64,
}

fn collect_loose_file_evidence(
    scan_root: &Path,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(DatAuditProgress),
    include_archive_outers: bool,
) -> Result<HashedLocalScan, DatAuditError> {
    if cancelled(cancel) {
        return Err(DatAuditError::Cancelled);
    }
    let scan = scan_local_files(scan_root, cancel, on_progress)?;
    if scan.files.is_empty() {
        return Err(DatAuditError::NothingToAudit(format!(
            "no files were found in {}",
            scan_root.display()
        )));
    }

    let total = scan.files.len();
    let mut known = Vec::with_capacity(total);
    let mut unhashed = Vec::new();
    let mut bytes_hashed = 0_u64;
    for (position, path) in scan.files.iter().enumerate() {
        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
        let defer_container = is_chd_path(path)
            || is_rar_path(path)
            || (!include_archive_outers && (is_zip_path(path) || is_sevenz_path(path)));
        if defer_container {
            let file_name = file_name_of(path);
            if !include_archive_outers {
                unhashed.push(UnhashedFile {
                    path: path.to_string_lossy().into_owned(),
                    file_name: file_name.clone(),
                    code: "combined-container-deferred".to_string(),
                    detail: "Combined evidence does not yet merge this container's specialised identity path; it was left non-actionable."
                        .to_string(),
                });
                // Retain a physical-file row. With no digest it can reach at
                // most FilenameOnly and the rename planner never promotes
                // that weak result to an action.
                known.push(KnownFileEvidence::new(
                    path.to_string_lossy().into_owned(),
                    file_name,
                ));
            }
            continue;
        }
        let file_name = file_name_of(path);
        on_progress(DatAuditProgress::Hashing {
            index: position + 1,
            total,
            file_name: file_name.clone(),
        });
        let evidence = KnownFileEvidence::new(path.to_string_lossy().into_owned(), &file_name);
        match hash_file_reporting(path, trusted, Some(cancel), &|_| {}) {
            Ok(hashes) => {
                bytes_hashed = bytes_hashed.saturating_add(hashes.bytes_hashed);
                known.push(
                    evidence
                        .with_size(hashes.fingerprint.size_bytes)
                        .with_crc32(hashes.crc32)
                        .with_md5(hashes.md5)
                        .with_sha1(hashes.sha1),
                );
            }
            Err(HashRefusal::Cancelled) => return Err(DatAuditError::Cancelled),
            Err(refusal) => {
                unhashed.push(UnhashedFile {
                    path: path.to_string_lossy().into_owned(),
                    file_name,
                    code: refusal.code().to_string(),
                    detail: refusal.detail(),
                });
                known.push(evidence);
            }
        }
    }
    Ok(HashedLocalScan {
        scan,
        known,
        unhashed,
        bytes_hashed,
    })
}

/// Runs one bounded scan/hash pass and compares each resulting evidence object
/// with every supplied catalogue index.  It is intentionally a loose-file
/// first slice: CHD and archive-set identity stay on their existing dedicated
/// one-catalogue paths until their multi-catalogue merge rules are equally
/// explicit.  They are never promoted through a raw-container hash here.
pub fn run_combined_dat_audit(
    request: &CombinedDatAuditRequest,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(DatAuditProgress),
) -> Result<DatAuditOutcome, DatAuditError> {
    if request.sources.is_empty() {
        return Err(DatAuditError::NoCatalogue(
            "no enabled, installed DAT catalogues are available".to_string(),
        ));
    }

    let mut loaded = Vec::new();
    let mut unreadable_catalogues = Vec::new();
    let mut catalogue_names = Vec::new();
    let mut catalogue_entries = 0_usize;
    let mut catalogue_roms = 0_usize;
    for source in &request.sources {
        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
        match load_combined_catalogue(source, request.limits, on_progress) {
            Ok(catalogue) => {
                catalogue_entries = catalogue_entries.saturating_add(catalogue.entries);
                catalogue_roms = catalogue_roms.saturating_add(catalogue.roms);
                catalogue_names.extend(catalogue.names.iter().cloned());
                loaded.push(catalogue);
            }
            Err(detail) => {
                unreadable_catalogues.push(format!("{}: {detail}", source.source_display_name))
            }
        }
    }
    if loaded.is_empty() {
        return Err(DatAuditError::NoCatalogue(unreadable_catalogues.join("; ")));
    }

    on_progress(DatAuditProgress::CatalogueReady {
        entries: catalogue_entries,
        roms: catalogue_roms,
    });
    let hashed =
        collect_loose_file_evidence(&request.scan_root, trusted, cancel, on_progress, false)?;
    if cancelled(cancel) {
        return Err(DatAuditError::Cancelled);
    }
    on_progress(DatAuditProgress::Comparing {
        files: hashed.known.len(),
    });

    let mut entries = Vec::with_capacity(hashed.known.len());
    let mut evidence_sources = Vec::new();
    let mut content_matches = Vec::new();
    for known in &hashed.known {
        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
        let combined = merge_combined_evidence(known, &loaded);
        if let Some(content) = combined.content {
            content_matches.push(content);
        }
        evidence_sources.extend(combined.evidence);
        entries.push(AuditEntry {
            local_path: known.filepath.clone(),
            local_filename: std::path::Path::new(&known.filepath)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| known.filename.clone()),
            verdict: combined.verdict,
        });
    }
    let report = AuditReport {
        summary: combined_summary(&entries),
        entries,
    };

    Ok(DatAuditOutcome {
        source_id: "combined-enabled-dat-sources".to_string(),
        source_display_name: "All enabled evidence catalogues".to_string(),
        dat_path: "multiple local and managed DAT catalogues".to_string(),
        scan_root: request.scan_root.to_string_lossy().into_owned(),
        catalogue_names,
        catalogue_entries,
        catalogue_roms,
        content: DatAuditContentOutcome {
            selection: ContentSelectionPolicy::AllEntries,
            catalogue: DatContentSummary::default(),
            matches: content_matches,
        },
        unreadable_catalogues,
        report,
        evidence_sources,
        archives: Vec::new(),
        sets: Vec::new(),
        unhashed: hashed.unhashed,
        files_scanned: hashed.scan.files.len(),
        bytes_hashed: hashed.bytes_hashed,
        archive_bytes_hashed: 0,
        truncated: hashed.scan.truncated,
        policy: None,
        platform: None,
    })
}

struct LoadedCombinedCatalogue {
    source: CombinedDatAuditSource,
    index: DatIndex,
    names: Vec<String>,
    entries: usize,
    roms: usize,
}

fn load_combined_catalogue(
    source: &CombinedDatAuditSource,
    limits: DatLimits,
    on_progress: &dyn Fn(DatAuditProgress),
) -> Result<LoadedCombinedCatalogue, String> {
    let dat_files = match source.dat_kind {
        DatSourceKind::File => {
            validation::validate_dat_path(&source.dat_path, DatSourceKind::File)
                .map_err(|refusal| refusal.detail())?;
            vec![source.dat_path.clone()]
        }
        DatSourceKind::Folder => {
            validation::discover_dat_files(&source.dat_path)
                .map_err(|refusal| refusal.detail())?
                .files
        }
    };
    if dat_files.is_empty() {
        return Err("contains no DAT files".to_string());
    }
    let mut names = Vec::new();
    let mut merged: Option<ParsedDat> = None;
    let mut failures = Vec::new();
    for path in dat_files {
        let file_name = file_name_of(&path);
        on_progress(DatAuditProgress::ReadingCatalogue {
            file_name: file_name.clone(),
        });
        match parse_dat_file(&path, limits) {
            Ok(parsed) => {
                names.push(parsed.dat.source.name.clone().unwrap_or(file_name));
                match merged.as_mut() {
                    Some(existing) => {
                        existing.games.extend(parsed.dat.games);
                        existing.source.entry_count = existing
                            .source
                            .entry_count
                            .saturating_add(parsed.dat.source.entry_count);
                        existing.source.rom_count = existing
                            .source
                            .rom_count
                            .saturating_add(parsed.dat.source.rom_count);
                    }
                    None => merged = Some(parsed.dat),
                }
            }
            Err(error) => failures.push(format!("{file_name}: {error}")),
        }
    }
    let Some(parsed) = merged else {
        return Err(failures.join("; "));
    };
    Ok(LoadedCombinedCatalogue {
        source: source.clone(),
        entries: parsed.source.entry_count,
        roms: parsed.source.rom_count,
        index: DatIndex::build(&parsed),
        names,
    })
}

struct CombinedEvidenceResult {
    verdict: AuditVerdict,
    evidence: Vec<DatAuditEvidenceSource>,
    content: Option<DatContentMatch>,
}

fn merge_combined_evidence(
    known: &KnownFileEvidence,
    catalogues: &[LoadedCombinedCatalogue],
) -> CombinedEvidenceResult {
    let mut exact = Vec::new();
    let mut source_ambiguities = Vec::new();
    let mut non_exact = Vec::new();
    for catalogue in catalogues {
        match audit_one(known, &catalogue.index) {
            AuditVerdict::Exact {
                game_name,
                rom_name,
                algorithm,
            } => exact.push((catalogue, game_name, rom_name, algorithm)),
            AuditVerdict::ExactMultipleCandidates { count, .. } => {
                source_ambiguities.push(format!(
                    "{} has {count} exact candidates",
                    catalogue.source.source_display_name
                ))
            }
            verdict => non_exact.push(verdict),
        }
    }
    if !source_ambiguities.is_empty() {
        return CombinedEvidenceResult {
            verdict: AuditVerdict::Ambiguous {
                detail: source_ambiguities.join("; "),
            },
            evidence: Vec::new(),
            content: None,
        };
    }
    let Some((first_source, first_game, first_rom, first_algorithm)) = exact.first() else {
        // Keep useful non-exact diagnostics, but never allow a filename-only
        // result from one catalogue to eclipse hash evidence from another.
        let verdict = non_exact
            .iter()
            .find(|verdict| matches!(verdict, AuditVerdict::Ambiguous { .. }))
            .cloned()
            .or_else(|| {
                non_exact
                    .iter()
                    .find(|verdict| matches!(verdict, AuditVerdict::Probable { .. }))
                    .cloned()
            })
            .or_else(|| {
                non_exact
                    .iter()
                    .find(|verdict| {
                        matches!(verdict, AuditVerdict::ProbableMultipleCandidates { .. })
                    })
                    .cloned()
            })
            .or_else(|| {
                non_exact
                    .iter()
                    .find(|verdict| matches!(verdict, AuditVerdict::NotInDat))
                    .cloned()
            })
            .unwrap_or(AuditVerdict::NoUsableEvidence);
        return CombinedEvidenceResult {
            verdict,
            evidence: Vec::new(),
            content: None,
        };
    };
    let same_identity = exact
        .iter()
        .all(|(_, game, rom, _)| game == first_game && rom == first_rom);
    let known_platform = exact
        .iter()
        .filter_map(|(source, _, _, _)| source.source.platform.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    if !same_identity || known_platform.len() > 1 {
        let details = exact
            .iter()
            .map(|(source, game, rom, _)| {
                let platform = source
                    .source
                    .platform
                    .as_deref()
                    .unwrap_or("platform not assigned");
                format!(
                    "{}: {game} / {rom} ({platform})",
                    source.source.source_display_name
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return CombinedEvidenceResult {
            verdict: AuditVerdict::Ambiguous {
                detail: format!("exact catalogues disagree: {details}"),
            },
            evidence: Vec::new(),
            content: None,
        };
    }
    let evidence = exact
        .iter()
        .map(|(source, game, rom, algorithm)| DatAuditEvidenceSource {
            local_path: known.filepath.clone(),
            source_id: source.source.source_id.clone(),
            source_display_name: source.source.source_display_name.clone(),
            platform: source.source.platform.clone(),
            game_name: game.clone(),
            rom_name: rom.clone(),
            algorithm: (*algorithm).to_string(),
        })
        .collect();
    let content = combined_content_match(known, first_source, first_game, first_rom);
    CombinedEvidenceResult {
        verdict: AuditVerdict::Exact {
            game_name: first_game.clone(),
            rom_name: first_rom.clone(),
            algorithm: first_algorithm,
        },
        evidence,
        content,
    }
}

fn combined_content_match(
    known: &KnownFileEvidence,
    catalogue: &LoadedCombinedCatalogue,
    game_name: &str,
    rom_name: &str,
) -> Option<DatContentMatch> {
    let candidate = catalogue
        .index
        .lookup_filename(rom_name)
        .iter()
        .find(|candidate| candidate.game_name == game_name && candidate.rom_name == rom_name)?;
    Some(DatContentMatch {
        local_path: known.filepath.clone(),
        candidates: vec![DatContentCandidate {
            game_name: candidate.game_name.clone(),
            rom_name: candidate.rom_name.clone(),
            classification: candidate.content_classification.clone(),
            eligibility: ContentSelectionPolicy::AllEntries
                .eligibility(&candidate.content_classification),
            original_metadata: candidate.original_metadata.clone(),
        }],
    })
}

fn combined_summary(entries: &[AuditEntry]) -> AuditSummary {
    let mut summary = AuditSummary {
        total: entries.len(),
        ..AuditSummary::default()
    };
    for entry in entries {
        match entry.verdict {
            AuditVerdict::Exact { .. } => summary.exact += 1,
            AuditVerdict::ExactMultipleCandidates { .. } => summary.exact_multiple += 1,
            AuditVerdict::Probable { .. } => summary.probable += 1,
            AuditVerdict::ProbableMultipleCandidates { .. } => summary.probable_multiple += 1,
            AuditVerdict::FilenameOnly { .. } => summary.filename_only += 1,
            AuditVerdict::Ambiguous { .. } => summary.ambiguous += 1,
            AuditVerdict::NotInDat => summary.not_in_dat += 1,
            AuditVerdict::NoUsableEvidence => summary.no_evidence += 1,
        }
    }
    summary
}

/// How long one RAR-backend capability probe (`RarProvider::discover`) may
/// take. Run at most once per [`audit_archives`] call - see its "RAR
/// backend discovery" note.
const RAR_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one RAR archive-listing/relisting child may take.
const RAR_OPEN_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one RAR member-extraction child may take.
const RAR_MEMBER_TIMEOUT: Duration = Duration::from_secs(120);

/// Opens the right [`ArchiveMemberSource`] for `path`'s extension.
///
/// Returned as `Box<dyn ArchiveMemberSource>` precisely so the caller below
/// does not need to know which format it is holding: the trait is
/// object-safe for exactly this reason (see its doc). Dispatch is by
/// extension only - this never sniffs file contents to pick a format.
///
/// `rar_provider` is `None` only when this path is not a `.rar` (the RAR
/// branch below is the only one that reads it); when it *is* a `.rar` and
/// discovery already ran for this audit but found no capable backend, the
/// discovery failure itself (never a panic, never a silent skip) is what is
/// returned as [`ArchiveMemberSourceError::Unsupported`].
#[allow(clippy::too_many_arguments)]
fn open_archive_source(
    path: &Path,
    trusted: &TrustedRoots,
    limits: ArchiveLimits,
    cancel: &AtomicBool,
    index: &DatIndex,
    rar_provider: Option<&Result<RarProvider, RarError>>,
) -> Result<Box<dyn ArchiveMemberSource>, ArchiveMemberSourceError> {
    if is_zip_path(path) {
        ZipArchiveSource::open(path, trusted, limits, cancel)
            .map(|source| Box::new(source) as Box<dyn ArchiveMemberSource>)
    } else if is_sevenz_path(path) {
        SevenZArchiveSource::open(path, trusted, limits, cancel)
            .map(|source| Box::new(source) as Box<dyn ArchiveMemberSource>)
    } else if is_rar_path(path) {
        let provider = match rar_provider {
            Some(Ok(provider)) => provider,
            Some(Err(error)) => {
                return Err(ArchiveMemberSourceError::Unsupported {
                    detail: format!("no capable RAR backend is available: {error}"),
                });
            }
            None => {
                return Err(ArchiveMemberSourceError::Unsupported {
                    detail: "RAR backend was not probed for this run".to_string(),
                });
            }
        };
        RarArchiveSource::open(
            path,
            provider,
            index,
            limits,
            RAR_OPEN_TIMEOUT,
            RAR_MEMBER_TIMEOUT,
        )
        .map(|source| Box::new(source) as Box<dyn ArchiveMemberSource>)
    } else {
        Err(ArchiveMemberSourceError::Unsupported {
            detail: "unrecognised archive extension".to_string(),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn audit_archives(
    files: &[PathBuf],
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    index: &DatIndex,
    disk_evidence: &[DatDiskAudit],
    disk_scan_complete: bool,
    games: &[DatGameEntry],
    source_id: &str,
) -> Result<(Vec<DatArchiveAudit>, u64, Vec<SetResolution>), DatAuditError> {
    let mut archives = Vec::new();
    let mut bytes_hashed = 0_u64;
    let mut sets = Vec::new();
    let mut run_budget = ArchiveRunBudget::new(MAX_ARCHIVE_RUN_LOGICAL_BYTES);
    // RAR backend discovery (a real child-process spawn + capability parse,
    // unlike ZIP/7z which never need external-binary discovery at all) runs
    // at most once per audit run, lazily, and only if a `.rar` is actually
    // present - never probed when a library has none, so RAR support being
    // absent costs nothing when it is never used.
    let mut rar_provider: Option<Result<RarProvider, RarError>> = None;

    for path in files
        .iter()
        .filter(|path| is_zip_path(path) || is_sevenz_path(path) || is_rar_path(path))
    {
        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
        // The format label for a source-open failure is inferred from the
        // extension alone, since no `ArchiveMemberSource` exists yet to ask.
        let format_guess = if is_zip_path(path) {
            "zip"
        } else if is_sevenz_path(path) {
            "7z"
        } else {
            "rar"
        };
        if is_rar_path(path) && rar_provider.is_none() {
            rar_provider = Some(RarProvider::discover(RAR_DISCOVERY_TIMEOUT));
        }
        let identity_before = crate::dat::rename_apply::capture_identity(path).ok();
        let mut source = match open_archive_source(
            path,
            trusted,
            ArchiveLimits::default(),
            cancel,
            index,
            rar_provider.as_ref(),
        ) {
            Ok(source) => source,
            Err(ArchiveMemberSourceError::Cancelled) => return Err(DatAuditError::Cancelled),
            Err(error) => {
                archives.push(DatArchiveAudit {
                    archive_path: path.clone(),
                    outer_identity: None,
                    format: format_guess.to_string(),
                    total_members: 0,
                    completion: ArchivePassCompletion::Incomplete {
                        reason: ArchivePassStopReason::SourceError {
                            detail: format!("{error:?}"),
                        },
                    },
                    members: Vec::new(),
                });
                continue;
            }
        };

        let mut pass = source.verify_all(cancel, &mut run_budget);
        let identity_after = crate::dat::rename_apply::capture_identity(path).ok();
        let stable_outer_identity = identity_before.filter(|before| {
            identity_after
                .as_ref()
                .is_some_and(|after| crate::dat::rename_apply::identity_matches(before, after))
        });
        if stable_outer_identity.is_none() {
            pass.completion = ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::OuterFileChanged,
            };
        }
        let outer_changed = matches!(
            pass.completion,
            ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::OuterFileChanged
            }
        );
        let members = pass
            .members
            .into_iter()
            .map(|evidence| {
                // A hash-complete member without hashes cannot be matched and
                // must not panic the audit: the source contract is checked
                // here rather than asserted, so a future format implementation
                // that breaks it degrades to "not matched", never to a crash.
                let (verdict, matched_refs) =
                    if let (false, ArchiveMemberStatus::HashComplete, Some(hashes)) =
                        (outer_changed, &evidence.status, evidence.hashes.as_ref())
                    {
                        bytes_hashed = bytes_hashed.saturating_add(evidence.logical_size);
                        let known = KnownFileEvidence::new(
                            format!("{}::#{}", path.display(), evidence.index),
                            &evidence.member_name_display,
                        )
                        .with_size(evidence.logical_size)
                        .with_crc32(&hashes.crc32)
                        .with_md5(&hashes.md5)
                        .with_sha1(&hashes.sha1)
                        .with_sha256(&hashes.sha256);
                        let verdict = audit_one(&known, index);
                        let matched_refs = matched_refs_for_verdict(&verdict, &known, index);
                        (Some(verdict), matched_refs)
                    } else {
                        (None, Vec::new())
                    };
                DatArchiveMemberAudit {
                    evidence,
                    verdict,
                    matched_refs,
                }
            })
            .collect();
        let archive_audit = DatArchiveAudit {
            archive_path: path.clone(),
            outer_identity: stable_outer_identity,
            format: source.archive_format().to_string(),
            total_members: pass.total_members,
            completion: pass.completion,
            members,
        };
        // `games` is the exact parsed instance `index` (above) was built
        // from - see dat::set's "Runtime DAT binding" doc for why this must
        // never be an independently re-parsed slice.
        sets.extend(classify_archive_sets(
            &archive_audit,
            disk_evidence,
            disk_scan_complete,
            games,
            source_id,
        ));
        archives.push(archive_audit);

        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
    }
    Ok((archives, bytes_hashed, sets))
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

fn is_sevenz_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("7z"))
}

fn is_rar_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rar"))
}

fn annotate_content_matches(
    report: &AuditReport,
    known: &[KnownFileEvidence],
    index: &DatIndex,
    selection: ContentSelectionPolicy,
) -> Vec<DatContentMatch> {
    report
        .entries
        .iter()
        .zip(known.iter())
        .filter_map(|(entry, evidence)| {
            let refs: Vec<DatRomRef> = match &entry.verdict {
                AuditVerdict::Exact { .. } | AuditVerdict::ExactMultipleCandidates { .. } => {
                    verified_candidate_refs(evidence, index)
                }
                AuditVerdict::Probable { .. } | AuditVerdict::ProbableMultipleCandidates { .. } => {
                    evidence
                        .crc32
                        .as_deref()
                        .map(|crc| {
                            index
                                .lookup_crc32(crc)
                                .iter()
                                .filter(|candidate| {
                                    evidence
                                        .size_bytes
                                        .is_none_or(|size| candidate.size_bytes == Some(size))
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default()
                }
                AuditVerdict::FilenameOnly { .. } => {
                    index.lookup_filename(&evidence.filename).to_vec()
                }
                AuditVerdict::Ambiguous { .. }
                | AuditVerdict::NotInDat
                | AuditVerdict::NoUsableEvidence => Vec::new(),
            };
            (!refs.is_empty()).then(|| DatContentMatch {
                local_path: entry.local_path.clone(),
                candidates: refs
                    .into_iter()
                    .map(|candidate| DatContentCandidate {
                        game_name: candidate.game_name.clone(),
                        rom_name: candidate.rom_name.clone(),
                        eligibility: selection.eligibility(&candidate.content_classification),
                        classification: candidate.content_classification.clone(),
                        original_metadata: candidate.original_metadata.clone(),
                    })
                    .collect(),
            })
        })
        .collect()
}

/// Builds the policy annotation for an audit.
///
/// For every entry whose verdict is `ExactMultipleCandidates`, the matching
/// catalogue ROMs are turned into policy candidates and ranked. The verdict
/// itself is never replaced - `Exact (multiple)` still says what it says - the
/// note just shows the user's preferred order over that already-valid set.
fn annotate_with_policy(
    report: &AuditReport,
    known: &[KnownFileEvidence],
    index: &DatIndex,
    games: &[crate::dat::model::DatGameEntry],
    policy: &EffectiveDatPolicy,
    audited_source_id: &str,
) -> DatAuditPolicyOutcome {
    let source_ordering: Vec<String> = policy
        .source_ordering
        .iter()
        .map(|source| source.display_name.clone())
        .collect();

    // All candidates come from the audited source's own catalogue. Its id and
    // priority are what the ranking attributes them to, so the candidate
    // participates (its source must be in the ordering) and any priority
    // explanation is honest.
    let audited_source = policy
        .source_ordering
        .iter()
        .find(|source| source.id == audited_source_id);

    let mut notes = Vec::new();
    for (entry, evidence) in report.entries.iter().zip(known.iter()) {
        if !matches!(entry.verdict, AuditVerdict::ExactMultipleCandidates { .. }) {
            continue;
        }
        let refs = verified_candidate_refs(evidence, index);
        if refs.len() < 2 {
            continue;
        }
        let candidates: Vec<crate::dat::policy::DatCandidate> = refs
            .iter()
            .filter_map(|rom_ref| {
                let key = rom_ref.key();
                let game = games.get(key.game_index)?;
                let rom = rom_for_key(games, key)?;
                Some(candidate_for_rom(
                    game,
                    rom,
                    audited_source_id,
                    audited_source.map(|source| source.priority).unwrap_or(0),
                ))
            })
            .collect();
        if candidates.len() < 2 {
            continue;
        }
        let resolution = rank_candidates(candidates, policy);
        notes.push(DatPolicyNote {
            local_path: entry.local_path.clone(),
            verdict_label: entry.verdict.label().to_string(),
            resolution,
        });
    }

    DatAuditPolicyOutcome {
        source_ordering,
        notes,
    }
}

fn rom_for_key(
    games: &[DatGameEntry],
    key: DatMemberKey,
) -> Option<&crate::dat::model::DatRomEntry> {
    let game = games.get(key.game_index)?;
    match key.location {
        MemberLocation::TopLevel { rom_index } => game.roms.get(rom_index),
        MemberLocation::DataArea {
            part_index,
            data_area_index,
            member_index,
        } => game
            .parts
            .get(part_index)?
            .data_areas
            .get(data_area_index)?
            .roms
            .get(member_index),
    }
}

/// The candidate catalogue ROMs a cryptographic hash matched, strongest hash
/// first, mirroring [`crate::dat::audit`]'s evidence priority.
///
/// This is deliberately the same algorithm the verdict uses: a file whose
/// SHA-1 matched is ranked by the same SHA-1 candidates the audit reported,
/// so the annotation can never disagree with the verdict about what matched.
fn verified_candidate_refs(known: &KnownFileEvidence, index: &DatIndex) -> Vec<DatRomRef> {
    for value in [
        known.sha256.as_deref(),
        known.sha1.as_deref(),
        known.md5.as_deref(),
    ] {
        let Some(value) = value else { continue };
        let candidates = match value.len() {
            64 => index.lookup_sha256(value),
            40 => index.lookup_sha1(value),
            32 => index.lookup_md5(value),
            _ => continue,
        };
        if !candidates.is_empty() {
            return candidates.to_vec();
        }
    }
    Vec::new()
}

fn matched_refs_for_verdict(
    verdict: &AuditVerdict,
    known: &KnownFileEvidence,
    index: &DatIndex,
) -> Vec<DatRomRef> {
    match verdict {
        AuditVerdict::Exact { .. } | AuditVerdict::ExactMultipleCandidates { .. } => {
            verified_candidate_refs(known, index)
        }
        _ => Vec::new(),
    }
}

struct LocalScan {
    files: Vec<PathBuf>,
    /// The scan hit a configured ceiling ([`MAX_SCAN_DEPTH`],
    /// [`MAX_SCAN_FILES`], or [`MAX_SCAN_ENTRIES_EXAMINED`]) and stopped
    /// early by design.
    truncated: bool,
    /// The scan encountered a traversal error - an unreadable directory, a
    /// directory-entry read failure, or a `file_type()` failure - and
    /// silently skipped whatever that entry might have been, rather than
    /// hitting a ceiling on purpose. Deliberately a separate concept from
    /// `truncated`: a ceiling is an intentional, reported stopping point: an
    /// unreadable subtree is missing candidates nobody chose to skip, and a
    /// caller deciding whether it can trust "nothing required was missed"
    /// needs to tell the two apart.
    scan_complete: bool,
}

/// Walks a folder target, or accepts one regular-file target, collecting files
/// in a deterministic order.
///
/// Symlinked *directories* are not descended into: following one can produce a
/// cycle, and a folder that links elsewhere is asking the scan to leave the
/// tree the user chose. Symlinked *files* are collected and left to the read
/// policy, which is the one place in the build that decides whether a link may
/// be followed - duplicating that decision here would be a second, divergent
/// answer to the same question.
fn scan_local_files(
    root: &Path,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(DatAuditProgress),
) -> Result<LocalScan, DatAuditError> {
    // Production always uses the real filesystem; only tests inject a
    // failure, and only for one directory they name, to prove the
    // bookkeeping below without depending on chmod (root bypasses
    // permission checks, so a chmod-based test behaves differently under
    // CI-as-root) or an unreproducible TOCTOU race against the real OS.
    scan_local_files_impl(root, cancel, on_progress, &|_| false)
}

fn scan_local_files_impl(
    root: &Path,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(DatAuditProgress),
    inject_read_dir_failure: &dyn Fn(&Path) -> bool,
) -> Result<LocalScan, DatAuditError> {
    if !root.is_absolute() {
        return Err(DatAuditError::ScanPath(
            "the folder path is not absolute".to_string(),
        ));
    }
    let metadata = std::fs::symlink_metadata(root)
        .map_err(|error| DatAuditError::ScanPath(format!("{}: {error}", root.display())))?;
    if metadata.is_file() {
        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
        on_progress(DatAuditProgress::Scanning {
            files_found: 1,
            current_dir: root
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned()),
        });
        return Ok(LocalScan {
            files: vec![root.to_path_buf()],
            truncated: false,
            scan_complete: true,
        });
    }
    if !metadata.is_dir() {
        return Err(DatAuditError::ScanPath(format!(
            "{} is not a folder or regular file",
            root.display()
        )));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut truncated = false;
    let mut scan_complete = true;
    let mut examined = 0usize;
    // Breadth-first over an explicit queue rather than recursion, so depth is a
    // number this function controls instead of a property of the call stack.
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = std::collections::VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));

    while let Some((directory, depth)) = queue.pop_front() {
        if cancelled(cancel) {
            return Err(DatAuditError::Cancelled);
        }
        if inject_read_dir_failure(&directory) {
            scan_complete = false;
            continue;
        }
        let Ok(read_dir) = std::fs::read_dir(&directory) else {
            // An unreadable subdirectory is skipped, not fatal: one permission
            // problem deep in a library should not throw away the rest of the
            // audit. It does mean this walk cannot claim to have seen every
            // candidate, though - a required disk (or ROM) could be exactly
            // what sits behind the door that would not open.
            scan_complete = false;
            continue;
        };

        let mut children: Vec<PathBuf> = Vec::new();
        for entry in read_dir {
            let Ok(entry) = entry else {
                // A directory-entry read failure loses that one candidate
                // the same way an unreadable directory does.
                scan_complete = false;
                continue;
            };
            examined += 1;
            if examined > MAX_SCAN_ENTRIES_EXAMINED {
                truncated = true;
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                scan_complete = false;
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                if depth < MAX_SCAN_DEPTH {
                    children.push(path);
                } else {
                    truncated = true;
                }
            } else if file_type.is_file() || file_type.is_symlink() {
                if files.len() >= MAX_SCAN_FILES {
                    truncated = true;
                } else {
                    files.push(path);
                }
            }
        }

        // Sorted per directory so the walk order is stable across runs and
        // across filesystems; `read_dir` order is not defined.
        children.sort();
        for child in children {
            queue.push_back((child, depth + 1));
        }

        on_progress(DatAuditProgress::Scanning {
            files_found: files.len(),
            current_dir: Some(directory.to_string_lossy().into_owned()),
        });
        if truncated && files.len() >= MAX_SCAN_FILES {
            break;
        }
    }

    files.sort();
    Ok(LocalScan {
        files,
        truncated,
        scan_complete,
    })
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

#[cfg(test)]
mod nested_member_evidence_tests {
    use std::collections::HashMap;

    use super::*;
    use crate::dat::model::{ChecksumAlgorithm, DatChecksum};

    #[test]
    fn exact_crypto_verdict_preserves_nested_candidate_refs_but_filename_only_does_not() {
        let digest = "1111111111111111111111111111111111111111";
        let key = DatMemberKey {
            game_index: 0,
            location: MemberLocation::DataArea {
                part_index: 1,
                data_area_index: 2,
                member_index: 3,
            },
        };
        let candidate = DatRomRef {
            game_index: 0,
            game_name: "Software".to_string(),
            rom_index: 3,
            member_key: key,
            rom_name: "nested.bin".to_string(),
            size_bytes: Some(4),
            checksums: vec![DatChecksum::parse(ChecksumAlgorithm::Sha1, digest).unwrap()],
            status: None,
            merge: None,
            content_classification: Default::default(),
            original_metadata: Default::default(),
            clone_of: None,
        };
        let index = DatIndex {
            by_crc32: HashMap::new(),
            by_md5: HashMap::new(),
            by_sha1: HashMap::from([(digest.to_string(), vec![candidate])]),
            by_sha256: HashMap::new(),
            by_filename: HashMap::new(),
            game_clone_of: HashMap::new(),
        };
        let known = KnownFileEvidence::new("archive.zip::#0", "nested.bin")
            .with_size(4)
            .with_sha1(digest);
        let exact = AuditVerdict::Exact {
            game_name: "Software".to_string(),
            rom_name: "nested.bin".to_string(),
            algorithm: "SHA-1",
        };

        let matched = matched_refs_for_verdict(&exact, &known, &index);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].key(), key);

        let filename_only = AuditVerdict::FilenameOnly {
            game_name: "Software".to_string(),
            rom_name: "nested.bin".to_string(),
        };
        assert!(matched_refs_for_verdict(&filename_only, &known, &index).is_empty());
    }
}

#[cfg(test)]
mod local_scan_traversal_tests {
    use super::*;

    fn no_progress(_: DatAuditProgress) {}

    /// A deterministic, portable, non-chmod stand-in for a real traversal
    /// error (an unreadable directory, a directory-entry read failure, a
    /// `file_type()` failure): `chmod`-based fixtures behave differently
    /// under CI running as root (root bypasses the permission check the test
    /// depends on), and racing a real `read_dir` failure against the OS is
    /// not reproducible. `scan_local_files_impl`'s injection hook exercises
    /// exactly the same `scan_complete = false` bookkeeping a real failure
    /// would, without depending on either.
    #[test]
    fn an_injected_read_dir_failure_marks_the_scan_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("unreadable");
        std::fs::create_dir(&broken).unwrap();
        std::fs::write(dir.path().join("ordinary.rom"), b"test").unwrap();
        std::fs::write(broken.join("hidden.rom"), b"test").unwrap();

        let cancel = AtomicBool::new(false);
        let scan = scan_local_files_impl(dir.path(), &cancel, &no_progress, &|path| path == broken)
            .unwrap();

        assert!(
            !scan.scan_complete,
            "an injected traversal failure must mark the scan incomplete"
        );
        assert!(
            !scan.truncated,
            "a traversal error is not the same concept as hitting a ceiling"
        );
        assert!(
            scan.files.iter().any(|f| f.ends_with("ordinary.rom")),
            "files outside the failed directory are still collected"
        );
        assert!(
            !scan.files.iter().any(|f| f.ends_with("hidden.rom")),
            "the file behind the failed directory is genuinely lost, not silently found anyway"
        );
    }

    #[test]
    fn a_clean_traversal_with_no_injected_failure_is_scan_complete() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ordinary.rom"), b"test").unwrap();

        let cancel = AtomicBool::new(false);
        let scan = scan_local_files_impl(dir.path(), &cancel, &no_progress, &|_| false).unwrap();

        assert!(scan.scan_complete);
        assert!(!scan.truncated);
    }

    #[test]
    fn a_regular_file_target_is_a_complete_one_file_scan() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("one.st");
        std::fs::write(&file, b"test").unwrap();

        let cancel = AtomicBool::new(false);
        let scan = scan_local_files_impl(&file, &cancel, &no_progress, &|_| false).unwrap();

        assert_eq!(scan.files, vec![file]);
        assert!(scan.scan_complete);
        assert!(!scan.truncated);
    }
}

#[cfg(test)]
mod combined_audit_tests {
    use super::*;

    const TEST_SHA1: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";

    fn dat(game: &str, rom: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><datafile><header><name>Fixture</name><author>No-Intro</author></header><game name="{game}"><rom name="{rom}" size="4" sha1="{TEST_SHA1}"/></game></datafile>"#
        )
    }

    fn source(path: PathBuf, id: &str) -> CombinedDatAuditSource {
        CombinedDatAuditSource {
            source_id: id.to_string(),
            source_display_name: format!("Catalogue {id}"),
            dat_path: path,
            dat_kind: DatSourceKind::File,
            platform: None,
        }
    }

    fn run(sources: Vec<CombinedDatAuditSource>, root: PathBuf) -> DatAuditOutcome {
        run_combined_dat_audit(
            &CombinedDatAuditRequest {
                sources,
                scan_root: root,
                limits: DatLimits::default(),
            },
            &TrustedRoots::none(),
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap()
    }

    #[test]
    fn one_exact_source_is_verified_without_writing_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let dat_path = dir.path().join("one.dat");
        let rom_path = dir.path().join("messy-name.bin");
        std::fs::write(&dat_path, dat("Canonical Game", "canonical.bin")).unwrap();
        std::fs::write(&rom_path, b"test").unwrap();
        let before = std::fs::read(&rom_path).unwrap();

        let outcome = run(vec![source(dat_path, "no-intro")], rom_path.clone());

        assert!(matches!(
            outcome.report.entries[0].verdict,
            AuditVerdict::Exact { .. }
        ));
        assert_eq!(outcome.evidence_sources.len(), 1);
        assert_eq!(outcome.evidence_sources[0].source_id, "no-intro");
        assert_eq!(std::fs::read(&rom_path).unwrap(), before);
    }

    #[test]
    fn agreeing_exact_catalogues_preserve_both_provenances() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.dat");
        let second = dir.path().join("second.dat");
        let rom = dir.path().join("source.bin");
        std::fs::write(&first, dat("Canonical Game", "canonical.bin")).unwrap();
        std::fs::write(&second, dat("Canonical Game", "canonical.bin")).unwrap();
        std::fs::write(&rom, b"test").unwrap();

        let outcome = run(vec![source(first, "one"), source(second, "two")], rom);

        assert!(matches!(
            outcome.report.entries[0].verdict,
            AuditVerdict::Exact { .. }
        ));
        assert_eq!(outcome.evidence_sources.len(), 2);
        assert_eq!(outcome.evidence_sources[0].source_id, "one");
        assert_eq!(outcome.evidence_sources[1].source_id, "two");
        let plan = crate::dat::rename_plan::build_rename_plan(
            &outcome,
            &crate::dat::rename_plan::RenamePlanContext { generation: 1 },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(plan.proposals.len(), 1);
        assert!(
            plan.proposals[0]
                .source_display_name
                .contains("Catalogue one")
        );
        assert!(
            plan.proposals[0]
                .source_display_name
                .contains("Catalogue two")
        );
    }

    #[test]
    fn conflicting_exact_catalogues_are_ambiguous_and_non_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.dat");
        let second = dir.path().join("second.dat");
        let rom = dir.path().join("source.bin");
        std::fs::write(&first, dat("Game A", "a.bin")).unwrap();
        std::fs::write(&second, dat("Game B", "b.bin")).unwrap();
        std::fs::write(&rom, b"test").unwrap();

        let outcome = run(vec![source(first, "one"), source(second, "two")], rom);

        assert!(matches!(
            outcome.report.entries[0].verdict,
            AuditVerdict::Ambiguous { .. }
        ));
        assert!(outcome.evidence_sources.is_empty());
        let plan = crate::dat::rename_plan::build_rename_plan(
            &outcome,
            &crate::dat::rename_plan::RenamePlanContext { generation: 1 },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(plan.proposals.is_empty());
    }

    #[test]
    fn agreeing_names_with_conflicting_explicit_platforms_are_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.dat");
        let second = dir.path().join("second.dat");
        let rom = dir.path().join("source.bin");
        std::fs::write(&first, dat("Canonical Game", "canonical.bin")).unwrap();
        std::fs::write(&second, dat("Canonical Game", "canonical.bin")).unwrap();
        std::fs::write(&rom, b"test").unwrap();

        let mut first_source = source(first, "one");
        first_source.platform = Some("nintendo-nes".to_string());
        let mut second_source = source(second, "two");
        second_source.platform = Some("sega-mega-drive".to_string());
        let outcome = run(vec![first_source, second_source], rom);

        assert!(matches!(
            outcome.report.entries[0].verdict,
            AuditVerdict::Ambiguous { .. }
        ));
        assert!(outcome.evidence_sources.is_empty());
    }

    #[test]
    fn no_match_remains_unmatched() {
        let dir = tempfile::tempdir().unwrap();
        let dat_path = dir.path().join("one.dat");
        let rom = dir.path().join("source.bin");
        std::fs::write(&dat_path, dat("Canonical Game", "canonical.bin")).unwrap();
        std::fs::write(&rom, b"other").unwrap();

        let outcome = run(vec![source(dat_path, "one")], rom);

        assert!(matches!(
            outcome.report.entries[0].verdict,
            AuditVerdict::NotInDat
        ));
    }

    #[test]
    fn combined_archive_outer_is_visible_but_never_raw_hash_matched_or_planned() {
        let dir = tempfile::tempdir().unwrap();
        let dat_path = dir.path().join("one.dat");
        let archive = dir.path().join("source.zip");
        std::fs::write(&dat_path, dat("Canonical Game", "canonical.bin")).unwrap();
        // It need not be a valid archive: the combined loose slice must not
        // inspect outer bytes as though they were a ROM.
        std::fs::write(&archive, b"test").unwrap();

        let outcome = run(vec![source(dat_path, "one")], archive);

        assert_eq!(outcome.report.entries.len(), 1);
        assert!(matches!(
            outcome.report.entries[0].verdict,
            AuditVerdict::NoUsableEvidence
        ));
        assert_eq!(outcome.unhashed.len(), 1);
        assert_eq!(outcome.unhashed[0].code, "combined-container-deferred");
        let plan = crate::dat::rename_plan::build_rename_plan(
            &outcome,
            &crate::dat::rename_plan::RenamePlanContext { generation: 1 },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(plan.proposals.is_empty());
    }
}

#[cfg(test)]
mod rar_dispatch_tests {
    use super::*;

    fn empty_index() -> DatIndex {
        DatIndex {
            by_crc32: std::collections::HashMap::new(),
            by_md5: std::collections::HashMap::new(),
            by_sha1: std::collections::HashMap::new(),
            by_sha256: std::collections::HashMap::new(),
            by_filename: std::collections::HashMap::new(),
            game_clone_of: std::collections::HashMap::new(),
        }
    }

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// `Box<dyn ArchiveMemberSource>` is not `Debug` (the trait does not
    /// require it), so `Result::unwrap_err` cannot be used directly on
    /// `open_archive_source`'s return value.
    fn expect_error(
        result: Result<Box<dyn ArchiveMemberSource>, ArchiveMemberSourceError>,
    ) -> ArchiveMemberSourceError {
        match result {
            Ok(_) => panic!("expected an error, got a constructed ArchiveMemberSource"),
            Err(error) => error,
        }
    }

    #[test]
    fn is_rar_path_is_case_insensitive_and_extension_only() {
        assert!(is_rar_path(Path::new("game.rar")));
        assert!(is_rar_path(Path::new("game.RAR")));
        assert!(is_rar_path(Path::new("game.Rar")));
        assert!(!is_rar_path(Path::new("game.zip")));
        assert!(!is_rar_path(Path::new("game.7z")));
        assert!(!is_rar_path(Path::new("game.rar.bak")));
        assert!(!is_rar_path(Path::new("rar")));
    }

    #[test]
    fn a_rar_path_is_never_dispatched_to_zip_or_sevenz() {
        // `open_archive_source`'s three branches are mutually exclusive by
        // construction (`if`/`else if`/`else if`), so a `.rar` path can only
        // ever reach the RAR branch - proven here by giving it no RAR
        // provider at all and confirming the failure is the RAR-specific
        // "not probed" refusal, not a ZIP/7z parser error (which would mean
        // it fell through to the wrong branch).
        let index = empty_index();
        let error = expect_error(open_archive_source(
            Path::new("/nonexistent/does-not-matter.rar"),
            &TrustedRoots::none(),
            ArchiveLimits::default(),
            &no_cancel(),
            &index,
            None,
        ));
        assert_eq!(
            error,
            ArchiveMemberSourceError::Unsupported {
                detail: "RAR backend was not probed for this run".to_string()
            }
        );
    }

    #[test]
    fn a_rar_path_with_a_failed_discovery_refuses_as_unsupported_never_corrupt() {
        // Provider-unavailable must never be reported as archive corruption
        // or bad ROM data - it is reported as `Unsupported`, the same
        // fail-closed shape `audit_archives` already turns into
        // `ArchivePassCompletion::Incomplete { SourceError }` for any format,
        // never `Complete`.
        let index = empty_index();
        let discovery = Err(RarError::BackendNotFound);
        let error = expect_error(open_archive_source(
            Path::new("/nonexistent/does-not-matter.rar"),
            &TrustedRoots::none(),
            ArchiveLimits::default(),
            &no_cancel(),
            &index,
            Some(&discovery),
        ));
        assert!(matches!(
            error,
            ArchiveMemberSourceError::Unsupported { .. }
        ));
    }

    #[test]
    fn a_missing_provider_is_not_treated_as_a_missing_file() {
        // Even a path that does not exist on disk must fail with the RAR
        // "no provider" reason, not an I/O error - `open_archive_source`
        // resolves the provider *before* touching the filesystem for RAR,
        // exactly mirroring how ZIP/7z fail on a bad path only after their
        // own `open_bounded_read`.
        let index = empty_index();
        let error = expect_error(open_archive_source(
            Path::new("/definitely/does/not/exist/anywhere.rar"),
            &TrustedRoots::none(),
            ArchiveLimits::default(),
            &no_cancel(),
            &index,
            None,
        ));
        assert_eq!(
            error,
            ArchiveMemberSourceError::Unsupported {
                detail: "RAR backend was not probed for this run".to_string()
            }
        );
    }
}
