//! Dreamcast `.gdi` (GD-ROM descriptor) parsing: bounded, safe resolution
//! of the single high-density data track needed for identity.
//!
//! Mirrors [`super::cue_bin`]'s own read-only, size-bounded,
//! symlink/traversal-safe design exactly - see that module's own doc
//! comment for the shared rationale (`.gdi` is likewise always small plain
//! text, so a file above [`MAX_GDI_BYTES`] is refused rather than read; a
//! referenced track file is resolved only relative to the descriptor's own
//! directory, and the same `canonicalize` + `starts_with` check that
//! catches both `..`-traversal and symlink-escape for CUE is reused here
//! verbatim).
//!
//! # Why data-track selection is GDI-specific
//!
//! Unlike a CUE sheet (exactly one data track by construction) or a plain
//! CHD (opaque track metadata, handled by
//! [`crate::chd_identity::select_candidate_data_track`]'s own
//! lowest-numbered-non-audio heuristic - which that module's own doc
//! comment documents as *wrong* for a real GD-ROM), a `.gdi` descriptor
//! states every track's exact starting LBA directly. A real Dreamcast disc
//! always has a small CD-compatible "low-density" track first (a
//! warning-text/audio area, never the real game) followed by the actual
//! game data in the "high-density" area starting at the documented
//! [`crate::chd_identity::GDROM_HIGH_DENSITY_START_FRAME`] boundary - the
//! same constant CHD's own specialist-backend detection already cites.
//! Reusing it here means GDI's own descriptor data is enough to correctly
//! identify the real data track without any specialist backend or
//! filename guess at all.

use std::path::{Component, Path, PathBuf};

use crate::chd_identity::GDROM_HIGH_DENSITY_START_FRAME;

/// `.gdi` descriptors are a handful of short lines - a few hundred bytes
/// even for a many-track disc. Refuse anything larger as not a genuine GDI
/// descriptor rather than reading it.
const MAX_GDI_BYTES: u64 = 64 * 1024;

/// A GDI track line's fields (number, LBA, type, sector size, filename,
/// trailing field) never need to be long; a quoted filename is the only
/// variable-length one.
const MAX_GDI_LINE_BYTES: usize = 512;

/// Real Dreamcast dumps have 2-3 tracks (one low-density, one or more
/// high-density). Matches [`super::cue_bin::MAX_CUE_FILE_REFERENCES`]'s
/// bound in spirit - generous for any legitimate disc, finite against a
/// hostile descriptor.
const MAX_GDI_TRACKS: usize = 99;

/// A real GD-ROM's total addressable range is far under this. Refuses a
/// deliberately huge LBA rather than trusting it verbatim.
const MAX_GDI_LBA: u32 = 10_000_000;

/// The GDI track "type" field value for a data track (CD-ROM Mode 1/2).
const GDI_TRACK_TYPE_DATA: u32 = 4;
/// The GDI track "type" field value for a 2-channel audio track.
const GDI_TRACK_TYPE_AUDIO: u32 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GdiError {
    Io(String),
    TooLarge,
    Malformed(String),
    UnsafeReference,
    MissingTrackFile(PathBuf),
    DuplicateTrackNumber(u32),
    DuplicateStartLba,
    UnsupportedSectorSize(u32),
    NoDataTrack,
    AmbiguousDataTracks,
}

impl std::fmt::Display for GdiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "GDI I/O error: {error}"),
            Self::TooLarge => formatter.write_str("GDI descriptor exceeds the bounded size limit"),
            Self::Malformed(detail) => write!(formatter, "malformed GDI descriptor: {detail}"),
            Self::UnsafeReference => formatter.write_str("GDI references an unsafe track path"),
            Self::MissingTrackFile(path) => {
                write!(formatter, "GDI track file is missing: {}", path.display())
            }
            Self::DuplicateTrackNumber(number) => {
                write!(formatter, "GDI descriptor has duplicate track number {number}")
            }
            Self::DuplicateStartLba => {
                formatter.write_str("GDI descriptor has two tracks with the same starting LBA")
            }
            Self::UnsupportedSectorSize(size) => {
                write!(formatter, "unsupported GDI track sector size: {size}")
            }
            Self::NoDataTrack => formatter.write_str(
                "GDI descriptor has no high-density data track at or beyond the documented GD-ROM boundary",
            ),
            Self::AmbiguousDataTracks => {
                formatter.write_str("GDI descriptor has more than one high-density data track")
            }
        }
    }
}

impl std::error::Error for GdiError {}

/// The only GDI data-track sector layouts that can currently be exposed as
/// a bounded, filesystem-readable logical disc. Audio tracks are
/// intentionally not represented here - see the module doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdiDataTrackMode {
    Cooked2048,
    Raw2352,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdiDataTrack {
    pub path: PathBuf,
    pub mode: GdiDataTrackMode,
}

struct GdiTrackLine {
    number: u32,
    start_lba: u32,
    track_type: u32,
    filename: String,
    sector_size: u32,
}

/// Reads, bounds, and validates a `.gdi` descriptor's track lines -
/// exactly the shared prefix [`resolve_gdi_data_track`] and
/// [`resolve_gdi_all_tracks`] both need: header parsing, per-line length
/// bounds, duplicate-track-number, and duplicate-start-LBA checks. Neither
/// caller re-parses anything; they only interpret this same validated
/// `Vec<GdiTrackLine>` differently (one selects the identity track, the
/// other resolves every track's own file).
fn parse_and_validate_gdi_tracks(
    gdi_path: &Path,
) -> Result<(Vec<GdiTrackLine>, PathBuf, PathBuf), GdiError> {
    let metadata = std::fs::metadata(gdi_path).map_err(|error| GdiError::Io(error.to_string()))?;
    if metadata.len() > MAX_GDI_BYTES {
        return Err(GdiError::TooLarge);
    }
    let contents =
        std::fs::read_to_string(gdi_path).map_err(|error| GdiError::Io(error.to_string()))?;
    let base = gdi_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let canonical_base =
        std::fs::canonicalize(&base).map_err(|error| GdiError::Io(error.to_string()))?;

    let mut lines = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let header = lines
        .next()
        .ok_or_else(|| GdiError::Malformed("descriptor is empty".to_string()))?;
    if header.len() > MAX_GDI_LINE_BYTES {
        return Err(GdiError::Malformed("header line is too long".to_string()));
    }
    let track_count: usize = header
        .parse()
        .map_err(|_| GdiError::Malformed("first line is not a track count".to_string()))?;
    if track_count == 0 || track_count > MAX_GDI_TRACKS {
        return Err(GdiError::Malformed(
            "declared track count is out of bounds".to_string(),
        ));
    }

    let mut tracks = Vec::with_capacity(track_count);
    for _ in 0..track_count {
        let line = lines.next().ok_or_else(|| {
            GdiError::Malformed("descriptor has fewer track lines than declared".to_string())
        })?;
        if line.len() > MAX_GDI_LINE_BYTES {
            return Err(GdiError::Malformed("track line is too long".to_string()));
        }
        tracks.push(parse_gdi_track_line(line, track_count)?);
    }
    if lines.next().is_some() {
        return Err(GdiError::Malformed(
            "descriptor has more track lines than declared".to_string(),
        ));
    }

    // Every track number must appear exactly once, covering exactly
    // 1..=track_count - anything else is an inconsistent/ambiguous
    // descriptor, refused before any track is trusted.
    let mut seen_numbers = std::collections::BTreeSet::new();
    for track in &tracks {
        if !seen_numbers.insert(track.number) {
            return Err(GdiError::DuplicateTrackNumber(track.number));
        }
    }
    if !(1..=track_count as u32).all(|number| seen_numbers.contains(&number)) {
        return Err(GdiError::Malformed(
            "track numbers do not cover 1..=declared track count".to_string(),
        ));
    }

    // Two tracks can never legitimately start at the same LBA - the
    // determinable overlap check available without opening any track file.
    let mut start_lbas: Vec<u32> = tracks.iter().map(|track| track.start_lba).collect();
    start_lbas.sort_unstable();
    if start_lbas.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(GdiError::DuplicateStartLba);
    }

    Ok((tracks, base, canonical_base))
}

/// Safety-resolves one GDI track's filename relative to `base`, exactly
/// like [`resolve_gdi_data_track`]'s own inline check: no absolute path,
/// no `..` component, and the canonicalized result must both exist as a
/// regular file and remain inside `canonical_base`.
fn resolve_gdi_track_file(
    filename: &str,
    base: &Path,
    canonical_base: &Path,
) -> Result<PathBuf, GdiError> {
    let reference = Path::new(filename);
    if reference.is_absolute()
        || reference
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(GdiError::UnsafeReference);
    }
    let resolved = base.join(reference);
    let canonical = std::fs::canonicalize(&resolved)
        .map_err(|_| GdiError::MissingTrackFile(resolved.clone()))?;
    if !canonical.starts_with(canonical_base) || !canonical.is_file() {
        return Err(GdiError::UnsafeReference);
    }
    Ok(canonical)
}

/// Parses a `.gdi` descriptor and resolves the single unambiguous
/// high-density data track needed for identity, relative to the
/// descriptor's own directory. The parser deliberately ignores low-density
/// and audio tracks, and refuses ambiguous selections, unsafe references,
/// duplicate/inconsistent track metadata, and sector sizes for which no
/// verified logical-sector view exists.
pub fn resolve_gdi_data_track(gdi_path: &Path) -> Result<GdiDataTrack, GdiError> {
    let (tracks, base, canonical_base) = parse_and_validate_gdi_tracks(gdi_path)?;

    // Data-track selection: metadata only, never filename or track order -
    // see the module doc comment for the exact GD-ROM boundary this reuses.
    let mut candidates = tracks
        .iter()
        .filter(|track| {
            track.track_type == GDI_TRACK_TYPE_DATA
                && track.start_lba >= GDROM_HIGH_DENSITY_START_FRAME
        })
        .collect::<Vec<_>>();
    let track = match candidates.len() {
        0 => return Err(GdiError::NoDataTrack),
        1 => candidates.remove(0),
        _ => return Err(GdiError::AmbiguousDataTracks),
    };

    let mode = match track.sector_size {
        2048 => GdiDataTrackMode::Cooked2048,
        2352 => GdiDataTrackMode::Raw2352,
        other => return Err(GdiError::UnsupportedSectorSize(other)),
    };

    let canonical = resolve_gdi_track_file(&track.filename, &base, &canonical_base)?;

    Ok(GdiDataTrack {
        path: canonical,
        mode,
    })
}

/// Resolves every track file a `.gdi` descriptor references - low-density,
/// high-density, and audio alike - the complete file set a multi-track
/// GD-ROM release needs, not just the one identity track
/// [`resolve_gdi_data_track`] selects. Reuses the exact same descriptor
/// parsing/validation ([`parse_and_validate_gdi_tracks`]) and the exact
/// same per-file safety check ([`resolve_gdi_track_file`]); refuses (never
/// guesses at) any unsafe, missing, or malformed reference exactly as
/// [`resolve_gdi_data_track`] does. Declaration order is preserved and
/// duplicates by canonical path are collapsed to one entry.
pub fn resolve_gdi_all_tracks(gdi_path: &Path) -> Result<Vec<PathBuf>, GdiError> {
    let per_track = resolve_gdi_all_tracks_lenient(gdi_path)?;
    let mut files = Vec::with_capacity(per_track.len());
    for outcome in per_track {
        let resolved = outcome?;
        if !files.contains(&resolved) {
            files.push(resolved);
        }
    }
    Ok(files)
}

/// Like [`resolve_gdi_all_tracks`], but never lets one bad track hide the
/// others: every declared track's file is safety-checked independently and
/// reported as its own `Ok`/`Err`, in declaration order. The outer
/// `Result` only ever fails for reasons that make the descriptor itself
/// unreadable (I/O, size, header/count/duplicate-number/duplicate-LBA
/// problems) - never for one missing or unsafe track file, since a caller
/// rejecting an incomplete multi-file release still needs to know exactly
/// which of its files were safely, structurally identified as belonging to
/// that release (see `playing_library::matching`'s fail-closed-as-a-whole
/// handling).
pub fn resolve_gdi_all_tracks_lenient(
    gdi_path: &Path,
) -> Result<Vec<Result<PathBuf, GdiError>>, GdiError> {
    let (tracks, base, canonical_base) = parse_and_validate_gdi_tracks(gdi_path)?;
    Ok(tracks
        .iter()
        .map(|track| resolve_gdi_track_file(&track.filename, &base, &canonical_base))
        .collect())
}

fn parse_gdi_track_line(line: &str, track_count: usize) -> Result<GdiTrackLine, GdiError> {
    let mut rest = line;
    let number = take_uint_field(&mut rest)?;
    let start_lba = take_uint_field(&mut rest)?;
    let track_type = take_uint_field(&mut rest)?;
    let sector_size = take_uint_field(&mut rest)?;
    rest = rest.trim_start();
    let filename = take_filename_field(&mut rest)?;
    let trailing = rest.trim();
    if trailing.is_empty() {
        return Err(GdiError::Malformed(
            "track line has no trailing field".to_string(),
        ));
    }
    let _trailing: i64 = trailing
        .parse()
        .map_err(|_| GdiError::Malformed("track line trailing field is not numeric".to_string()))?;

    if number == 0 || number as usize > track_count {
        return Err(GdiError::Malformed(
            "track number is out of the declared range".to_string(),
        ));
    }
    if start_lba > MAX_GDI_LBA {
        return Err(GdiError::Malformed(
            "track starting LBA is out of bounds".to_string(),
        ));
    }
    if track_type != GDI_TRACK_TYPE_DATA && track_type != GDI_TRACK_TYPE_AUDIO {
        return Err(GdiError::Malformed(format!(
            "unsupported track type {track_type}"
        )));
    }
    if filename.is_empty() || filename.chars().any(|c| c.is_control()) {
        return Err(GdiError::Malformed(
            "track line has no usable filename".to_string(),
        ));
    }

    Ok(GdiTrackLine {
        number,
        start_lba,
        track_type,
        filename,
        sector_size,
    })
}

/// Consumes one whitespace-delimited numeric field from the front of
/// `rest`, advancing past it (and the whitespace that follows).
fn take_uint_field(rest: &mut &str) -> Result<u32, GdiError> {
    *rest = rest.trim_start();
    let end = rest
        .find(char::is_whitespace)
        .ok_or_else(|| GdiError::Malformed("track line is truncated".to_string()))?;
    let (field, remainder) = rest.split_at(end);
    *rest = remainder;
    field
        .parse()
        .map_err(|_| GdiError::Malformed(format!("expected a number, got `{field}`")))
}

/// Consumes the filename field, which may be a bare token or a
/// double-quoted string (real-world dumps quote a filename containing
/// spaces, mirroring CUE's own `FILE "..."` convention).
fn take_filename_field(rest: &mut &str) -> Result<String, GdiError> {
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped
            .find('"')
            .ok_or_else(|| GdiError::Malformed("unterminated quoted filename".to_string()))?;
        let filename = stripped[..end].to_string();
        *rest = &stripped[end + 1..];
        Ok(filename)
    } else {
        let end = rest
            .find(char::is_whitespace)
            .ok_or_else(|| GdiError::Malformed("track line is truncated".to_string()))?;
        let (field, remainder) = rest.split_at(end);
        *rest = remainder;
        Ok(field.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "archivefs-gdi-test-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn write_sibling(gdi_path: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = gdi_path.with_file_name(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn normal_gdi(data_filename: &str, sector_size: u32) -> String {
        format!(
            "3\n\
             1 0 4 2352 track01.bin 0\n\
             2 600 0 2352 track02.raw 0\n\
             3 45000 4 {sector_size} {data_filename} 0\n"
        )
    }

    #[test]
    fn resolves_the_high_density_data_track_and_ignores_low_density_and_audio() {
        let gdi = write_temp("normal.gdi", &normal_gdi("track03.bin", 2352));
        write_sibling(&gdi, "track01.bin", &vec![0_u8; 2352]);
        write_sibling(&gdi, "track02.raw", &vec![0_u8; 2352]);
        write_sibling(&gdi, "track03.bin", &vec![0_u8; 2352 * 2]);
        let track = resolve_gdi_data_track(&gdi).unwrap();
        assert_eq!(track.mode, GdiDataTrackMode::Raw2352);
        assert!(track.path.ends_with("track03.bin"));
    }

    #[test]
    fn cooked_2048_sector_size_is_supported() {
        let gdi = write_temp("cooked.gdi", &normal_gdi("track03.iso", 2048));
        write_sibling(&gdi, "track01.bin", &vec![0_u8; 2352]);
        write_sibling(&gdi, "track02.raw", &vec![0_u8; 2352]);
        write_sibling(&gdi, "track03.iso", &vec![0_u8; 2048 * 2]);
        let track = resolve_gdi_data_track(&gdi).unwrap();
        assert_eq!(track.mode, GdiDataTrackMode::Cooked2048);
    }

    #[test]
    fn filename_disagreement_with_convention_is_irrelevant() {
        // Named nothing like "track03.bin" - selection is by metadata only.
        let gdi = write_temp(
            "renamed.gdi",
            "2\n1 0 4 2352 lowdensity.bin 0\n2 45000 4 2352 totally_unrelated_name.dat 0\n",
        );
        write_sibling(&gdi, "lowdensity.bin", &vec![0_u8; 2352]);
        write_sibling(&gdi, "totally_unrelated_name.dat", &vec![0_u8; 2352 * 2]);
        let track = resolve_gdi_data_track(&gdi).unwrap();
        assert!(track.path.ends_with("totally_unrelated_name.dat"));
    }

    #[test]
    fn malformed_descriptor_refuses() {
        let gdi = write_temp("malformed.gdi", "not-a-number\n");
        assert!(matches!(
            resolve_gdi_data_track(&gdi),
            Err(GdiError::Malformed(_))
        ));
    }

    #[test]
    fn missing_track_file_refuses() {
        let gdi = write_temp("missing.gdi", "1\n1 45000 4 2352 does_not_exist.bin 0\n");
        assert!(matches!(
            resolve_gdi_data_track(&gdi),
            Err(GdiError::MissingTrackFile(_))
        ));
    }

    #[test]
    fn traversal_refuses() {
        let gdi = write_temp("traversal.gdi", "1\n1 45000 4 2352 ../outside.bin 0\n");
        assert_eq!(resolve_gdi_data_track(&gdi), Err(GdiError::UnsafeReference));
    }

    #[test]
    fn absolute_path_refuses() {
        let gdi = write_temp("absolute.gdi", "1\n1 45000 4 2352 /etc/passwd 0\n");
        assert_eq!(resolve_gdi_data_track(&gdi), Err(GdiError::UnsafeReference));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_refuses() {
        let gdi = write_temp("symlink.gdi", "1\n1 45000 4 2352 escape.bin 0\n");
        let dir = gdi.parent().unwrap();
        let outside = dir
            .parent()
            .unwrap()
            .join(format!("archivefs-gdi-outside-{}", std::process::id()));
        std::fs::write(&outside, vec![0_u8; 2352 * 2]).unwrap();
        // `write_temp` intentionally reuses its PID/name directory. Remove
        // any stale sibling left by an earlier interrupted run before
        // creating the escape symlink, whose existence is the behavior under
        // test rather than a setup failure.
        let _ = std::fs::remove_file(dir.join("escape.bin"));
        std::os::unix::fs::symlink(&outside, dir.join("escape.bin")).unwrap();
        assert_eq!(resolve_gdi_data_track(&gdi), Err(GdiError::UnsafeReference));
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn duplicate_track_number_refuses() {
        let gdi = write_temp(
            "duplicate-number.gdi",
            "2\n1 0 4 2352 a.bin 0\n1 45000 4 2352 b.bin 0\n",
        );
        write_sibling(&gdi, "a.bin", &vec![0_u8; 2352]);
        write_sibling(&gdi, "b.bin", &vec![0_u8; 2352 * 2]);
        assert_eq!(
            resolve_gdi_data_track(&gdi),
            Err(GdiError::DuplicateTrackNumber(1))
        );
    }

    #[test]
    fn duplicate_start_lba_refuses() {
        let gdi = write_temp(
            "duplicate-lba.gdi",
            "2\n1 45000 4 2352 a.bin 0\n2 45000 4 2352 b.bin 0\n",
        );
        write_sibling(&gdi, "a.bin", &vec![0_u8; 2352 * 2]);
        write_sibling(&gdi, "b.bin", &vec![0_u8; 2352 * 2]);
        assert_eq!(
            resolve_gdi_data_track(&gdi),
            Err(GdiError::DuplicateStartLba)
        );
    }

    #[test]
    fn ambiguous_data_tracks_refuse() {
        let gdi = write_temp(
            "ambiguous.gdi",
            "2\n1 45000 4 2352 a.bin 0\n2 50000 4 2352 b.bin 0\n",
        );
        write_sibling(&gdi, "a.bin", &vec![0_u8; 2352 * 2]);
        write_sibling(&gdi, "b.bin", &vec![0_u8; 2352 * 2]);
        assert_eq!(
            resolve_gdi_data_track(&gdi),
            Err(GdiError::AmbiguousDataTracks)
        );
    }

    #[test]
    fn no_high_density_track_refuses() {
        // Only a low-density track exists - no track starts at or beyond
        // the GD-ROM high-density boundary.
        let gdi = write_temp("no-data.gdi", "1\n1 0 4 2352 a.bin 0\n");
        write_sibling(&gdi, "a.bin", &vec![0_u8; 2352 * 2]);
        assert_eq!(resolve_gdi_data_track(&gdi), Err(GdiError::NoDataTrack));
    }

    #[test]
    fn audio_high_density_track_is_never_selected() {
        // An audio track sitting at/beyond the high-density boundary must
        // never itself become the selected data track.
        let gdi = write_temp("audio-only-hd.gdi", "1\n1 45000 0 2352 a.bin 0\n");
        write_sibling(&gdi, "a.bin", &vec![0_u8; 2352 * 2]);
        assert_eq!(resolve_gdi_data_track(&gdi), Err(GdiError::NoDataTrack));
    }

    #[test]
    fn unsupported_sector_size_refuses() {
        let gdi = write_temp("bad-sector-size.gdi", "1\n1 45000 4 2336 a.bin 0\n");
        write_sibling(&gdi, "a.bin", &vec![0_u8; 2336 * 2]);
        assert_eq!(
            resolve_gdi_data_track(&gdi),
            Err(GdiError::UnsupportedSectorSize(2336))
        );
    }

    #[test]
    fn oversized_descriptor_refuses() {
        let gdi = write_temp("huge.gdi", &"0".repeat((MAX_GDI_BYTES + 1) as usize));
        assert_eq!(resolve_gdi_data_track(&gdi), Err(GdiError::TooLarge));
    }

    #[test]
    fn track_count_mismatch_refuses() {
        let too_few = write_temp("too-few.gdi", "2\n1 0 4 2352 a.bin 0\n");
        assert!(matches!(
            resolve_gdi_data_track(&too_few),
            Err(GdiError::Malformed(_))
        ));

        let too_many = write_temp(
            "too-many.gdi",
            "1\n1 0 4 2352 a.bin 0\n2 45000 4 2352 b.bin 0\n",
        );
        assert!(matches!(
            resolve_gdi_data_track(&too_many),
            Err(GdiError::Malformed(_))
        ));
    }

    #[test]
    fn resolve_gdi_all_tracks_returns_every_track_in_order() {
        let gdi = write_temp("all-tracks.gdi", &normal_gdi("track03.bin", 2352));
        write_sibling(&gdi, "track01.bin", &vec![0_u8; 2352]);
        write_sibling(&gdi, "track02.raw", &vec![0_u8; 2352]);
        write_sibling(&gdi, "track03.bin", &vec![0_u8; 2352 * 2]);

        let files = resolve_gdi_all_tracks(&gdi).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files[0].ends_with("track01.bin"));
        assert!(files[1].ends_with("track02.raw"));
        assert!(files[2].ends_with("track03.bin"));
    }

    #[test]
    fn resolve_gdi_all_tracks_refuses_a_missing_track() {
        let gdi = write_temp("missing-one-track.gdi", &normal_gdi("track03.bin", 2352));
        write_sibling(&gdi, "track01.bin", &vec![0_u8; 2352]);
        // track02.raw deliberately not written.
        write_sibling(&gdi, "track03.bin", &vec![0_u8; 2352 * 2]);

        assert!(matches!(
            resolve_gdi_all_tracks(&gdi),
            Err(GdiError::MissingTrackFile(_))
        ));
    }

    #[test]
    fn resolve_gdi_all_tracks_refuses_traversal() {
        let gdi = write_temp("traversal-all.gdi", "1\n1 45000 4 2352 ../outside.bin 0\n");
        assert_eq!(resolve_gdi_all_tracks(&gdi), Err(GdiError::UnsafeReference));
    }
}
