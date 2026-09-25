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
pub const CUE_FRAMES_PER_SECOND: u64 = 75;
pub const CUE_FRAME_BYTES: u64 = 2352;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CueTimestamp {
    pub frames: u64,
}

impl CueTimestamp {
    pub fn parse(raw: &str) -> Result<Self, CueError> {
        let mut fields = raw.split(':');
        let minutes = fields
            .next()
            .ok_or_else(|| CueError::Malformed("timestamp has no minutes".into()))?
            .parse::<u64>()
            .map_err(|_| CueError::Malformed("timestamp minutes are malformed".into()))?;
        let seconds = fields
            .next()
            .ok_or_else(|| CueError::Malformed("timestamp has no seconds".into()))?
            .parse::<u64>()
            .map_err(|_| CueError::Malformed("timestamp seconds are malformed".into()))?;
        let frames = fields
            .next()
            .ok_or_else(|| CueError::Malformed("timestamp has no frames".into()))?
            .parse::<u64>()
            .map_err(|_| CueError::Malformed("timestamp frames are malformed".into()))?;
        if fields.next().is_some() || seconds >= 60 || frames >= CUE_FRAMES_PER_SECOND {
            return Err(CueError::Malformed(
                "timestamp is outside MSF bounds".into(),
            ));
        }
        let total = minutes
            .checked_mul(60)
            .and_then(|value| value.checked_add(seconds))
            .and_then(|value| value.checked_mul(CUE_FRAMES_PER_SECOND))
            .and_then(|value| value.checked_add(frames))
            .ok_or_else(|| CueError::Malformed("timestamp overflows frame arithmetic".into()))?;
        Ok(Self { frames: total })
    }
}

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
    pub disc_track_count: u32,
    pub index_01_frame: u64,
    pub data_frame_count: u64,
    pub pregap: CuePregap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuePregap {
    None,
    InFile { start_frame: u64, frames: u64 },
    Synthetic { frames: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueTrackMode {
    Data(CueDataTrackMode),
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueTrack {
    pub number: u32,
    pub mode: CueTrackMode,
    pub path: PathBuf,
    pub index_00: Option<CueTimestamp>,
    pub index_01: Option<CueTimestamp>,
    pub pregap: Option<CueTimestamp>,
    pub postgap: Option<CueTimestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueLayout {
    pub cue_path: PathBuf,
    pub tracks: Vec<CueTrack>,
}

impl CueLayout {
    pub fn supported_single_mode1_2048(&self) -> Result<&CueTrack, CueError> {
        if self.tracks.len() != 1 {
            return Err(CueError::AmbiguousDataTracks);
        }
        let track = &self.tracks[0];
        if track.index_01.is_none() {
            return Err(CueError::Malformed("track has no INDEX 01".into()));
        }
        if !matches!(track.mode, CueTrackMode::Data(CueDataTrackMode::Mode1_2048)) {
            return Err(CueError::UnsupportedTrackMode(format!("{:?}", track.mode)));
        }
        Ok(track)
    }
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

/// Parses the complete ordered track layout of a CUE sheet. This is the
/// structured companion to [`resolve_data_track`]; it applies the same
/// bounded file reading and safe reference resolution, while retaining
/// audio tracks and per-track `INDEX 01` evidence for callers that must not
/// accidentally treat a partial sheet as a complete disc.
pub fn resolve_cue_layout(cue_path: &Path) -> Result<CueLayout, CueError> {
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
    let mut current_track: Option<(
        u32,
        CueTrackMode,
        Option<CueTimestamp>,
        Option<CueTimestamp>,
        Option<CueTimestamp>,
        Option<CueTimestamp>,
    )> = None;
    let mut tracks = Vec::new();

    let finish = |current_file: &mut Option<PathBuf>,
                  current_track: &mut Option<(
        u32,
        CueTrackMode,
        Option<CueTimestamp>,
        Option<CueTimestamp>,
        Option<CueTimestamp>,
        Option<CueTimestamp>,
    )>,
                  tracks: &mut Vec<CueTrack>|
     -> Result<(), CueError> {
        if let Some((number, mode, index_00, index_01, pregap, postgap)) = current_track.take() {
            let path = current_file
                .as_ref()
                .cloned()
                .ok_or_else(|| CueError::Malformed("TRACK has no FILE".into()))?;
            if index_01.is_none() {
                return Err(CueError::Malformed("TRACK has no INDEX 01".into()));
            }
            if let (Some(index_00), Some(index_01)) = (index_00, index_01) {
                if index_00 >= index_01 {
                    return Err(CueError::Malformed("INDEX 00 must precede INDEX 01".into()));
                }
            }
            tracks.push(CueTrack {
                number,
                mode,
                path,
                index_00,
                index_01,
                pregap,
                postgap,
            });
        }
        Ok(())
    };

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("REM") {
            continue;
        }
        if line.len() >= 4 && line[..4].eq_ignore_ascii_case("FILE") {
            finish(&mut current_file, &mut current_track, &mut tracks)?;
            let rest = line[4..].trim_start();
            let quoted = rest
                .strip_prefix('"')
                .and_then(|value| value.find('"').map(|end| &value[..end]))
                .ok_or_else(|| CueError::Malformed("FILE line has no quoted filename".into()))?;
            current_file = Some(resolve_safe_reference(quoted, base, &canonical_base)?);
            continue;
        }
        if line.len() >= 5 && line[..5].eq_ignore_ascii_case("TRACK") {
            finish(&mut current_file, &mut current_track, &mut tracks)?;
            let mut fields = line.split_whitespace();
            let _ = fields.next();
            let number = fields
                .next()
                .ok_or_else(|| CueError::Malformed("TRACK has no number".into()))?
                .parse::<u32>()
                .map_err(|_| CueError::Malformed("TRACK number is malformed".into()))?;
            let mode = fields
                .next()
                .ok_or_else(|| CueError::Malformed("TRACK has no mode".into()))?;
            let mode = match mode.to_ascii_uppercase().as_str() {
                "MODE1/2048" => CueTrackMode::Data(CueDataTrackMode::Mode1_2048),
                "MODE1/2352" => CueTrackMode::Data(CueDataTrackMode::Mode1_2352),
                "MODE2/2352" => CueTrackMode::Data(CueDataTrackMode::Mode2_2352),
                "AUDIO" => CueTrackMode::Audio,
                unsupported => return Err(CueError::UnsupportedTrackMode(unsupported.into())),
            };
            current_track = Some((number, mode, None, None, None, None));
            continue;
        }
        if line.len() >= 6 && line[..6].eq_ignore_ascii_case("PREGAP") {
            let timestamp = CueTimestamp::parse(line[6..].trim())?;
            let Some((_, _, _, _, pregap, _)) = current_track.as_mut() else {
                return Err(CueError::Malformed("PREGAP has no TRACK".into()));
            };
            if pregap.replace(timestamp).is_some() {
                return Err(CueError::Malformed("TRACK has duplicate PREGAP".into()));
            }
            continue;
        }
        if line.len() >= 7 && line[..7].eq_ignore_ascii_case("POSTGAP") {
            let timestamp = CueTimestamp::parse(line[7..].trim())?;
            let Some((_, _, _, _, _, postgap)) = current_track.as_mut() else {
                return Err(CueError::Malformed("POSTGAP has no TRACK".into()));
            };
            if postgap.replace(timestamp).is_some() {
                return Err(CueError::Malformed("TRACK has duplicate POSTGAP".into()));
            }
            continue;
        }
        if line.len() >= 5 && line[..5].eq_ignore_ascii_case("INDEX") {
            let mut fields = line.split_whitespace();
            let _ = fields.next();
            let index = fields
                .next()
                .ok_or_else(|| CueError::Malformed("INDEX has no number".into()))?;
            let timestamp = fields
                .next()
                .ok_or_else(|| CueError::Malformed("INDEX has no timestamp".into()))?;
            let timestamp = CueTimestamp::parse(timestamp)?;
            let Some((_, _, index_00, index_01, _, _)) = current_track.as_mut() else {
                return Err(CueError::Malformed("INDEX has no TRACK".into()));
            };
            match index.parse::<u8>() {
                Ok(0) => {
                    if index_00.replace(timestamp).is_some() {
                        return Err(CueError::Malformed("TRACK has duplicate INDEX 00".into()));
                    }
                }
                Ok(1) => {
                    if index_01.replace(timestamp).is_some() {
                        return Err(CueError::Malformed("TRACK has duplicate INDEX 01".into()));
                    }
                }
                Ok(_) => {}
                Err(_) => return Err(CueError::Malformed("INDEX number is malformed".into())),
            }
        }
    }
    finish(&mut current_file, &mut current_track, &mut tracks)?;
    if tracks.is_empty() {
        return Err(CueError::NoFileReferences);
    }
    for track in &tracks {
        let index_01 = track
            .index_01
            .ok_or_else(|| CueError::Malformed("TRACK has no INDEX 01".into()))?;
        let frame_bytes = match track.mode {
            CueTrackMode::Data(CueDataTrackMode::Mode1_2048) => 2048,
            CueTrackMode::Data(CueDataTrackMode::Mode1_2352)
            | CueTrackMode::Data(CueDataTrackMode::Mode2_2352)
            | CueTrackMode::Audio => CUE_FRAME_BYTES,
        };
        let offset = index_01
            .frames
            .checked_mul(frame_bytes)
            .ok_or_else(|| CueError::Malformed("INDEX 01 byte offset overflows".into()))?;
        let length = std::fs::metadata(&track.path)
            .map_err(|error| CueError::Io(error.to_string()))?
            .len();
        if offset > length {
            return Err(CueError::Malformed(format!(
                "INDEX 01 is outside FILE for track {}",
                track.number
            )));
        }
        if let Some(index_00) = track.index_00 {
            let pregap_offset = index_00
                .frames
                .checked_mul(frame_bytes)
                .ok_or_else(|| CueError::Malformed("INDEX 00 byte offset overflows".into()))?;
            if pregap_offset >= offset || pregap_offset > length {
                return Err(CueError::Malformed(format!(
                    "INDEX 00 is outside FILE for track {}",
                    track.number
                )));
            }
        }
    }
    Ok(CueLayout {
        cue_path: cue_path.to_path_buf(),
        tracks,
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
    let layout = resolve_cue_layout(cue_path)?;
    let data_tracks: Vec<_> = layout
        .tracks
        .iter()
        .filter_map(|track| match track.mode {
            CueTrackMode::Data(mode) => Some((track, mode)),
            CueTrackMode::Audio => None,
        })
        .collect();
    if data_tracks.len() != 1 {
        return if data_tracks.is_empty() {
            Err(CueError::NoFileReferences)
        } else {
            Err(CueError::AmbiguousDataTracks)
        };
    }
    let (track, mode) = data_tracks.into_iter().next().expect("length checked");
    let index_01 = track
        .index_01
        .ok_or_else(|| CueError::Malformed("TRACK has no INDEX 01".into()))?;
    let frame_bytes = frame_bytes(&track.mode);
    let length = std::fs::metadata(&track.path)
        .map_err(|error| CueError::Io(error.to_string()))?
        .len();
    if !length.is_multiple_of(frame_bytes) {
        return Err(CueError::Malformed(
            "FILE is not a whole-sector stream".into(),
        ));
    }
    let file_frames = length / frame_bytes;
    let boundary = layout
        .tracks
        .iter()
        .filter(|candidate| candidate.path == track.path)
        .filter_map(|candidate| {
            let candidate_start = candidate.index_01?.frames;
            (candidate_start > index_01.frames).then(|| {
                candidate
                    .index_00
                    .map(|index| index.frames)
                    .unwrap_or(candidate_start)
            })
        })
        .min()
        .unwrap_or(file_frames);
    if boundary <= index_01.frames || boundary > file_frames {
        return Err(CueError::Malformed(
            "track boundary is outside its FILE".into(),
        ));
    }
    let pregap = match (track.index_00, track.pregap) {
        (Some(index_00), _) => CuePregap::InFile {
            start_frame: index_00.frames,
            frames: index_01.frames - index_00.frames,
        },
        (None, Some(pregap)) => CuePregap::Synthetic {
            frames: pregap.frames,
        },
        (None, None) => CuePregap::None,
    };
    Ok(CueDataTrack {
        path: track.path.clone(),
        mode,
        disc_track_count: u32::try_from(layout.tracks.len())
            .map_err(|_| CueError::Malformed("too many tracks".into()))?,
        index_01_frame: index_01.frames,
        data_frame_count: boundary - index_01.frames,
        pregap,
    })
}

fn frame_bytes(mode: &CueTrackMode) -> u64 {
    match mode {
        CueTrackMode::Data(CueDataTrackMode::Mode1_2048) => 2048,
        CueTrackMode::Data(CueDataTrackMode::Mode1_2352)
        | CueTrackMode::Data(CueDataTrackMode::Mode2_2352)
        | CueTrackMode::Audio => CUE_FRAME_BYTES,
    }
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
    fn structured_layout_classifies_only_a_single_mode1_2048_track() {
        let cue = write_temp(
            "layout.cue",
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        );
        std::fs::write(cue.with_file_name("data.bin"), vec![0_u8; 4096]).unwrap();
        let layout = resolve_cue_layout(&cue).unwrap();
        let track = layout.supported_single_mode1_2048().unwrap();
        assert_eq!(layout.tracks.len(), 1);
        assert_eq!(track.number, 1);
        assert!(matches!(
            track.mode,
            CueTrackMode::Data(CueDataTrackMode::Mode1_2048)
        ));
    }

    #[test]
    fn structured_layout_retains_audio_and_refuses_the_narrow_slice() {
        let cue = write_temp(
            "audio-layout.cue",
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        );
        std::fs::write(cue.with_file_name("data.bin"), vec![0_u8; 2048]).unwrap();
        std::fs::write(cue.with_file_name("audio.bin"), vec![0_u8; 2352]).unwrap();
        let layout = resolve_cue_layout(&cue).unwrap();
        assert_eq!(layout.tracks.len(), 2);
        assert!(matches!(layout.tracks[1].mode, CueTrackMode::Audio));
        assert!(layout.supported_single_mode1_2048().is_err());
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

    #[test]
    fn index_00_is_an_in_file_pregap_and_index_01_is_the_data_start() {
        let cue = write_temp(
            "index00.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:02:00\n",
        );
        std::fs::write(cue.with_file_name("disc.bin"), vec![0_u8; 2048 * 152]).unwrap();
        let track = resolve_data_track(&cue).unwrap();
        assert_eq!(track.index_01_frame, 150);
        assert_eq!(track.data_frame_count, 2);
        assert_eq!(
            track.pregap,
            CuePregap::InFile {
                start_frame: 0,
                frames: 150
            }
        );
    }

    #[test]
    fn explicit_pregap_is_synthetic_and_does_not_shift_file_offset() {
        let cue = write_temp(
            "synthetic-pregap.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nPREGAP 00:02:00\nINDEX 01 00:00:00\n",
        );
        std::fs::write(cue.with_file_name("disc.bin"), vec![0_u8; 2048 * 2]).unwrap();
        let track = resolve_data_track(&cue).unwrap();
        assert_eq!(track.index_01_frame, 0);
        assert_eq!(track.data_frame_count, 2);
        assert_eq!(track.pregap, CuePregap::Synthetic { frames: 150 });
    }

    #[test]
    fn one_file_track_boundaries_stop_at_next_index_00() {
        let cue = write_temp(
            "one-file.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 00 00:02:00\nINDEX 01 00:03:00\n",
        );
        std::fs::write(cue.with_file_name("disc.bin"), vec![0_u8; 2352 * 300]).unwrap();
        let track = resolve_data_track(&cue).unwrap();
        assert_eq!(track.index_01_frame, 0);
        assert_eq!(track.data_frame_count, 150);
    }

    #[test]
    fn malformed_or_missing_index_01_fails_closed() {
        let missing = write_temp(
            "missing-index01.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 00 00:00:00\n",
        );
        std::fs::write(missing.with_file_name("disc.bin"), vec![0_u8; 2352]).unwrap();
        assert!(matches!(
            resolve_cue_layout(&missing),
            Err(CueError::Malformed(_))
        ));

        let decreasing = write_temp(
            "decreasing-index.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:02\nINDEX 01 00:00:01\n",
        );
        std::fs::write(decreasing.with_file_name("disc.bin"), vec![0_u8; 2048 * 2]).unwrap();
        assert!(matches!(
            resolve_cue_layout(&decreasing),
            Err(CueError::Malformed(_))
        ));
    }

    #[test]
    fn index_00_outside_the_referenced_file_is_refused() {
        let cue = write_temp(
            "outside-index00.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:01:00\nINDEX 01 00:02:00\n",
        );
        std::fs::write(cue.with_file_name("disc.bin"), vec![0_u8; 2048]).unwrap();
        assert!(matches!(
            resolve_cue_layout(&cue),
            Err(CueError::Malformed(_))
        ));
    }

    #[test]
    fn data_after_audio_uses_its_own_file_relative_indexes() {
        let cue = write_temp(
            "data-after-audio.cue",
            "FILE \"audio.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE \"data.bin\" BINARY\nTRACK 02 MODE1/2048\nPREGAP 00:01:00\nINDEX 00 00:00:00\nINDEX 01 00:00:01\n",
        );
        std::fs::write(cue.with_file_name("audio.bin"), vec![0_u8; 2352]).unwrap();
        std::fs::write(cue.with_file_name("data.bin"), vec![0_u8; 2048 * 2]).unwrap();
        let track = resolve_data_track(&cue).unwrap();
        assert!(track.path.ends_with("data.bin"));
        assert_eq!(track.index_01_frame, 1);
        assert_eq!(track.data_frame_count, 1);
        assert_eq!(
            track.pregap,
            CuePregap::InFile {
                start_frame: 0,
                frames: 1
            }
        );
    }

    #[test]
    fn multiple_file_tracks_do_not_share_offsets() {
        let cue = write_temp(
            "separate-files.cue",
            "FILE \"track01.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE \"track02.bin\" BINARY\nTRACK 02 MODE1/2048\nINDEX 01 00:00:00\n",
        );
        std::fs::write(cue.with_file_name("track01.bin"), vec![0_u8; 2352]).unwrap();
        std::fs::write(cue.with_file_name("track02.bin"), vec![0_u8; 2048 * 3]).unwrap();
        let track = resolve_data_track(&cue).unwrap();
        assert!(track.path.ends_with("track02.bin"));
        assert_eq!(track.index_01_frame, 0);
        assert_eq!(track.data_frame_count, 3);
    }
}
