//! Read-only indexing of user-supplied cheat files.
//!
//! This module is deliberately an import *index*, not an installer.  It reads
//! bounded plain files, adapts the existing RetroArch and PCSX2 parsers, and
//! returns provenance plus match evidence for a future review surface.  No
//! source file, emulator file, database, or cache is written, and no content
//! is executed.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(not(unix))]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::canonical_platform_for_alias;

use super::cheat_provider::ProviderGameMatchConfidence;
use super::cht_document::{ChtDocument, parse_cht_bytes};
use super::pcsx2::{normalize_crc, normalize_serial, parse_patch_identity};
use super::pcsx2_pnach::PnachPatchLine;

/// Maximum bytes read from one selected user cheat file.
pub const USER_CHEAT_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum bytes read across one recursive directory scan.
pub const USER_CHEAT_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum regular files visited during one recursive scan.
pub const USER_CHEAT_MAX_FILES_VISITED: usize = 10_000;
/// Maximum directory depth below the selected root.  The root itself is 0.
pub const USER_CHEAT_MAX_DEPTH: usize = 32;
/// Maximum parsed cheat/code records retained from one file.
pub const USER_CHEAT_MAX_CHEATS_PER_FILE: usize = 16_384;
/// Maximum warnings retained in one report.
pub const USER_CHEAT_MAX_WARNINGS: usize = 256;
/// Maximum path bytes accepted from a user-selected source.
pub const USER_CHEAT_MAX_PATH_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserCheatFormat {
    RetroarchCht,
    Pcsx2Pnach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserCheatSourceOrigin {
    UserSupplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserCheatMatchState {
    Exact,
    Strong,
    Possible,
    Ambiguous,
    Unsupported,
    NoMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserCheatDiagnosticKind {
    UnsupportedFormat,
    ExecutableOrScript,
    SymlinkSkipped,
    NotRegularFile,
    FileTooLarge,
    CumulativeLimitReached,
    FileLimitReached,
    DepthLimitReached,
    PathTooLong,
    PermissionDenied,
    Malformed,
    ReadError,
    DuplicateFile,
    IgnoredFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatDiagnostic {
    pub kind: UserCheatDiagnosticKind,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatEvidence {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatProvenance {
    pub origin: UserCheatSourceOrigin,
    pub original_path: PathBuf,
    pub original_filename: String,
    pub source_sha256: String,
}

/// Identity evidence supplied by the existing library index.  All fields are
/// optional because an archive may be known only by a title or platform.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatLibraryGame {
    pub game_id: String,
    pub title: String,
    pub platform: Option<String>,
    pub region: Option<String>,
    pub serial: Option<String>,
    pub title_id: Option<String>,
    pub crc: Option<String>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatMatch {
    pub game_id: String,
    pub game_title: String,
    pub platform: Option<String>,
    pub state: UserCheatMatchState,
    pub provider_confidence: ProviderGameMatchConfidence,
    pub evidence: Vec<UserCheatEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatCandidate {
    pub format: UserCheatFormat,
    pub provenance: UserCheatProvenance,
    pub title_hints: Vec<String>,
    pub platform_hint: Option<String>,
    pub serial: Option<String>,
    pub title_id: Option<String>,
    pub crc: Option<String>,
    pub cheat_count: usize,
    pub parser_warnings: Vec<String>,
    pub match_state: UserCheatMatchState,
    pub matches: Vec<UserCheatMatch>,
    pub duplicate_of: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatDuplicate {
    pub source_sha256: String,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatImportLimits {
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_files_visited: usize,
    pub max_depth: usize,
    pub max_cheats_per_file: usize,
    pub max_warnings: usize,
}

impl Default for UserCheatImportLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: USER_CHEAT_MAX_FILE_BYTES,
            max_total_bytes: USER_CHEAT_MAX_TOTAL_BYTES,
            max_files_visited: USER_CHEAT_MAX_FILES_VISITED,
            max_depth: USER_CHEAT_MAX_DEPTH,
            max_cheats_per_file: USER_CHEAT_MAX_CHEATS_PER_FILE,
            max_warnings: USER_CHEAT_MAX_WARNINGS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCheatImportReport {
    pub read_only: bool,
    pub writes_performed: bool,
    pub apply_available: bool,
    pub origin: UserCheatSourceOrigin,
    pub scanned_root: PathBuf,
    pub scanned_at_unix_seconds: u64,
    pub candidates: Vec<UserCheatCandidate>,
    pub duplicates: Vec<UserCheatDuplicate>,
    pub diagnostics: Vec<UserCheatDiagnostic>,
    pub files_visited: usize,
    pub bytes_read: u64,
    pub supported_files: usize,
    pub unsupported_files: usize,
    pub skipped_symlinks: usize,
    pub truncated: bool,
}

impl UserCheatImportReport {
    fn new(root: &Path) -> Self {
        Self {
            read_only: true,
            writes_performed: false,
            apply_available: false,
            origin: UserCheatSourceOrigin::UserSupplied,
            scanned_root: root.to_path_buf(),
            scanned_at_unix_seconds: now_unix_seconds(),
            candidates: Vec::new(),
            duplicates: Vec::new(),
            diagnostics: Vec::new(),
            files_visited: 0,
            bytes_read: 0,
            supported_files: 0,
            unsupported_files: 0,
            skipped_symlinks: 0,
            truncated: false,
        }
    }

    fn diagnostic(&mut self, diagnostic: UserCheatDiagnostic, limits: &UserCheatImportLimits) {
        if self.diagnostics.len() < limits.max_warnings {
            self.diagnostics.push(diagnostic);
        } else {
            self.truncated = true;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCheatImportError {
    MissingSource(PathBuf),
    SourceIsDirectory(PathBuf),
    SourceIsNotRegularFile(PathBuf),
    SourceIsSymlink(PathBuf),
    PathTooLong(PathBuf),
    PermissionDenied(PathBuf),
    Io { path: PathBuf, message: String },
    InvalidLimits(String),
}

impl std::fmt::Display for UserCheatImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSource(path) => {
                write!(formatter, "source does not exist: {}", path.display())
            }
            Self::SourceIsDirectory(path) => {
                write!(formatter, "source is a directory: {}", path.display())
            }
            Self::SourceIsNotRegularFile(path) => write!(
                formatter,
                "source is not a regular file: {}",
                path.display()
            ),
            Self::SourceIsSymlink(path) => write!(
                formatter,
                "source symlink is not followed: {}",
                path.display()
            ),
            Self::PathTooLong(path) => write!(
                formatter,
                "source path exceeds the bounded path limit: {}",
                path.display()
            ),
            Self::PermissionDenied(path) => {
                write!(formatter, "source is not readable: {}", path.display())
            }
            Self::Io { path, message } => {
                write!(formatter, "could not inspect {}: {message}", path.display())
            }
            Self::InvalidLimits(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for UserCheatImportError {}

/// Scan one selected plain cheat file.  The source is read and parsed in
/// place; it is never copied, renamed, rewritten, or passed to an executor.
pub fn scan_user_cheat_file(
    path: impl AsRef<Path>,
    library: &[UserCheatLibraryGame],
) -> Result<UserCheatImportReport, UserCheatImportError> {
    scan_user_cheat_file_with_limits(path.as_ref(), library, &UserCheatImportLimits::default())
}

/// Bounded variant of [`scan_user_cheat_file`] used by callers and tests that
/// need a stricter local policy.
pub fn scan_user_cheat_file_with_limits(
    path: &Path,
    library: &[UserCheatLibraryGame],
    limits: &UserCheatImportLimits,
) -> Result<UserCheatImportReport, UserCheatImportError> {
    validate_limits(limits)?;
    validate_path_length(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| map_metadata_error(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(UserCheatImportError::SourceIsSymlink(path.to_path_buf()));
    }
    if metadata.is_dir() {
        return Err(UserCheatImportError::SourceIsDirectory(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(UserCheatImportError::SourceIsNotRegularFile(
            path.to_path_buf(),
        ));
    }
    if is_executable_or_script(path, &metadata) {
        return Err(UserCheatImportError::PermissionDenied(path.to_path_buf()));
    }
    if metadata.len() > limits.max_file_bytes {
        return Err(UserCheatImportError::Io {
            path: path.to_path_buf(),
            message: format!("file exceeds {} byte limit", limits.max_file_bytes),
        });
    }
    let mut report = UserCheatImportReport::new(path);
    report.files_visited = 1;
    match parse_one_file(path, metadata.len(), limits, library, &mut report)? {
        Some(candidate) => {
            report.supported_files = 1;
            report.candidates.push(candidate);
        }
        None => report.unsupported_files = 1,
    }
    Ok(report)
}

/// Recursively scan a selected directory without following directory or file
/// symlinks.  Entries are sorted by their lossless display path before
/// parsing, which makes reports and duplicate ownership deterministic.
pub fn scan_user_cheat_directory(
    root: impl AsRef<Path>,
    library: &[UserCheatLibraryGame],
) -> Result<UserCheatImportReport, UserCheatImportError> {
    scan_user_cheat_directory_with_limits(root.as_ref(), library, &UserCheatImportLimits::default())
}

pub fn scan_user_cheat_directory_with_limits(
    root: &Path,
    library: &[UserCheatLibraryGame],
    limits: &UserCheatImportLimits,
) -> Result<UserCheatImportReport, UserCheatImportError> {
    validate_limits(limits)?;
    validate_path_length(root)?;
    let metadata = fs::symlink_metadata(root).map_err(|error| map_metadata_error(root, error))?;
    if metadata.file_type().is_symlink() {
        return Err(UserCheatImportError::SourceIsSymlink(root.to_path_buf()));
    }
    if !metadata.is_dir() {
        return Err(UserCheatImportError::SourceIsNotRegularFile(
            root.to_path_buf(),
        ));
    }

    let mut report = UserCheatImportReport::new(root);
    let mut files = Vec::new();
    collect_files(root, 0, limits, &mut report, &mut files)?;
    files.sort_by_key(|left| path_sort_key(left));

    let mut hashes = BTreeMap::<String, Vec<PathBuf>>::new();
    for path in files {
        if report.files_visited >= limits.max_files_visited {
            report.truncated = true;
            report.diagnostic(
                UserCheatDiagnostic {
                    kind: UserCheatDiagnosticKind::FileLimitReached,
                    path: root.to_path_buf(),
                    message: format!(
                        "directory scan stopped at {} visited files",
                        limits.max_files_visited
                    ),
                },
                limits,
            );
            break;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                report.diagnostic(
                    UserCheatDiagnostic {
                        kind: UserCheatDiagnosticKind::ReadError,
                        path: path.clone(),
                        message: error.to_string(),
                    },
                    limits,
                );
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            report.skipped_symlinks += 1;
            report.diagnostic(
                UserCheatDiagnostic {
                    kind: UserCheatDiagnosticKind::SymlinkSkipped,
                    path,
                    message: "symlink was not followed".to_string(),
                },
                limits,
            );
            continue;
        }
        if !metadata.is_file() {
            report.diagnostic(
                UserCheatDiagnostic {
                    kind: UserCheatDiagnosticKind::NotRegularFile,
                    path,
                    message: "entry is not a regular file".to_string(),
                },
                limits,
            );
            continue;
        }
        report.files_visited += 1;
        if is_executable_or_script(&path, &metadata) {
            report.unsupported_files += 1;
            report.diagnostic(
                UserCheatDiagnostic {
                    kind: UserCheatDiagnosticKind::ExecutableOrScript,
                    path,
                    message: "executable or script content is never imported".to_string(),
                },
                limits,
            );
            continue;
        }
        if metadata.len() > limits.max_file_bytes {
            report.unsupported_files += 1;
            report.diagnostic(
                UserCheatDiagnostic {
                    kind: UserCheatDiagnosticKind::FileTooLarge,
                    path,
                    message: format!(
                        "file is {} bytes; limit is {}",
                        metadata.len(),
                        limits.max_file_bytes
                    ),
                },
                limits,
            );
            continue;
        }
        if report.bytes_read.saturating_add(metadata.len()) > limits.max_total_bytes {
            report.truncated = true;
            report.diagnostic(
                UserCheatDiagnostic {
                    kind: UserCheatDiagnosticKind::CumulativeLimitReached,
                    path,
                    message: format!("cumulative byte limit {} reached", limits.max_total_bytes),
                },
                limits,
            );
            break;
        }
        match parse_one_file(&path, metadata.len(), limits, library, &mut report) {
            Ok(Some(candidate)) => {
                report.supported_files += 1;
                hashes
                    .entry(candidate.provenance.source_sha256.clone())
                    .or_default()
                    .push(candidate.provenance.original_path.clone());
                report.candidates.push(candidate);
            }
            Ok(None) => report.unsupported_files += 1,
            Err(error) => {
                report.unsupported_files += 1;
                report.diagnostic(
                    UserCheatDiagnostic {
                        kind: diagnostic_kind_for_error(&error),
                        path,
                        message: error.to_string(),
                    },
                    limits,
                );
            }
        }
    }
    apply_duplicate_groups(&mut report, hashes, limits);
    Ok(report)
}

fn collect_files(
    directory: &Path,
    depth: usize,
    limits: &UserCheatImportLimits,
    report: &mut UserCheatImportReport,
    files: &mut Vec<PathBuf>,
) -> Result<(), UserCheatImportError> {
    if depth > limits.max_depth {
        report.truncated = true;
        report.diagnostic(
            UserCheatDiagnostic {
                kind: UserCheatDiagnosticKind::DepthLimitReached,
                path: directory.to_path_buf(),
                message: format!("directory depth exceeds {}", limits.max_depth),
            },
            limits,
        );
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|error| map_metadata_error(directory, error))?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| UserCheatImportError::Io {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?;
    entries.sort_by_key(|left| path_sort_key(&left.path()));
    for entry in entries {
        let path = entry.path();
        validate_path_length(&path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                report.diagnostic(
                    UserCheatDiagnostic {
                        kind: UserCheatDiagnosticKind::ReadError,
                        path,
                        message: error.to_string(),
                    },
                    limits,
                );
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            report.skipped_symlinks += 1;
            report.diagnostic(
                UserCheatDiagnostic {
                    kind: UserCheatDiagnosticKind::SymlinkSkipped,
                    path,
                    message: "symlink was not followed".to_string(),
                },
                limits,
            );
        } else if metadata.is_dir() {
            collect_files(&path, depth + 1, limits, report, files)?;
        } else if metadata.is_file() {
            files.push(path);
        } else {
            report.diagnostic(
                UserCheatDiagnostic {
                    kind: UserCheatDiagnosticKind::NotRegularFile,
                    path,
                    message: "entry is not a regular file".to_string(),
                },
                limits,
            );
        }
    }
    Ok(())
}

fn parse_one_file(
    path: &Path,
    length: u64,
    limits: &UserCheatImportLimits,
    library: &[UserCheatLibraryGame],
    report: &mut UserCheatImportReport,
) -> Result<Option<UserCheatCandidate>, UserCheatImportError> {
    let format = match format_for_path(path) {
        Some(format) => format,
        None => {
            if is_known_unsafe_extension(path) {
                report.diagnostic(
                    UserCheatDiagnostic {
                        kind: UserCheatDiagnosticKind::ExecutableOrScript,
                        path: path.to_path_buf(),
                        message: "executable or script extension is never imported".to_string(),
                    },
                    limits,
                );
            } else if is_known_unsupported_extension(path) {
                report.diagnostic(
                    UserCheatDiagnostic {
                        kind: UserCheatDiagnosticKind::UnsupportedFormat,
                        path: path.to_path_buf(),
                        message: "archives and other non-plain formats are not unpacked in Phase 1"
                            .to_string(),
                    },
                    limits,
                );
            }
            return Ok(None);
        }
    };
    if length > limits.max_file_bytes {
        return Err(UserCheatImportError::Io {
            path: path.to_path_buf(),
            message: format!("file exceeds {} byte limit", limits.max_file_bytes),
        });
    }
    let bytes = read_bounded(path, length, limits.max_file_bytes)?;
    report.bytes_read = report.bytes_read.saturating_add(bytes.len() as u64);
    let source_sha256 = sha256_hex(&bytes);
    let provenance = UserCheatProvenance {
        origin: UserCheatSourceOrigin::UserSupplied,
        original_path: path.to_path_buf(),
        original_filename: path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default(),
        source_sha256,
    };
    match format {
        UserCheatFormat::RetroarchCht => {
            let document = parse_cht_bytes(&bytes).map_err(|error| UserCheatImportError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            Ok(Some(candidate_from_cht(
                path, provenance, document, limits, library,
            )))
        }
        UserCheatFormat::Pcsx2Pnach => parse_pnach(path, provenance, &bytes, limits, library),
    }
}

fn candidate_from_cht(
    path: &Path,
    provenance: UserCheatProvenance,
    document: ChtDocument,
    limits: &UserCheatImportLimits,
    library: &[UserCheatLibraryGame],
) -> UserCheatCandidate {
    let mut warnings = document
        .warnings
        .iter()
        .map(|warning| warning.detail.clone())
        .collect::<Vec<_>>();
    for entry in &document.entries {
        for warning in &entry.warnings {
            if warnings.len() >= limits.max_warnings {
                break;
            }
            warnings.push(warning.detail.clone());
        }
    }
    let title = path
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unknown cheat set".to_string());
    let platform_hint = infer_platform_hint(path);
    let matches = match_library(
        std::slice::from_ref(&title),
        platform_hint.as_deref(),
        None,
        None,
        None,
        library,
    );
    let match_state = overall_match_state(&matches);
    UserCheatCandidate {
        format: UserCheatFormat::RetroarchCht,
        provenance,
        title_hints: vec![title],
        platform_hint,
        serial: None,
        title_id: None,
        crc: None,
        cheat_count: document.entries.len().min(limits.max_cheats_per_file),
        parser_warnings: warnings.into_iter().take(limits.max_warnings).collect(),
        match_state,
        matches,
        duplicate_of: None,
    }
}

fn parse_pnach(
    path: &Path,
    provenance: UserCheatProvenance,
    bytes: &[u8],
    limits: &UserCheatImportLimits,
    library: &[UserCheatLibraryGame],
) -> Result<Option<UserCheatCandidate>, UserCheatImportError> {
    let text = std::str::from_utf8(bytes).map_err(|_| UserCheatImportError::Io {
        path: path.to_path_buf(),
        message: "PNACH is not valid UTF-8".to_string(),
    })?;
    let mut warnings = Vec::new();
    let mut patch_count = 0usize;
    let mut title_hints = Vec::new();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with(';') {
            continue;
        }
        if let Some(value) = line.strip_prefix("gametitle=") {
            let value = value.trim();
            if !value.is_empty() {
                title_hints.push(value.to_string());
            }
            continue;
        }
        if line.starts_with("comment=") || line.starts_with("comment_") {
            continue;
        }
        if line.starts_with("patch=") {
            match PnachPatchLine::parse(line) {
                Ok(_) => {
                    if patch_count < limits.max_cheats_per_file {
                        patch_count += 1;
                    } else if warnings.len() < limits.max_warnings {
                        warnings.push(format!("line {}: PNACH code limit reached", line_index + 1));
                    }
                }
                Err(error) => {
                    if warnings.len() < limits.max_warnings {
                        warnings.push(format!("line {}: {error}", line_index + 1));
                    }
                }
            }
            continue;
        }
        if line.contains('=') {
            // PCSX2 has other metadata directives. Preserve no executable
            // behavior; unknown directives remain a bounded warning.
            if warnings.len() < limits.max_warnings {
                warnings.push(format!(
                    "line {}: unsupported PNACH directive",
                    line_index + 1
                ));
            }
        } else if warnings.len() < limits.max_warnings {
            warnings.push(format!("line {}: malformed PNACH line", line_index + 1));
        }
    }
    if patch_count == 0 {
        return Err(UserCheatImportError::Io {
            path: path.to_path_buf(),
            message: "PNACH contains no valid patch codes".to_string(),
        });
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let (serial, crc) = parse_patch_identity(stem);
    let platform_hint = Some("PlayStation 2".to_string());
    if title_hints.is_empty() {
        title_hints.push(stem.to_string());
    }
    let matches = match_library(
        &title_hints,
        platform_hint.as_deref(),
        serial.as_deref(),
        None,
        crc.as_deref(),
        library,
    );
    Ok(Some(UserCheatCandidate {
        format: UserCheatFormat::Pcsx2Pnach,
        provenance,
        title_hints,
        platform_hint,
        serial,
        title_id: None,
        crc,
        cheat_count: patch_count,
        parser_warnings: warnings,
        match_state: overall_match_state(&matches),
        matches,
        duplicate_of: None,
    }))
}

fn match_library(
    title_hints: &[String],
    platform_hint: Option<&str>,
    serial: Option<&str>,
    title_id: Option<&str>,
    crc: Option<&str>,
    library: &[UserCheatLibraryGame],
) -> Vec<UserCheatMatch> {
    let normalized_titles = title_hints
        .iter()
        .map(|title| normalize_title(title))
        .filter(|title| !title.is_empty())
        .collect::<BTreeSet<_>>();
    let canonical_hint = platform_hint.and_then(canonical_platform_for_alias);
    let mut matches = Vec::new();
    for game in library {
        let canonical_game_platform = game
            .platform
            .as_deref()
            .and_then(canonical_platform_for_alias);
        let platform_matches = match (canonical_hint, canonical_game_platform) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        };
        let title_matches = normalized_titles.contains(&normalize_title(&game.title));
        let serial_matches = serial.is_some_and(|value| {
            game.serial
                .as_deref()
                .and_then(normalize_serial)
                .is_some_and(|candidate| candidate == value)
        });
        let title_id_matches = title_id.is_some_and(|value| {
            game.title_id.as_deref().is_some_and(|candidate| {
                normalize_identifier(candidate) == normalize_identifier(value)
            })
        });
        let crc_matches = crc.is_some_and(|value| {
            game.crc
                .as_deref()
                .and_then(normalize_crc)
                .is_some_and(|candidate| candidate == value)
        });
        let content_hash_matches = crc.is_some_and(|value| {
            game.content_hash.as_deref().is_some_and(|candidate| {
                normalize_identifier(candidate) == normalize_identifier(value)
            })
        });
        let mut evidence = Vec::new();
        if platform_matches {
            evidence.push(UserCheatEvidence {
                kind: "platform_match".to_string(),
                value: canonical_hint.unwrap_or_default().to_string(),
            });
        }
        if title_matches {
            evidence.push(UserCheatEvidence {
                kind: "normalized_title_match".to_string(),
                value: game.title.clone(),
            });
        }
        if serial_matches {
            evidence.push(UserCheatEvidence {
                kind: "exact_serial".to_string(),
                value: serial.unwrap_or_default().to_string(),
            });
        }
        if title_id_matches {
            evidence.push(UserCheatEvidence {
                kind: "exact_title_id".to_string(),
                value: title_id.unwrap_or_default().to_string(),
            });
        }
        if crc_matches {
            evidence.push(UserCheatEvidence {
                kind: "exact_crc".to_string(),
                value: crc.unwrap_or_default().to_string(),
            });
        }
        if content_hash_matches {
            evidence.push(UserCheatEvidence {
                kind: "exact_content_hash".to_string(),
                value: crc.unwrap_or_default().to_string(),
            });
        }
        let state = if platform_matches
            && (serial_matches || title_id_matches || crc_matches || content_hash_matches)
        {
            UserCheatMatchState::Exact
        } else if platform_matches && title_matches {
            UserCheatMatchState::Strong
        } else if title_matches
            || platform_matches
            || serial_matches
            || title_id_matches
            || crc_matches
            || content_hash_matches
        {
            UserCheatMatchState::Possible
        } else {
            continue;
        };
        let provider_confidence = match state {
            UserCheatMatchState::Exact if crc_matches || content_hash_matches => {
                ProviderGameMatchConfidence::ExactHashPlatform
            }
            UserCheatMatchState::Exact if serial_matches => {
                ProviderGameMatchConfidence::ExactSerialPlatformRegion
            }
            UserCheatMatchState::Exact => ProviderGameMatchConfidence::ExactTitlePlatform,
            UserCheatMatchState::Strong => ProviderGameMatchConfidence::ExactTitlePlatform,
            UserCheatMatchState::Possible => ProviderGameMatchConfidence::ProbableTitlePlatform,
            _ => ProviderGameMatchConfidence::NoMatch,
        };
        matches.push(UserCheatMatch {
            game_id: game.game_id.clone(),
            game_title: game.title.clone(),
            platform: game.platform.clone(),
            state,
            provider_confidence,
            evidence,
        });
    }
    matches.sort_by(|left, right| {
        match_strength(right.state)
            .cmp(&match_strength(left.state))
            .then_with(|| left.game_id.cmp(&right.game_id))
    });
    let best = matches
        .first()
        .map(|candidate| match_strength(candidate.state));
    if let Some(best) = best {
        let tied = matches
            .iter()
            .filter(|candidate| match_strength(candidate.state) == best)
            .count();
        if tied > 1 {
            for candidate in &mut matches {
                if match_strength(candidate.state) == best {
                    candidate.state = UserCheatMatchState::Ambiguous;
                    candidate.provider_confidence = ProviderGameMatchConfidence::Ambiguous;
                }
            }
        }
    }
    matches
}

fn overall_match_state(matches: &[UserCheatMatch]) -> UserCheatMatchState {
    let Some(best) = matches.first() else {
        return UserCheatMatchState::NoMatch;
    };
    if matches
        .iter()
        .any(|candidate| candidate.state == UserCheatMatchState::Ambiguous)
    {
        UserCheatMatchState::Ambiguous
    } else {
        best.state
    }
}

fn apply_duplicate_groups(
    report: &mut UserCheatImportReport,
    hashes: BTreeMap<String, Vec<PathBuf>>,
    limits: &UserCheatImportLimits,
) {
    for (source_sha256, mut paths) in hashes {
        if paths.len() < 2 {
            continue;
        }
        paths.sort_by_key(|left| path_sort_key(left));
        let first = paths[0].clone();
        report.duplicates.push(UserCheatDuplicate {
            source_sha256: source_sha256.clone(),
            paths: paths.clone(),
        });
        for candidate in &mut report.candidates {
            if candidate.provenance.source_sha256 == source_sha256
                && candidate.provenance.original_path != first
            {
                candidate.duplicate_of = Some(first.clone());
            }
        }
        report.diagnostic(
            UserCheatDiagnostic {
                kind: UserCheatDiagnosticKind::DuplicateFile,
                path: first,
                message: format!(
                    "{} byte-identical user-supplied files share SHA-256 {source_sha256}",
                    paths.len()
                ),
            },
            limits,
        );
    }
}

fn format_for_path(path: &Path) -> Option<UserCheatFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "cht" => Some(UserCheatFormat::RetroarchCht),
        "pnach" => Some(UserCheatFormat::Pcsx2Pnach),
        _ => None,
    }
}

fn infer_platform_hint(path: &Path) -> Option<String> {
    path.ancestors()
        .skip(1)
        .filter_map(|ancestor| ancestor.file_name())
        .filter_map(|name| name.to_str())
        .find_map(canonical_platform_for_alias)
        .map(str::to_string)
}

fn read_bounded(path: &Path, length: u64, maximum: u64) -> Result<Vec<u8>, UserCheatImportError> {
    if length > maximum {
        return Err(UserCheatImportError::Io {
            path: path.to_path_buf(),
            message: format!("file exceeds {maximum} byte limit"),
        });
    }
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| map_metadata_error(path, error))?
    };
    #[cfg(not(unix))]
    let file = File::open(path).map_err(|error| map_metadata_error(path, error))?;
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| UserCheatImportError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    if bytes.len() as u64 > maximum {
        return Err(UserCheatImportError::Io {
            path: path.to_path_buf(),
            message: format!("file grew beyond {maximum} byte limit while reading"),
        });
    }
    Ok(bytes)
}

fn validate_limits(limits: &UserCheatImportLimits) -> Result<(), UserCheatImportError> {
    if limits.max_file_bytes == 0
        || limits.max_total_bytes < limits.max_file_bytes
        || limits.max_files_visited == 0
        || limits.max_cheats_per_file == 0
        || limits.max_warnings == 0
    {
        return Err(UserCheatImportError::InvalidLimits(
            "user cheat import limits must all be positive and cumulative bytes must cover one file".to_string(),
        ));
    }
    Ok(())
}

fn validate_path_length(path: &Path) -> Result<(), UserCheatImportError> {
    if path.as_os_str().len() > USER_CHEAT_MAX_PATH_BYTES {
        return Err(UserCheatImportError::PathTooLong(path.to_path_buf()));
    }
    Ok(())
}

fn map_metadata_error(path: &Path, error: io::Error) -> UserCheatImportError {
    match error.kind() {
        io::ErrorKind::NotFound => UserCheatImportError::MissingSource(path.to_path_buf()),
        io::ErrorKind::PermissionDenied => {
            UserCheatImportError::PermissionDenied(path.to_path_buf())
        }
        _ => UserCheatImportError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        },
    }
}

fn diagnostic_kind_for_error(error: &UserCheatImportError) -> UserCheatDiagnosticKind {
    match error {
        UserCheatImportError::PermissionDenied(_) => UserCheatDiagnosticKind::PermissionDenied,
        UserCheatImportError::PathTooLong(_) => UserCheatDiagnosticKind::PathTooLong,
        UserCheatImportError::SourceIsSymlink(_) => UserCheatDiagnosticKind::SymlinkSkipped,
        UserCheatImportError::Io { message, .. } if message.contains("exceeds") => {
            UserCheatDiagnosticKind::FileTooLarge
        }
        UserCheatImportError::Io { .. } => UserCheatDiagnosticKind::Malformed,
        _ => UserCheatDiagnosticKind::ReadError,
    }
}

fn is_executable_or_script(path: &Path, metadata: &fs::Metadata) -> bool {
    if is_known_unsafe_extension(path) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn is_known_unsafe_extension(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "exe" | "bat" | "cmd" | "ps1" | "sh" | "jar" | "appimage" | "bin" | "com" | "run"
    )
}

fn is_known_unsupported_extension(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "iso" | "dsk"
    )
}

fn path_sort_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn normalize_title(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_uppercase())
        .collect()
}

fn match_strength(state: UserCheatMatchState) -> u8 {
    match state {
        UserCheatMatchState::Exact => 4,
        UserCheatMatchState::Strong => 3,
        UserCheatMatchState::Possible => 2,
        UserCheatMatchState::Ambiguous => 1,
        UserCheatMatchState::Unsupported | UserCheatMatchState::NoMatch => 0,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn game(id: &str, title: &str, platform: &str) -> UserCheatLibraryGame {
        UserCheatLibraryGame {
            game_id: id.to_string(),
            title: title.to_string(),
            platform: Some(platform.to_string()),
            ..UserCheatLibraryGame::default()
        }
    }

    fn write(root: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn valid_cht_import_retains_provenance_and_hash_without_writing() {
        let root = tempdir().unwrap();
        let platform_root = root.path().join("Mega Drive");
        fs::create_dir_all(&platform_root).unwrap();
        let path = write(
            &platform_root,
            "Sonic the Hedgehog.cht",
            br#"cheats = 1
cheat0_desc = "Infinite lives"
cheat0_code = "1234+ABCD"
cheat0_enable = true
"#,
        );
        let before = fs::read(&path).unwrap();
        let report =
            scan_user_cheat_file(&path, &[game("sonic", "Sonic the Hedgehog", "Mega Drive")])
                .unwrap();
        assert!(report.read_only);
        assert!(!report.writes_performed);
        assert!(!report.apply_available);
        assert_eq!(report.supported_files, 1);
        assert_eq!(report.candidates[0].format, UserCheatFormat::RetroarchCht);
        assert_eq!(report.candidates[0].cheat_count, 1);
        assert_eq!(
            report.candidates[0].match_state,
            UserCheatMatchState::Strong
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            report.candidates[0].provenance.origin,
            UserCheatSourceOrigin::UserSupplied
        );
        assert_eq!(
            report.candidates[0].provenance.source_sha256,
            sha256_hex(&before)
        );
    }

    #[test]
    fn valid_pnach_import_uses_existing_patch_validator_and_exact_identity() {
        let root = tempdir().unwrap();
        let path = write(
            root.path(),
            "SLUS-20312_A1B2C3D4.pnach",
            b"gametitle=Example\npatch=1,EE,00345678,word,12345678\n",
        );
        let mut expected = game("ps2-game", "Example", "PlayStation 2");
        expected.serial = Some("SLUS-20312".to_string());
        expected.crc = Some("A1B2C3D4".to_string());
        let report = scan_user_cheat_file(&path, &[expected]).unwrap();
        let candidate = &report.candidates[0];
        assert_eq!(candidate.format, UserCheatFormat::Pcsx2Pnach);
        assert_eq!(candidate.cheat_count, 1);
        assert_eq!(candidate.serial.as_deref(), Some("SLUS-20312"));
        assert_eq!(candidate.crc.as_deref(), Some("A1B2C3D4"));
        assert_eq!(candidate.match_state, UserCheatMatchState::Exact);
    }

    #[test]
    fn malformed_file_does_not_abort_directory_scan() {
        let root = tempdir().unwrap();
        write(root.path(), "bad.pnach", b"patch=not-a-valid-code\n");
        write(
            root.path(),
            "good.cht",
            b"cheats = 1\ncheat0_code = \"AAAA\"\n",
        );
        let report = scan_user_cheat_directory(root.path(), &[]).unwrap();
        assert_eq!(report.supported_files, 1);
        assert_eq!(report.unsupported_files, 1);
        assert_eq!(report.candidates[0].format, UserCheatFormat::RetroarchCht);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == UserCheatDiagnosticKind::Malformed)
        );
    }

    #[test]
    fn scripts_and_executables_are_never_imported() {
        let root = tempdir().unwrap();
        write(root.path(), "run.sh", b"#!/bin/sh\necho unsafe\n");
        write(root.path(), "payload.exe", b"MZ\0\0");
        let report = scan_user_cheat_directory(root.path(), &[]).unwrap();
        assert_eq!(report.candidates.len(), 0);
        assert_eq!(report.unsupported_files, 2);
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.kind == UserCheatDiagnosticKind::ExecutableOrScript)
                .count(),
            2
        );
    }

    #[test]
    fn recursive_scan_is_bounded_and_deterministic() {
        let root = tempdir().unwrap();
        let nested = root.path().join("one").join("two");
        fs::create_dir_all(&nested).unwrap();
        write(&nested, "deep.cht", b"cheats = 1\ncheat0_code = \"AAAA\"\n");
        let limits = UserCheatImportLimits {
            max_depth: 1,
            ..UserCheatImportLimits::default()
        };
        let report = scan_user_cheat_directory_with_limits(root.path(), &[], &limits).unwrap();
        assert!(report.truncated);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == UserCheatDiagnosticKind::DepthLimitReached)
        );
    }

    #[test]
    fn directory_symlink_is_skipped() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        write(
            outside.path(),
            "outside.cht",
            b"cheats = 1\ncheat0_code = \"AAAA\"\n",
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        let report = scan_user_cheat_directory(root.path(), &[]).unwrap();
        #[cfg(unix)]
        assert_eq!(report.skipped_symlinks, 1);
        #[cfg(unix)]
        assert!(report.candidates.is_empty());
    }

    #[test]
    fn duplicate_files_are_reported_without_deletion() {
        let root = tempdir().unwrap();
        let bytes = b"cheats = 1\ncheat0_code = \"AAAA\"\n";
        let first = write(root.path(), "a.cht", bytes);
        let second = write(root.path(), "b.cht", bytes);
        let report = scan_user_cheat_directory(root.path(), &[]).unwrap();
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(
            report.duplicates[0].paths,
            vec![first.clone(), second.clone()]
        );
        assert_eq!(report.candidates[1].duplicate_of, Some(first));
        assert_eq!(fs::read(&second).unwrap(), bytes);
    }

    #[test]
    fn title_platform_is_possible_without_exact_identity() {
        let root = tempdir().unwrap();
        let platform_root = root.path().join("PlayStation 2");
        fs::create_dir_all(&platform_root).unwrap();
        let path = write(
            &platform_root,
            "Example.cht",
            b"cheats = 1\ncheat0_code = \"AAAA\"\n",
        );
        let mut candidate_game = game("one", "Example", "PlayStation 2");
        candidate_game.region = Some("US".to_string());
        let report = scan_user_cheat_file(&path, &[candidate_game]).unwrap();
        assert_eq!(
            report.candidates[0].match_state,
            UserCheatMatchState::Strong
        );
        assert_ne!(report.candidates[0].match_state, UserCheatMatchState::Exact);
    }

    #[test]
    fn ambiguous_best_matches_are_not_auto_selected() {
        let root = tempdir().unwrap();
        let path = write(
            root.path(),
            "Example.cht",
            b"cheats = 1\ncheat0_code = \"AAAA\"\n",
        );
        let report = scan_user_cheat_file(
            &path,
            &[
                game("one", "Example", "PlayStation 2"),
                game("two", "Example", "PlayStation 2"),
            ],
        )
        .unwrap();
        assert_eq!(
            report.candidates[0].match_state,
            UserCheatMatchState::Ambiguous
        );
        assert!(
            report.candidates[0]
                .matches
                .iter()
                .all(|candidate| candidate.state == UserCheatMatchState::Ambiguous)
        );
    }

    #[test]
    fn large_input_is_rejected_before_unbounded_parse() {
        let root = tempdir().unwrap();
        let path = write(
            root.path(),
            "large.cht",
            b"cheats = 1\ncheat0_code = \"AAAA\"\n",
        );
        let limits = UserCheatImportLimits {
            max_file_bytes: 4,
            max_total_bytes: 4,
            ..UserCheatImportLimits::default()
        };
        let error = scan_user_cheat_file_with_limits(&path, &[], &limits).unwrap_err();
        assert!(error.to_string().contains("limit"));
    }

    #[test]
    fn random_malformed_data_returns_an_error_or_unsupported_without_panicking() {
        let root = tempdir().unwrap();
        for index in 0..32 {
            let path = write(
                root.path(),
                &format!("random-{index}.cht"),
                &[index as u8; 64],
            );
            let result = scan_user_cheat_file(&path, &[]);
            assert!(result.is_ok() || result.is_err());
        }
    }
}
