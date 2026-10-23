//! Bounded, read-only PSP Game Slimmer Phase 2 analysis.
//!
//! This module deliberately stops at evidence.  A directory name such as
//! `MOVIE`, `UPDATE`, or `FRENCH` is useful analysis context, but it is not
//! authority to remove bytes from a game.  No candidate emitted here is
//! `ProvenSafe`; a future profile must add game identity, target settings,
//! and executable/resource evidence before a writer can be considered.

use crate::iso9660::{
    DiscFilesystemObservation, Iso9660Entry, Iso9660Error, find_path, observe_iso9660,
    read_directory_entries,
};
use crate::logical_media::{LogicalMedia, LogicalMediaError};
use crate::param_sfo::{SfoObservation, parse_param_sfo};
use crate::psp_boot_evidence::PspLayoutObservation;
use crate::psp_reversible_shrink::inspect_psp_iso;
use std::cmp::Reverse;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub const MAX_ANALYZER_ENTRIES: usize = 100_000;
pub const MAX_ANALYZER_REPORT_FILES: usize = 64;
pub const MAX_ANALYZER_READ_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ANALYZER_DIRECTORY_DEPTH: usize = 16;
const MAX_SFO_READ_BYTES: usize = crate::param_sfo::MAX_SFO_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PspSlimSafety {
    ProvenSafe,
    ConditionallySafe,
    UnknownUnsafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PspCandidateCategory {
    LanguageLike,
    MovieLike,
    AudioLike,
    UpdateLike,
    InstallLike,
    ManualLike,
    DemoLike,
    PaddingLike,
    DuplicateLike,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PspSlimAction {
    Remove,
    Replace,
    Neutralize,
    Retain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PspSlimVerification {
    StructuralVerified,
    IdentityRetained,
    BootCheckPassed,
    PlayabilityVerified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspSlimProfileRule {
    pub path: String,
    pub category: PspCandidateCategory,
    pub action: PspSlimAction,
    pub safety: PspSlimSafety,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspSlimProfile {
    pub verified_disc_id: String,
    pub target_language: Option<String>,
    pub rules: Vec<PspSlimProfileRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PspSlimProfileError {
    MissingIdentity,
    IdentityMismatch,
    RuleNotFound,
    RuleNotRemovalAuthorized,
}

impl PspSlimProfile {
    /// Returns an authorized rule only when the source identity matches exactly.
    /// This is planning evidence, not a write operation.
    pub fn authorize<'a>(
        &'a self,
        analysis: &PspSlimAnalysis,
        path: &str,
    ) -> Result<&'a PspSlimProfileRule, PspSlimProfileError> {
        let disc_id = analysis
            .identity
            .as_ref()
            .and_then(|identity| identity.disc_id.as_deref())
            .ok_or(PspSlimProfileError::MissingIdentity)?;
        if disc_id != self.verified_disc_id {
            return Err(PspSlimProfileError::IdentityMismatch);
        }
        let rule = self
            .rules
            .iter()
            .find(|rule| rule.path == path)
            .ok_or(PspSlimProfileError::RuleNotFound)?;
        if rule.safety == PspSlimSafety::UnknownUnsafe || rule.action == PspSlimAction::Retain {
            return Err(PspSlimProfileError::RuleNotRemovalAuthorized);
        }
        Ok(rule)
    }
}

impl PspCandidateCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::LanguageLike => "language/resource",
            Self::MovieLike => "movie/video",
            Self::AudioLike => "audio/voice",
            Self::UpdateLike => "update payload",
            Self::InstallLike => "install payload",
            Self::ManualLike => "manual/help",
            Self::DemoLike => "demo content",
            Self::PaddingLike => "padding/dummy",
            Self::DuplicateLike => "duplicate-looking resource",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspSlimCandidate {
    pub path: String,
    pub size: u64,
    pub category: PspCandidateCategory,
    pub detected_language: Option<String>,
    pub evidence_for: Vec<String>,
    pub evidence_against: Vec<String>,
    pub safety: PspSlimSafety,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PspCategoryTotal {
    pub category: PspCandidateCategory,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspSlimFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspSlimIdentity {
    pub disc_id: Option<String>,
    pub title: Option<String>,
    pub category: Option<String>,
    pub disc_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspSlimSavings {
    pub original_size: u64,
    pub estimated_removal_savings: u64,
    pub derived_size: Option<u64>,
    pub compression_savings: Option<u64>,
    pub final_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspSlimAnalysis {
    pub source: PathBuf,
    pub source_sha256: String,
    pub total_size: u64,
    pub filesystem: DiscFilesystemObservation,
    pub layout: PspLayoutObservation,
    pub identity: Option<PspSlimIdentity>,
    pub entries_scanned: usize,
    pub files_truncated: bool,
    pub read_bytes: u64,
    /// Bytes of every file observed by the bounded walk (partial when truncated).
    pub total_file_bytes: u64,
    /// Per-category file counts and bytes; each file lands in at most one category.
    pub category_totals: Vec<PspCategoryTotal>,
    pub largest_files: Vec<PspSlimFile>,
    pub candidates: Vec<PspSlimCandidate>,
    pub estimated_candidate_savings: u64,
    pub savings: PspSlimSavings,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum PspSlimAnalysisError {
    Io(std::io::Error),
    InvalidSource(String),
    Iso9660(Iso9660Error),
    ReadBudgetExceeded {
        limit: u64,
    },
    /// The source is a symlink, special file, or otherwise not a plain regular file.
    UnsafeSource(String),
    Cancelled,
}

impl std::fmt::Display for PspSlimAnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "PSP analysis I/O error: {error}"),
            Self::InvalidSource(message) => write!(f, "PSP source is not usable: {message}"),
            Self::Iso9660(error) => write!(f, "PSP ISO9660 analysis failed: {error}"),
            Self::ReadBudgetExceeded { limit } => {
                write!(f, "PSP analysis read budget exceeded ({limit} bytes)")
            }
            Self::UnsafeSource(message) => write!(f, "PSP source refused: {message}"),
            Self::Cancelled => write!(f, "PSP analysis was cancelled"),
        }
    }
}

impl std::error::Error for PspSlimAnalysisError {}

impl From<std::io::Error> for PspSlimAnalysisError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Iso9660Error> for PspSlimAnalysisError {
    fn from(error: Iso9660Error) -> Self {
        Self::Iso9660(error)
    }
}

struct BoundedFileMedia {
    file: Mutex<File>,
    length: u64,
    read_bytes: Arc<Mutex<u64>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl BoundedFileMedia {
    fn open(path: &Path) -> Result<Self, PspSlimAnalysisError> {
        let file = File::open(path)?;
        Ok(Self::from_file(file, None))
    }

    fn from_file(file: File, cancel: Option<Arc<AtomicBool>>) -> Self {
        let length = file.metadata().map(|m| m.len()).unwrap_or(0);
        Self {
            file: Mutex::new(file),
            length,
            read_bytes: Arc::new(Mutex::new(0)),
            cancel,
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    fn read_bytes(&self) -> u64 {
        *self.read_bytes.lock().expect("read budget lock poisoned")
    }
}

impl LogicalMedia for BoundedFileMedia {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<(), LogicalMediaError> {
        if self.cancelled() {
            return Err(LogicalMediaError::DecodeFailed {
                detail: "cancelled".into(),
            });
        }
        let requested = buffer.len() as u64;
        let mut budget = self
            .read_bytes
            .lock()
            .map_err(|_| LogicalMediaError::DecodeFailed {
                detail: "read budget lock poisoned".into(),
            })?;
        let next = budget.saturating_add(requested);
        if next > MAX_ANALYZER_READ_BYTES {
            return Err(LogicalMediaError::DecodeFailed {
                detail: format!(
                    "bounded PSP analyzer read budget exceeded ({MAX_ANALYZER_READ_BYTES} bytes)"
                ),
            });
        }
        let mut file = self
            .file
            .lock()
            .map_err(|_| LogicalMediaError::DecodeFailed {
                detail: "source file lock poisoned".into(),
            })?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| LogicalMediaError::DecodeFailed {
                detail: error.to_string(),
            })?;
        file.read_exact(buffer)
            .map_err(|error| LogicalMediaError::DecodeFailed {
                detail: error.to_string(),
            })?;
        *budget = next;
        Ok(())
    }
}

pub fn analyze_psp_iso(source: &Path) -> Result<PspSlimAnalysis, PspSlimAnalysisError> {
    let inspection = inspect_psp_iso(source).map_err(|error| match error {
        crate::psp_reversible_shrink::PspShrinkError::Io(error) => PspSlimAnalysisError::Io(error),
        other => PspSlimAnalysisError::InvalidSource(other.to_string()),
    })?;
    let media = BoundedFileMedia::open(source)?;
    analyze_media(
        source,
        &media,
        inspection.source_sha256,
        inspection.source_size,
    )
}

fn analyze_media(
    source: &Path,
    media: &BoundedFileMedia,
    source_sha256: String,
    source_size: u64,
) -> Result<PspSlimAnalysis, PspSlimAnalysisError> {
    let media = media;
    let inspection = PspShrinkLite {
        source_sha256,
        source_size,
    };
    let filesystem = observe_iso9660(media)?;
    let mut layout = PspLayoutObservation {
        psp_game_dir_present: find_path(media, &filesystem, "PSP_GAME")?.is_some(),
        sysdir_present: find_path(media, &filesystem, "PSP_GAME/SYSDIR")?.is_some(),
        eboot_bin_present: find_path(media, &filesystem, "PSP_GAME/SYSDIR/EBOOT.BIN")?.is_some(),
        umd_data_bin_present: find_path(media, &filesystem, "UMD_DATA.BIN")?.is_some(),
        param_sfo: None,
    };
    let param_entry = find_path(media, &filesystem, "PSP_GAME/PARAM.SFO")?;
    if let Some(entry) = &param_entry
        && entry.size as usize <= MAX_SFO_READ_BYTES
    {
        let mut bytes = vec![0u8; entry.size as usize];
        media
            .read_at(entry.extent_lba as u64 * 2048, &mut bytes)
            .map_err(|error| PspSlimAnalysisError::InvalidSource(error.to_string()))?;
        layout.param_sfo = parse_param_sfo(&bytes);
    }
    let identity = layout.param_sfo.as_ref().map(sfo_identity);
    let mut report = WalkReport::default();
    walk_directory(
        media,
        &filesystem,
        &filesystem.root_entries,
        "",
        0,
        &mut report,
    )?;
    let mut largest_files = report.files.clone();
    largest_files.sort_by_key(|file| Reverse(file.size));
    largest_files.truncate(MAX_ANALYZER_REPORT_FILES);
    let candidates = report.candidates.clone();
    let estimated_candidate_savings = candidates
        .iter()
        .filter(|candidate| candidate.safety != PspSlimSafety::ProvenSafe)
        .map(|candidate| candidate.size)
        .sum();
    let mut warnings = vec![
        "No candidate is removal-authorized from filename or directory evidence alone.".into(),
        "Executable/resource reference analysis is not a removal proof and remains incomplete."
            .into(),
    ];
    if !layout.umd_data_bin_present {
        warnings.push("UMD_DATA.BIN was not observed; PSP platform evidence is incomplete.".into());
    }
    if layout.param_sfo.is_none() {
        warnings.push("PARAM.SFO was missing, oversized, or malformed.".into());
    }
    if report.truncated {
        warnings.push(format!(
            "Entry scan stopped at the bounded {}-entry limit.",
            MAX_ANALYZER_ENTRIES
        ));
    }
    Ok(PspSlimAnalysis {
        source: source.to_path_buf(),
        source_sha256: inspection.source_sha256,
        total_size: inspection.source_size,
        filesystem,
        layout,
        identity,
        entries_scanned: report.entries_scanned,
        files_truncated: report.truncated,
        read_bytes: media.read_bytes(),
        total_file_bytes: report.total_file_bytes,
        category_totals: report.category_totals,
        largest_files,
        candidates,
        estimated_candidate_savings,
        savings: PspSlimSavings {
            original_size: inspection.source_size,
            estimated_removal_savings: estimated_candidate_savings,
            derived_size: None,
            compression_savings: None,
            final_size: None,
        },
        warnings,
    })
}

struct PspShrinkLite {
    source_sha256: String,
    source_size: u64,
}

#[derive(Default)]
struct WalkReport {
    total_file_bytes: u64,
    category_totals: Vec<PspCategoryTotal>,
    entries_scanned: usize,
    truncated: bool,
    files: Vec<PspSlimFile>,
    candidates: Vec<PspSlimCandidate>,
}

fn walk_directory(
    media: &BoundedFileMedia,
    filesystem: &DiscFilesystemObservation,
    entries: &[Iso9660Entry],
    parent: &str,
    depth: usize,
    report: &mut WalkReport,
) -> Result<(), PspSlimAnalysisError> {
    if depth > MAX_ANALYZER_DIRECTORY_DEPTH {
        report.truncated = true;
        return Ok(());
    }
    for entry in entries {
        if media.cancelled() {
            return Err(PspSlimAnalysisError::Cancelled);
        }
        if report.entries_scanned >= MAX_ANALYZER_ENTRIES {
            report.truncated = true;
            break;
        }
        report.entries_scanned += 1;
        let path = if parent.is_empty() {
            format!("/{}", entry.comparison_name)
        } else {
            format!("{parent}/{}", entry.comparison_name)
        };
        if entry.is_directory {
            let children = read_directory_entries(
                media,
                entry.extent_lba,
                entry.size,
                filesystem.logical_block_size,
            )?;
            walk_directory(media, filesystem, &children, &path, depth + 1, report)?;
        } else {
            let file = PspSlimFile {
                path: path.clone(),
                size: u64::from(entry.size),
            };
            report.total_file_bytes = report.total_file_bytes.saturating_add(file.size);
            if let Some(candidate) = candidate_for_path(&path, file.size) {
                match report
                    .category_totals
                    .iter_mut()
                    .find(|total| total.category == candidate.category)
                {
                    Some(total) => {
                        total.files += 1;
                        total.bytes = total.bytes.saturating_add(candidate.size);
                    }
                    None => report.category_totals.push(PspCategoryTotal {
                        category: candidate.category,
                        files: 1,
                        bytes: candidate.size,
                    }),
                }
                report.candidates.push(candidate);
            }
            report.files.push(file);
        }
    }
    Ok(())
}

fn sfo_identity(sfo: &SfoObservation) -> PspSlimIdentity {
    PspSlimIdentity {
        disc_id: sfo.get_text("DISC_ID").map(str::to_owned),
        title: sfo.get_text("TITLE").map(str::to_owned),
        category: sfo.get_text("CATEGORY").map(str::to_owned),
        disc_version: sfo.get_text("DISC_VERSION").map(str::to_owned),
    }
}

fn candidate_for_path(path: &str, size: u64) -> Option<PspSlimCandidate> {
    let lower = path.to_ascii_lowercase();
    let (category, language) = if let Some(language) = language_hint(&lower) {
        (
            PspCandidateCategory::LanguageLike,
            Some(language.to_string()),
        )
    } else if contains_any(&lower, &["movie", "video", "cutscene"]) {
        (PspCandidateCategory::MovieLike, None)
    } else if contains_any(&lower, &["audio", "voice", "music", "sound"]) {
        (PspCandidateCategory::AudioLike, None)
    } else if lower.contains("update") {
        (PspCandidateCategory::UpdateLike, None)
    } else if lower.contains("install") {
        (PspCandidateCategory::InstallLike, None)
    } else if contains_any(&lower, &["manual", "help", "readme"]) {
        (PspCandidateCategory::ManualLike, None)
    } else if lower.contains("demo") {
        (PspCandidateCategory::DemoLike, None)
    } else if contains_any(&lower, &["dummy", "padding", "pad."]) {
        (PspCandidateCategory::PaddingLike, None)
    } else if contains_any(&lower, &["copy", "duplicate", "backup"]) {
        (PspCandidateCategory::DuplicateLike, None)
    } else {
        return None;
    };
    Some(PspSlimCandidate {
        path: path.to_string(),
        size,
        category,
        detected_language: language,
        evidence_for: vec!["path/name heuristic matched a research category".into()],
        evidence_against: vec![
            "filename and directory names do not prove that runtime code never references this content".into(),
            "no game-specific executable, manifest, checksum, or selected-language profile rule was applied".into(),
        ],
        safety: PspSlimSafety::UnknownUnsafe,
        reason: "Analysis candidate only; retain unless a verified game-specific profile proves safe removal.".into(),
    })
}

fn language_hint(path: &str) -> Option<&'static str> {
    [
        ("english", "EN"),
        ("french", "FR"),
        ("german", "DE"),
        ("italian", "IT"),
        ("spanish", "ES"),
        ("japanese", "JA"),
        ("korean", "KO"),
        ("chinese", "ZH"),
    ]
    .into_iter()
    .find_map(|(needle, language)| path.contains(needle).then_some(language))
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

// ---------------------------------------------------------------------------
// Bounded, cancellable content report for the Blue "PSP ISO content analyser".
// Read-only: it never opens the source for writing and creates no output.
// ---------------------------------------------------------------------------

pub const PSP_SECTOR_SIZE: u64 = 2048;
const COMPRESSION_SAMPLE_BLOCKS: u64 = 256;
const COMPRESSION_SEGMENTS: usize = 4;
const CSO_HEADER_BYTES: u64 = 24;

/// Identity of the source as observed when analysis started. Any difference
/// later means a report no longer describes the file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspSourceStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub device: u64,
    pub inode: u64,
}

impl PspSourceStamp {
    fn from_metadata(meta: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            device: meta.dev(),
            inode: meta.ino(),
        }
    }

    /// True only if `path` is still the same plain regular file with the same
    /// size, modification time, device and inode. Symlinks are never followed.
    pub fn is_current(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .ok()
            .filter(|meta| meta.file_type().is_file())
            .is_some_and(|meta| Self::from_metadata(&meta) == *self)
    }
}

/// Sampled CSO estimate. It is an extrapolation, never a conversion result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspCompressionEstimate {
    pub sampled_blocks: u64,
    pub total_blocks: u64,
    pub estimated_saved_low: u64,
    pub estimated_saved_high: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspContentReport {
    pub stamp: PspSourceStamp,
    pub analysis: PspSlimAnalysis,
    /// Bytes of the image not attributed to any observed file (system area,
    /// directory extents, padding, or files beyond a truncated scan).
    pub unattributed_bytes: u64,
    /// Bytes of observed files that match no research category.
    pub uncategorised_bytes: u64,
    pub compression: Option<PspCompressionEstimate>,
    pub notes: Vec<String>,
}

/// Open `source` only if it is a plain regular file, without following a
/// symlink and without blocking on a FIFO or device.
fn open_plain_regular(source: &Path) -> Result<(File, std::fs::Metadata), PspSlimAnalysisError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let link_meta = std::fs::symlink_metadata(source)?;
    if !link_meta.file_type().is_file() {
        return Err(PspSlimAnalysisError::UnsafeSource(
            "not a plain regular file (symlinks, devices, FIFOs and directories are refused)"
                .into(),
        ));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() || meta.dev() != link_meta.dev() || meta.ino() != link_meta.ino()
    {
        return Err(PspSlimAnalysisError::UnsafeSource(
            "the file changed while it was being opened".into(),
        ));
    }
    if meta.len() == 0 || meta.len() % PSP_SECTOR_SIZE != 0 {
        return Err(PspSlimAnalysisError::InvalidSource(format!(
            "size {} is not a non-zero multiple of {PSP_SECTOR_SIZE}",
            meta.len()
        )));
    }
    Ok((file, meta))
}

/// Analyse a loose PSP ISO without hashing the whole file. Honours `cancel`
/// between directory entries and on every logical read.
pub fn analyze_psp_iso_report(
    source: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<PspContentReport, PspSlimAnalysisError> {
    let outcome = analyze_report_inner(source, &cancel);
    match outcome {
        Err(_) if cancel.load(Ordering::Relaxed) => Err(PspSlimAnalysisError::Cancelled),
        other => other,
    }
}

fn analyze_report_inner(
    source: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<PspContentReport, PspSlimAnalysisError> {
    let (file, meta) = open_plain_regular(source)?;
    let stamp = PspSourceStamp::from_metadata(&meta);
    let media = BoundedFileMedia::from_file(file, Some(cancel.clone()));
    let mut analysis = analyze_media(source, &media, String::new(), stamp.len)?;
    analysis.source_sha256 = "not computed (bounded analysis)".into();
    let compression = sample_compression(&media, stamp.len);
    if cancel.load(Ordering::Relaxed) {
        return Err(PspSlimAnalysisError::Cancelled);
    }
    analysis.read_bytes = media.read_bytes();
    let categorised: u64 = analysis.category_totals.iter().map(|t| t.bytes).sum();
    let uncategorised_bytes = analysis.total_file_bytes.saturating_sub(categorised);
    let unattributed_bytes = stamp.len.saturating_sub(analysis.total_file_bytes);
    let mut notes = vec![
        "Category sizes come from file and directory names only; they are not removal advice."
            .to_string(),
        "The removal figure is an upper bound of name-matched files. It is not achievable without a verified game-specific profile, which does not exist for this tool."
            .to_string(),
    ];
    if compression.is_none() {
        notes.push(
            "Compression could not be sampled; run the Converter to learn the real CSO size."
                .into(),
        );
    } else {
        notes.push("The compression range is extrapolated from a small sample of sectors. The Converter reports the real size and verifies it.".into());
    }
    if analysis.files_truncated {
        notes.push("The scan hit a bound, so category and file totals are partial.".into());
    }
    if !analysis.layout.psp_game_dir_present || !analysis.layout.eboot_bin_present {
        notes.push(
            "PSP_GAME/SYSDIR/EBOOT.BIN was not observed; this may not be a PSP game image.".into(),
        );
    }
    Ok(PspContentReport {
        stamp,
        analysis,
        unattributed_bytes,
        uncategorised_bytes,
        compression,
        notes,
    })
}

/// Deflate evenly spaced sectors, as the CSO writer would, and extrapolate a
/// low/high saving from the best and worst quarter of the sample.
fn sample_compression(media: &BoundedFileMedia, length: u64) -> Option<PspCompressionEstimate> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;
    let total_blocks = length / PSP_SECTOR_SIZE;
    if total_blocks == 0 {
        return None;
    }
    let sampled = total_blocks.min(COMPRESSION_SAMPLE_BLOCKS);
    let mut raw = [0u64; COMPRESSION_SEGMENTS];
    let mut packed = [0u64; COMPRESSION_SEGMENTS];
    let mut buffer = vec![0u8; PSP_SECTOR_SIZE as usize];
    for index in 0..sampled {
        let block = index * total_blocks / sampled;
        media.read_at(block * PSP_SECTOR_SIZE, &mut buffer).ok()?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&buffer).ok()?;
        let compressed = encoder.finish().ok()?.len() as u64;
        let segment = (index * COMPRESSION_SEGMENTS as u64 / sampled) as usize;
        raw[segment] += PSP_SECTOR_SIZE;
        // CSO stores a block uncompressed when deflate does not shrink it.
        packed[segment] += compressed.min(PSP_SECTOR_SIZE);
    }
    let overhead = CSO_HEADER_BYTES + 4 * (total_blocks + 1);
    let saved = |ratio_num: u64, ratio_den: u64| -> u64 {
        let estimated =
            (u128::from(length) * u128::from(ratio_num) / u128::from(ratio_den.max(1))) as u64;
        length.saturating_sub(estimated.saturating_add(overhead))
    };
    let (mut low, mut high) = (u64::MAX, 0u64);
    for segment in 0..COMPRESSION_SEGMENTS {
        if raw[segment] == 0 {
            continue;
        }
        let value = saved(packed[segment], raw[segment]);
        low = low.min(value);
        high = high.max(value);
    }
    if high == 0 && low == u64::MAX {
        return None;
    }
    Some(PspCompressionEstimate {
        sampled_blocks: sampled,
        total_blocks,
        estimated_saved_low: low.min(high),
        estimated_saved_high: high,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::psp_iso_fixtures::psp_iso;

    #[test]
    fn filename_signal_never_becomes_proven_safe() {
        let candidate = candidate_for_path("/PSP_GAME/USRDIR/FRENCH.PAK", 123).unwrap();
        assert_eq!(candidate.category, PspCandidateCategory::LanguageLike);
        assert_eq!(candidate.detected_language.as_deref(), Some("FR"));
        assert_eq!(candidate.safety, PspSlimSafety::UnknownUnsafe);
        assert!(candidate.evidence_against.len() >= 2);
    }

    #[test]
    fn unknown_assets_are_not_candidates_or_removal_authority() {
        assert!(candidate_for_path("/PSP_GAME/USRDIR/DATA.BIN", 123).is_none());
    }

    #[test]
    fn malformed_iso_fails_closed_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("malformed.iso");
        fs::write(&source, vec![0u8; 2048]).unwrap();
        let before = fs::read(&source).unwrap();
        assert!(analyze_psp_iso(&source).is_err());
        assert_eq!(fs::read(&source).unwrap(), before);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn candidate_limit_and_read_budget_are_explicit() {
        const {
            assert!(MAX_ANALYZER_ENTRIES < usize::MAX);
            assert!(MAX_ANALYZER_REPORT_FILES < MAX_ANALYZER_ENTRIES);
            assert!(MAX_ANALYZER_READ_BYTES < u64::MAX);
            assert!(crate::iso9660::MAX_ENTRIES_PER_DIRECTORY < usize::MAX);
        }
    }

    #[test]
    fn profile_identity_mismatch_fails_closed() {
        let profile = PspSlimProfile {
            verified_disc_id: "UCUS-99999".into(),
            target_language: Some("EN".into()),
            rules: vec![PspSlimProfileRule {
                path: "/PSP_GAME/USRDIR/FRENCH.PAK".into(),
                category: PspCandidateCategory::LanguageLike,
                action: PspSlimAction::Remove,
                safety: PspSlimSafety::ProvenSafe,
                evidence: vec!["game-specific evidence".into()],
            }],
        };
        let analysis = PspSlimAnalysis {
            source: PathBuf::from("source.iso"),
            source_sha256: "hash".into(),
            total_size: 1,
            filesystem: DiscFilesystemObservation {
                filesystem_kind: crate::iso9660::DiscFilesystemKind::Iso9660,
                volume_identifier: String::new(),
                logical_block_size: 2048,
                root_entries: Vec::new(),
            },
            layout: PspLayoutObservation::default(),
            identity: Some(PspSlimIdentity {
                disc_id: Some("UCUS-00001".into()),
                title: None,
                category: None,
                disc_version: None,
            }),
            entries_scanned: 0,
            files_truncated: false,
            read_bytes: 0,
            total_file_bytes: 0,
            category_totals: Vec::new(),
            largest_files: Vec::new(),
            candidates: Vec::new(),
            estimated_candidate_savings: 0,
            savings: PspSlimSavings {
                original_size: 1,
                estimated_removal_savings: 0,
                derived_size: None,
                compression_savings: None,
                final_size: None,
            },
            warnings: Vec::new(),
        };
        assert_eq!(
            profile.authorize(&analysis, "/PSP_GAME/USRDIR/FRENCH.PAK"),
            Err(PspSlimProfileError::IdentityMismatch)
        );
    }

    #[test]
    fn savings_keep_removal_and_compression_separate() {
        let savings = PspSlimSavings {
            original_size: 100,
            estimated_removal_savings: 20,
            derived_size: Some(80),
            compression_savings: Some(10),
            final_size: Some(70),
        };
        assert_eq!(savings.estimated_removal_savings, 20);
        assert_eq!(savings.compression_savings, Some(10));
        assert_ne!(
            savings.estimated_removal_savings,
            savings.compression_savings.unwrap()
        );
    }

    // ----- bounded content report -----------------------------------------

    fn write_iso(dir: &Path) -> PathBuf {
        let path = dir.join("game.iso");
        fs::write(&path, psp_iso()).unwrap();
        path
    }

    fn never() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn report_breaks_down_categories_and_keeps_source_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_iso(dir.path());
        let before = fs::read(&path).unwrap();
        let report = analyze_psp_iso_report(&path, never()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        let a = &report.analysis;
        assert_eq!(a.total_file_bytes, 4096 + 2048 + 2048);
        let movie = a
            .category_totals
            .iter()
            .find(|t| t.category == PspCandidateCategory::MovieLike)
            .unwrap();
        assert_eq!((movie.files, movie.bytes), (1, 4096));
        assert!(
            a.category_totals
                .iter()
                .any(|t| { t.category == PspCandidateCategory::LanguageLike && t.bytes == 2048 })
        );
        assert_eq!(report.uncategorised_bytes, 2048);
        assert_eq!(report.unattributed_bytes, before.len() as u64 - 8192);
        assert_eq!(a.largest_files[0].size, 4096);
        assert!(
            a.candidates
                .iter()
                .all(|c| c.safety == PspSlimSafety::UnknownUnsafe)
        );
        let estimate = report.compression.expect("zero-filled sectors are sampled");
        assert!(estimate.estimated_saved_low <= estimate.estimated_saved_high);
        assert!(estimate.estimated_saved_high < before.len() as u64);
        assert!(report.stamp.is_current(&path));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn report_refuses_symlink_fifo_and_directory_sources() {
        let dir = tempfile::tempdir().unwrap();
        let real = write_iso(dir.path());
        let link = dir.path().join("link.iso");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            analyze_psp_iso_report(&link, never()),
            Err(PspSlimAnalysisError::UnsafeSource(_))
        ));
        let fifo = dir.path().join("pipe.iso");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(matches!(
            analyze_psp_iso_report(&fifo, never()),
            Err(PspSlimAnalysisError::UnsafeSource(_))
        ));
        assert!(matches!(
            analyze_psp_iso_report(dir.path(), never()),
            Err(PspSlimAnalysisError::UnsafeSource(_))
        ));
    }

    #[test]
    fn report_rejects_malformed_and_misaligned_images_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let zeros = dir.path().join("zeros.iso");
        fs::write(&zeros, vec![0u8; 40 * 2048]).unwrap();
        assert!(analyze_psp_iso_report(&zeros, never()).is_err());
        let odd = dir.path().join("odd.iso");
        fs::write(&odd, vec![0u8; 2049]).unwrap();
        assert!(matches!(
            analyze_psp_iso_report(&odd, never()),
            Err(PspSlimAnalysisError::InvalidSource(_))
        ));
        let empty = dir.path().join("empty.iso");
        fs::write(&empty, b"").unwrap();
        assert!(analyze_psp_iso_report(&empty, never()).is_err());
        assert_eq!(fs::read(&zeros).unwrap(), vec![0u8; 40 * 2048]);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn report_honours_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_iso(dir.path());
        let flag = Arc::new(AtomicBool::new(true));
        assert!(matches!(
            analyze_psp_iso_report(&path, flag),
            Err(PspSlimAnalysisError::Cancelled)
        ));
    }

    #[test]
    fn stamp_detects_change_replacement_and_symlink_swap() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_iso(dir.path());
        let stamp = analyze_psp_iso_report(&path, never()).unwrap().stamp;
        assert!(stamp.is_current(&path));
        let mut longer = psp_iso();
        longer.extend(vec![0u8; 2048]);
        fs::write(&path, &longer).unwrap();
        assert!(!stamp.is_current(&path));
        let stamp = analyze_psp_iso_report(&path, never()).unwrap().stamp;
        let other = dir.path().join("other.iso");
        fs::write(&other, &longer).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&other, &path).unwrap();
        assert!(!stamp.is_current(&path));
        fs::remove_file(&path).unwrap();
        assert!(!stamp.is_current(&path));
    }
}
