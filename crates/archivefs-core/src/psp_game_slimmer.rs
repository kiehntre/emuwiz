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
use std::sync::{Arc, Mutex};

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
    ReadBudgetExceeded { limit: u64 },
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
}

impl BoundedFileMedia {
    fn open(path: &Path) -> Result<Self, PspSlimAnalysisError> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
            length,
            read_bytes: Arc::new(Mutex::new(0)),
        })
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
    let filesystem = observe_iso9660(&media)?;
    let mut layout = PspLayoutObservation {
        psp_game_dir_present: find_path(&media, &filesystem, "PSP_GAME")?.is_some(),
        sysdir_present: find_path(&media, &filesystem, "PSP_GAME/SYSDIR")?.is_some(),
        eboot_bin_present: find_path(&media, &filesystem, "PSP_GAME/SYSDIR/EBOOT.BIN")?.is_some(),
        umd_data_bin_present: find_path(&media, &filesystem, "UMD_DATA.BIN")?.is_some(),
        param_sfo: None,
    };
    let param_entry = find_path(&media, &filesystem, "PSP_GAME/PARAM.SFO")?;
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
        &media,
        &filesystem,
        &filesystem.root_entries,
        "",
        0,
        &mut report,
    )?;
    let mut largest_files = report.files.clone();
    largest_files.sort_by_key(|file| Reverse(file.size));
    largest_files.truncate(MAX_ANALYZER_REPORT_FILES);
    let candidates = report.candidates;
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

#[derive(Default)]
struct WalkReport {
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
            if let Some(candidate) = candidate_for_path(&path, file.size) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
}
