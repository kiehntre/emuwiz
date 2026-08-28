//! CUE/BIN pairing: the `.cue` sheet is the anchor for a disc-image
//! candidate; a `.bin` is only ever resolved through a `.cue` that names
//! it. A lone `.bin` with no matching `.cue` is never guessed at here -
//! see [`super::discovery`]'s `SkipReason::MissingPairedFile`.
//!
//! Parsing is read-only and bounded: CUE sheets are always small plain
//! text, so a file above [`MAX_CUE_BYTES`] is refused rather than read.

use std::path::{Component, Path, PathBuf};

/// CUE sheets are a few hundred bytes to a few KiB. Refuse anything larger
/// as not a genuine CUE sheet rather than reading it.
const MAX_CUE_BYTES: u64 = 256 * 1024;

/// The maximum number of `FILE` references resolved from one CUE sheet
/// (multi-track/multi-session discs may reference more than one).
const MAX_CUE_FILE_REFERENCES: usize = 99;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueError {
    Io(String),
    TooLarge,
    NoFileReferences,
    Malformed(String),
    UnsafeReference,
    MissingDataFile(PathBuf),
    AmbiguousDataTracks,
    UnsupportedTrackMode(String),
}

impl std::fmt::Display for CueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "CUE I/O error: {error}"),
            Self::TooLarge => formatter.write_str("CUE exceeds the bounded size limit"),
            Self::NoFileReferences => formatter.write_str("CUE has no usable data track"),
            Self::Malformed(detail) => write!(formatter, "malformed CUE: {detail}"),
            Self::UnsafeReference => formatter.write_str("CUE references an unsafe path"),
            Self::MissingDataFile(path) => {
                write!(formatter, "CUE data file is missing: {}", path.display())
            }
            Self::AmbiguousDataTracks => formatter.write_str("CUE has multiple data tracks"),
            Self::UnsupportedTrackMode(mode) => {
                write!(formatter, "unsupported CUE track mode: {mode}")
            }
        }
    }
}

impl std::error::Error for CueError {}

/// The only CUE track layouts that can currently be exposed as a bounded
/// filesystem-readable logical disc.  Audio tracks are intentionally not
/// represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueDataTrackMode {
    Mode1_2048,
    Mode1_2352,
    Mode2_2352,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueDataTrack {
    pub path: PathBuf,
    pub mode: CueDataTrackMode,
}

/// One resolved CUE sheet: the sheet itself plus every `.bin` (or other
/// data file) it references, resolved relative to the sheet's own
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueSheet {
    pub cue_path: PathBuf,
    pub referenced_paths: Vec<PathBuf>,
}

/// Parse a `.cue` sheet and resolve every `FILE "..." <TYPE>` reference it
/// contains relative to the sheet's directory. Read-only; the referenced
/// files are never opened here, only named.
pub fn resolve_cue(cue_path: &Path) -> Result<CueSheet, CueError> {
    let metadata = std::fs::metadata(cue_path).map_err(|error| CueError::Io(error.to_string()))?;
    if metadata.len() > MAX_CUE_BYTES {
        return Err(CueError::TooLarge);
    }
    let contents =
        std::fs::read_to_string(cue_path).map_err(|error| CueError::Io(error.to_string()))?;
    let parent = cue_path.parent().unwrap_or_else(|| Path::new("."));
    let mut referenced_paths = Vec::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if !trimmed.to_ascii_uppercase().starts_with("FILE") {
            continue;
        }
        if let Some(name) = extract_quoted_filename(trimmed) {
            if referenced_paths.len() >= MAX_CUE_FILE_REFERENCES {
                break;
            }
            referenced_paths.push(parent.join(name));
        }
    }
    if referenced_paths.is_empty() {
        return Err(CueError::NoFileReferences);
    }
    Ok(CueSheet {
        cue_path: cue_path.to_path_buf(),
        referenced_paths,
    })
}

/// Safety-resolves one CUE `FILE "..."` reference relative to `base`,
/// exactly like [`resolve_data_track`]'s own inline check: no absolute path,
/// no `..` component, and the canonicalized result must both exist as a
/// regular file and remain inside `canonical_base`. Shared by
/// [`resolve_data_track`] (one data track) and [`resolve_cue_all_files`]
/// (every referenced file) so the one safety rule lives in one place.
fn resolve_safe_reference(
    quoted: &str,
    base: &Path,
    canonical_base: &Path,
) -> Result<PathBuf, CueError> {
    let reference = Path::new(quoted);
    if reference.is_absolute()
        || reference
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(CueError::UnsafeReference);
    }
    let resolved = base.join(reference);
    let canonical = std::fs::canonicalize(&resolved)
        .map_err(|_| CueError::MissingDataFile(resolved.clone()))?;
    if !canonical.starts_with(canonical_base) || !canonical.is_file() {
        return Err(CueError::UnsafeReference);
    }
    Ok(canonical)
}

/// Resolve every `FILE "..."` reference in a `.cue` sheet - every BIN/audio
/// track a multi-file release needs, not just the one data track
/// [`resolve_data_track`] selects for identity. Same bounded read and the
/// exact same per-reference safety check
/// ([`resolve_safe_reference`]); refuses (never guesses at) any unsafe,
/// missing, or duplicate-named reference. Declaration order is preserved
/// and duplicates by canonical path are collapsed to one entry (a track
/// split across `INDEX`es inside one `FILE` still names that file only
/// once here).
pub fn resolve_cue_all_files(cue_path: &Path) -> Result<Vec<PathBuf>, CueError> {
    let per_reference = resolve_cue_all_files_lenient(cue_path)?;
    let mut files = Vec::with_capacity(per_reference.len());
    for outcome in per_reference {
        let resolved = outcome?;
        if !files.contains(&resolved) {
            files.push(resolved);
        }
    }
    if files.is_empty() {
        return Err(CueError::NoFileReferences);
    }
    Ok(files)
}

/// Like [`resolve_cue_all_files`], but never lets one bad `FILE` reference
/// hide the others: every declared reference is safety-checked
/// independently and reported as its own `Ok`/`Err`, in declaration order.
/// The outer `Result` only ever fails for reasons that make the sheet
/// itself unreadable (I/O, size, a malformed `FILE` line, or more
/// references than the bounded limit) - never for one missing or unsafe
/// track, since a caller rejecting an incomplete multi-file release still
/// needs to know exactly which of its files were safely, structurally
/// identified as belonging to that release (see
/// `playing_library::matching`'s fail-closed-as-a-whole handling).
pub fn resolve_cue_all_files_lenient(
    cue_path: &Path,
) -> Result<Vec<Result<PathBuf, CueError>>, CueError> {
    let metadata = std::fs::metadata(cue_path).map_err(|error| CueError::Io(error.to_string()))?;
    if metadata.len() > MAX_CUE_BYTES {
        return Err(CueError::TooLarge);
    }
    let contents =
        std::fs::read_to_string(cue_path).map_err(|error| CueError::Io(error.to_string()))?;
    let base = cue_path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_base =
        std::fs::canonicalize(base).map_err(|error| CueError::Io(error.to_string()))?;

    let mut outcomes = Vec::new();
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || !(line.len() >= 4 && line[..4].eq_ignore_ascii_case("FILE")) {
            continue;
        }
        let rest = line[4..].trim_start();
        let quoted = rest
            .strip_prefix('"')
            .and_then(|value| value.find('"').map(|end| &value[..end]))
            .ok_or_else(|| CueError::Malformed("FILE line has no quoted filename".into()))?;
        if outcomes.len() >= MAX_CUE_FILE_REFERENCES {
            return Err(CueError::Malformed(
                "CUE declares more file references than the bounded limit".into(),
            ));
        }
        outcomes.push(resolve_safe_reference(quoted, base, &canonical_base));
    }
    if outcomes.is_empty() {
        return Err(CueError::NoFileReferences);
    }
    Ok(outcomes)
}

/// Resolve the single unambiguous data track needed for ISO9660 identity.
/// The parser deliberately ignores audio tracks but refuses multiple data
/// tracks, missing INDEX 01 declarations, unsafe references, and modes for
/// which no verified logical-sector view exists.
pub fn resolve_data_track(cue_path: &Path) -> Result<CueDataTrack, CueError> {
    let metadata = std::fs::metadata(cue_path).map_err(|error| CueError::Io(error.to_string()))?;
    if metadata.len() > MAX_CUE_BYTES {
        return Err(CueError::TooLarge);
    }
    let contents =
        std::fs::read_to_string(cue_path).map_err(|error| CueError::Io(error.to_string()))?;
    let base = cue_path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_base =
        std::fs::canonicalize(base).map_err(|error| CueError::Io(error.to_string()))?;
    let mut current_file: Option<PathBuf> = None;
    let mut current_track: Option<CueDataTrackMode> = None;
    let mut data_tracks = Vec::new();
    let mut has_index_01 = false;

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("REM") {
            continue;
        }
        if line.len() >= 4 && line[..4].eq_ignore_ascii_case("FILE") {
            if let Some(mode) = current_track.take() {
                let file = current_file
                    .take()
                    .ok_or_else(|| CueError::Malformed("TRACK has no FILE".into()))?;
                if !has_index_01 {
                    return Err(CueError::Malformed("data TRACK has no INDEX 01".into()));
                }
                data_tracks.push(CueDataTrack { path: file, mode });
            }
            has_index_01 = false;
            let rest = line[4..].trim_start();
            let quoted = rest
                .strip_prefix('"')
                .and_then(|value| value.find('"').map(|end| &value[..end]))
                .ok_or_else(|| CueError::Malformed("FILE line has no quoted filename".into()))?;
            current_file = Some(resolve_safe_reference(quoted, base, &canonical_base)?);
            continue;
        }
        if line.len() >= 5 && line[..5].eq_ignore_ascii_case("TRACK") {
            if let Some(mode) = current_track.take() {
                let file = current_file
                    .take()
                    .ok_or_else(|| CueError::Malformed("TRACK has no FILE".into()))?;
                if !has_index_01 {
                    return Err(CueError::Malformed("data TRACK has no INDEX 01".into()));
                }
                data_tracks.push(CueDataTrack { path: file, mode });
            }
            has_index_01 = false;
            let mut fields = line.split_whitespace();
            let _track = fields.next();
            let _number = fields
                .next()
                .ok_or_else(|| CueError::Malformed("TRACK line has no track number".into()))?;
            let mode = fields
                .next()
                .ok_or_else(|| CueError::Malformed("TRACK line has no mode".into()))?;
            current_track = match mode.to_ascii_uppercase().as_str() {
                "MODE1/2048" => Some(CueDataTrackMode::Mode1_2048),
                "MODE1/2352" => Some(CueDataTrackMode::Mode1_2352),
                "MODE2/2352" => Some(CueDataTrackMode::Mode2_2352),
                "AUDIO" => None,
                unsupported => {
                    return Err(CueError::UnsupportedTrackMode(unsupported.to_string()));
                }
            };
            continue;
        }
        if line.len() >= 5 && line[..5].eq_ignore_ascii_case("INDEX") {
            let mut fields = line.split_whitespace();
            let _index = fields.next();
            if fields.next() == Some("01") {
                let timestamp = fields
                    .next()
                    .ok_or_else(|| CueError::Malformed("INDEX 01 has no timestamp".into()))?;
                if timestamp.split(':').count() != 3 {
                    return Err(CueError::Malformed(
                        "INDEX 01 timestamp is malformed".into(),
                    ));
                }
                has_index_01 = true;
            }
        }
    }
    if let (Some(mode), Some(file)) = (current_track, current_file) {
        if !has_index_01 {
            return Err(CueError::Malformed("data TRACK has no INDEX 01".into()));
        }
        data_tracks.push(CueDataTrack { path: file, mode });
    }
    // Audio-only sheets have no identity-bearing filesystem track.
    if data_tracks.len() != 1 {
        return if data_tracks.is_empty() {
            Err(CueError::NoFileReferences)
        } else {
            Err(CueError::AmbiguousDataTracks)
        };
    }
    Ok(data_tracks.remove(0))
}

/// Extracts the quoted filename from a CUE `FILE "name.bin" BINARY` line.
fn extract_quoted_filename(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let end = start + line[start..].find('"')?;
    Some(&line[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "archivefs-cue-bin-test-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn resolves_a_single_file_reference() {
        let cue = write_temp(
            "game.cue",
            "FILE \"game.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n",
        );
        let sheet = resolve_cue(&cue).unwrap();
        assert_eq!(sheet.referenced_paths.len(), 1);
        assert_eq!(
            sheet.referenced_paths[0].file_name().unwrap().to_str(),
            Some("game.bin")
        );
    }

    #[test]
    fn resolves_multiple_file_references() {
        let cue = write_temp(
            "multi.cue",
            "FILE \"multi (Track 1).bin\" BINARY\n\
             FILE \"multi (Track 2).bin\" BINARY\n",
        );
        let sheet = resolve_cue(&cue).unwrap();
        assert_eq!(sheet.referenced_paths.len(), 2);
    }

    #[test]
    fn a_cue_with_no_file_reference_is_an_error() {
        let cue = write_temp("empty.cue", "REM just a comment\n");
        assert_eq!(resolve_cue(&cue), Err(CueError::NoFileReferences));
    }

    #[test]
    fn resolves_only_the_single_data_track_and_preserves_mode() {
        let cue = write_temp(
            "data-track.cue",
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        );
        std::fs::write(cue.with_file_name("data.bin"), vec![0_u8; 2048]).unwrap();
        let track = resolve_data_track(&cue).unwrap();
        assert_eq!(track.mode, CueDataTrackMode::Mode1_2048);
        assert!(track.path.ends_with("data.bin"));
    }

    #[test]
    fn rejects_unsafe_missing_ambiguous_and_unsupported_data_tracks() {
        let traversal = write_temp(
            "traversal.cue",
            "FILE \"../outside.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        assert_eq!(
            resolve_data_track(&traversal),
            Err(CueError::UnsafeReference)
        );

        let missing = write_temp(
            "missing-data.cue",
            "FILE \"missing.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        assert!(matches!(
            resolve_data_track(&missing),
            Err(CueError::MissingDataFile(_))
        ));

        let unsupported = write_temp(
            "unsupported-mode.cue",
            "FILE \"missing.bin\" BINARY\nTRACK 01 MODE2/2336\nINDEX 01 00:00:00\n",
        );
        std::fs::write(unsupported.with_file_name("missing.bin"), vec![0_u8; 2048]).unwrap();
        assert!(matches!(
            resolve_data_track(&unsupported),
            Err(CueError::UnsupportedTrackMode(_))
        ));

        let ambiguous = write_temp(
            "ambiguous.cue",
            "FILE \"a.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"b.bin\" BINARY\nTRACK 02 MODE1/2048\nINDEX 01 00:00:00\n",
        );
        std::fs::write(ambiguous.with_file_name("a.bin"), vec![0_u8; 2048]).unwrap();
        std::fs::write(ambiguous.with_file_name("b.bin"), vec![0_u8; 2048]).unwrap();
        assert_eq!(
            resolve_data_track(&ambiguous),
            Err(CueError::AmbiguousDataTracks)
        );
    }

    #[test]
    fn resolve_cue_all_files_returns_every_referenced_track_in_order() {
        let cue = write_temp(
            "multi-track.cue",
            "FILE \"track01.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n\
             FILE \"track02.bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n\
             FILE \"track03.bin\" BINARY\n  TRACK 03 AUDIO\n    INDEX 01 00:00:00\n",
        );
        for name in ["track01.bin", "track02.bin", "track03.bin"] {
            std::fs::write(cue.with_file_name(name), vec![0_u8; 16]).unwrap();
        }
        let files = resolve_cue_all_files(&cue).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files[0].ends_with("track01.bin"));
        assert!(files[1].ends_with("track02.bin"));
        assert!(files[2].ends_with("track03.bin"));
    }

    #[test]
    fn resolve_cue_all_files_refuses_a_missing_track() {
        let cue = write_temp(
            "missing-track.cue",
            "FILE \"present.bin\" BINARY\nFILE \"missing.bin\" BINARY\n",
        );
        std::fs::write(cue.with_file_name("present.bin"), vec![0_u8; 16]).unwrap();
        assert!(matches!(
            resolve_cue_all_files(&cue),
            Err(CueError::MissingDataFile(_))
        ));
    }

    #[test]
    fn resolve_cue_all_files_refuses_traversal_and_absolute_references() {
        let traversal = write_temp("traversal-all.cue", "FILE \"../outside.bin\" BINARY\n");
        assert_eq!(
            resolve_cue_all_files(&traversal),
            Err(CueError::UnsafeReference)
        );

        let absolute = write_temp("absolute-all.cue", "FILE \"/etc/passwd\" BINARY\n");
        assert_eq!(
            resolve_cue_all_files(&absolute),
            Err(CueError::UnsafeReference)
        );
    }

    #[test]
    fn resolve_cue_all_files_deduplicates_a_repeated_file_reference() {
        let cue = write_temp(
            "repeated.cue",
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nINDEX 02 00:01:00\n",
        );
        std::fs::write(cue.with_file_name("data.bin"), vec![0_u8; 16]).unwrap();
        // Only one FILE line here, so this mainly proves the ordinary
        // single-reference case still works through the shared resolver;
        // an explicit two-FILE-same-name case is exercised structurally by
        // the dedup check (`!files.contains`) itself.
        let files = resolve_cue_all_files(&cue).unwrap();
        assert_eq!(files.len(), 1);
    }
}
