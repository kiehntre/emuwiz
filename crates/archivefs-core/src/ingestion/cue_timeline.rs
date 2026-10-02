//! Deterministic, read-only disc timeline for a parsed CUE sheet.
//!
//! This extends the canonical parser in [`super::cue_bin`] (it is not a second
//! parser): it takes a [`CueLayout`] and projects, per track, which parts of
//! the pre-index region are **source-backed** (`INDEX 00`, real bytes in the
//! referenced file) and which are **synthetic** (`PREGAP`/`POSTGAP`, frames
//! that exist on the disc timeline but not in any file). The two are never
//! merged: turning one into the other changes the disc.
//!
//! # Semantics (independently derived from the CUE format documentation,
//! see `docs/research/CUE_INDEX00_PREGAP_SEMANTICS.md`)
//!
//! * MSF is `MM:SS:FF`, 75 frames per second; all arithmetic is checked
//!   integer arithmetic.
//! * `INDEX` timestamps are relative to their own `FILE`, not the disc.
//! * `INDEX 00` is a position inside the file; its frames are real data.
//! * `PREGAP` is synthetic: it lengthens the disc timeline but occupies no
//!   file bytes and does not move any file offset.
//! * `INDEX 00` together with `PREGAP` on one track is ambiguous across
//!   tools, so it is refused as [`RefusalKind::ReviewRequired`] rather than
//!   added up.
//! * Timeline positions are `cue_frame`s: frames from the first frame the
//!   sheet describes. They are not LBAs. `lba_index01` is derived relative to
//!   track 1's `INDEX 01`, which is LBA 0 on a CD.
//!
//! No file is read; only lengths from metadata are used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::cue_bin::{
    CUE_FRAMES_PER_SECOND, CueDataTrackMode, CueLayout, CueTimestamp, CueTrack, CueTrackMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// The sheet is internally inconsistent or does not fit its files.
    Invalid,
    /// Valid but not representable by this model.
    Unsupported,
    /// Meaning depends on the consuming tool; a human must decide.
    ReviewRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRefusal {
    pub kind: RefusalKind,
    pub track: Option<u32>,
    pub reason: String,
}

impl std::fmt::Display for TimelineRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.track {
            Some(track) => write!(f, "{:?} (track {track}): {}", self.kind, self.reason),
            None => write!(f, "{:?}: {}", self.kind, self.reason),
        }
    }
}

impl std::error::Error for TimelineRefusal {}

fn refuse<T>(
    kind: RefusalKind,
    track: Option<u32>,
    reason: impl Into<String>,
) -> Result<T, TimelineRefusal> {
    Err(TimelineRefusal {
        kind,
        track,
        reason: reason.into(),
    })
}

/// A source-backed span inside one referenced file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    pub start_frame: u64,
    pub frames: u64,
    pub byte_offset: u64,
    pub byte_len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PregapEvidence {
    None,
    /// `INDEX 00`: real bytes from the referenced file.
    SourceBacked(SourceRange),
    /// `PREGAP`: synthetic frames, not stored in any file.
    Synthetic {
        frames: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackTimeline {
    pub number: u32,
    pub mode: CueTrackMode,
    pub file_ordinal: u32,
    pub path: PathBuf,
    pub sector_bytes: u64,
    pub pregap: PregapEvidence,
    /// Source-backed program area: `INDEX 01` up to the next boundary.
    pub program: SourceRange,
    /// `INDEX 02..` positions as file frames.
    pub extra_indexes: Vec<(u8, u64)>,
    pub synthetic_postgap_frames: u64,
    /// First timeline frame belonging to this track (synthetic pregap or
    /// `INDEX 00` start, else `INDEX 01`).
    pub cue_frame_start: u64,
    pub cue_frame_index01: u64,
    /// One past the program area, before any synthetic postgap.
    pub cue_frame_end: u64,
    /// `INDEX 01` relative to track 1's `INDEX 01` (LBA).
    pub lba_index01: i64,
}

impl TrackTimeline {
    pub fn synthetic_pregap_frames(&self) -> u64 {
        match self.pregap {
            PregapEvidence::Synthetic { frames } => frames,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTimeline {
    pub ordinal: u32,
    pub path: PathBuf,
    pub file_type: String,
    pub frames: u64,
    pub bytes: u64,
}

/// What a future DAT comparison can distinguish. EmuWiz never claims Redump
/// equivalence from this; it only records which structure is present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutEvidence {
    pub source_backed_pregap_tracks: Vec<u32>,
    pub synthetic_pregap_tracks: Vec<u32>,
    pub synthetic_postgap_tracks: Vec<u32>,
    /// Frames of source-backed audio before track 1's `INDEX 01` beyond the
    /// standard 150-frame lead-in, when track 1 is audio.
    pub hidden_track_one_frames: Option<u64>,
    pub multi_file: bool,
    pub redump_equivalence_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscTimeline {
    pub files: Vec<FileTimeline>,
    pub tracks: Vec<TrackTimeline>,
    pub total_cue_frames: u64,
    pub evidence: LayoutEvidence,
}

/// Standard CD lead-in pregap length (2 seconds).
pub const STANDARD_PREGAP_FRAMES: u64 = 2 * CUE_FRAMES_PER_SECOND;

pub fn msf_to_frames(minutes: u64, seconds: u64, frames: u64) -> Option<u64> {
    if seconds >= 60 || frames >= CUE_FRAMES_PER_SECOND {
        return None;
    }
    minutes
        .checked_mul(60)?
        .checked_add(seconds)?
        .checked_mul(CUE_FRAMES_PER_SECOND)?
        .checked_add(frames)
}

pub fn frames_to_msf(total: u64) -> String {
    let frames = total % CUE_FRAMES_PER_SECOND;
    let seconds_total = total / CUE_FRAMES_PER_SECOND;
    format!(
        "{:02}:{:02}:{:02}",
        seconds_total / 60,
        seconds_total % 60,
        frames
    )
}

fn sector_bytes(mode: &CueTrackMode) -> u64 {
    match mode {
        CueTrackMode::Data(CueDataTrackMode::Mode1_2048) => 2048,
        CueTrackMode::Data(CueDataTrackMode::Mode1_2352)
        | CueTrackMode::Data(CueDataTrackMode::Mode2_2352)
        | CueTrackMode::Audio => 2352,
    }
}

fn bytes_for(frames: u64, sector: u64, track: u32) -> Result<u64, TimelineRefusal> {
    match frames.checked_mul(sector) {
        Some(bytes) => Ok(bytes),
        None => refuse(
            RefusalKind::Invalid,
            Some(track),
            "frame position overflows a byte offset",
        ),
    }
}

fn add(a: u64, b: u64, track: u32) -> Result<u64, TimelineRefusal> {
    match a.checked_add(b) {
        Some(sum) => Ok(sum),
        None => refuse(
            RefusalKind::Invalid,
            Some(track),
            "timeline position overflows",
        ),
    }
}

/// Builds the timeline using real file lengths from metadata.
pub fn build_timeline(layout: &CueLayout) -> Result<DiscTimeline, TimelineRefusal> {
    build_timeline_with(layout, |path| {
        std::fs::metadata(path)
            .map(|m| m.len())
            .map_err(|e| e.to_string())
    })
}

/// Same, with an injectable length source (used for overflow tests and by
/// callers that already hold lengths). `length_of` must not read file data.
pub fn build_timeline_with(
    layout: &CueLayout,
    length_of: impl Fn(&Path) -> Result<u64, String>,
) -> Result<DiscTimeline, TimelineRefusal> {
    if layout.tracks.is_empty() {
        return refuse(RefusalKind::Invalid, None, "CUE has no tracks");
    }
    // Group tracks per FILE, preserving order; tracks must be numbered
    // consecutively from 1 and files must not interleave.
    let mut groups: BTreeMap<u32, Vec<&CueTrack>> = BTreeMap::new();
    let mut previous_ordinal = 0u32;
    for (position, track) in layout.tracks.iter().enumerate() {
        if track.number as usize != position + 1 {
            return refuse(
                RefusalKind::Invalid,
                Some(track.number),
                "tracks are not numbered consecutively from 1",
            );
        }
        if track.file_ordinal < previous_ordinal {
            return refuse(
                RefusalKind::Invalid,
                Some(track.number),
                "FILE ordering is not monotonic",
            );
        }
        previous_ordinal = track.file_ordinal;
        groups.entry(track.file_ordinal).or_default().push(track);
    }

    let mut files = Vec::new();
    let mut tracks = Vec::new();
    let mut disc_pos: u64 = 0;
    for (ordinal, group) in &groups {
        let first = group[0];
        if first.file_type != "BINARY" {
            return refuse(
                RefusalKind::Unsupported,
                Some(first.number),
                format!(
                    "FILE type {:?} has no verified frame-to-byte mapping (only BINARY)",
                    first.file_type
                ),
            );
        }
        let sector = sector_bytes(&first.mode);
        if group
            .iter()
            .any(|t| sector_bytes(&t.mode) != sector || t.path != first.path)
        {
            return refuse(
                RefusalKind::Unsupported,
                Some(first.number),
                "one FILE mixes sector sizes",
            );
        }
        let bytes = length_of(&first.path).map_err(|e| TimelineRefusal {
            kind: RefusalKind::Invalid,
            track: Some(first.number),
            reason: format!("source file is unavailable: {e}"),
        })?;
        if bytes == 0 || bytes % sector != 0 {
            return refuse(
                RefusalKind::Invalid,
                Some(first.number),
                "source file is empty or not a whole-sector stream (truncated?)",
            );
        }
        let file_frames = bytes / sector;
        files.push(FileTimeline {
            ordinal: *ordinal,
            path: first.path.clone(),
            file_type: first.file_type.clone(),
            frames: file_frames,
            bytes,
        });

        let file_base = disc_pos;
        let mut inserted: u64 = 0;
        for (i, track) in group.iter().enumerate() {
            let n = track.number;
            let Some(index01) = track.index_01 else {
                return refuse(RefusalKind::Invalid, Some(n), "TRACK has no INDEX 01");
            };
            if track.index_00.is_some() && track.pregap.is_some() {
                return refuse(
                    RefusalKind::ReviewRequired,
                    Some(n),
                    "INDEX 00 and PREGAP on one track: whether they add or describe the same gap depends on the tool; not summed",
                );
            }
            // Region start in the file: INDEX 00 if present.
            let region_start = match track.index_00 {
                Some(i0) if i0.frames >= index01.frames => {
                    return refuse(
                        RefusalKind::Invalid,
                        Some(n),
                        "INDEX 00 must precede INDEX 01",
                    );
                }
                Some(i0) => i0.frames,
                None => index01.frames,
            };
            // Order against the previous track in the same file.
            if let Some(prev) = i.checked_sub(1).map(|p| group[p]) {
                let prev_i1 = prev.index_01.map_or(0, |t| t.frames);
                if region_start <= prev_i1 {
                    return refuse(
                        RefusalKind::Invalid,
                        Some(n),
                        "INDEX values do not increase through the FILE (overlapping tracks)",
                    );
                }
            }
            // End of this track's program area.
            let boundary = match group.get(i + 1) {
                Some(next) => match (next.index_00, next.index_01) {
                    (Some(i0), _) => i0.frames,
                    (None, Some(i1)) => i1.frames,
                    (None, None) => {
                        return refuse(
                            RefusalKind::Invalid,
                            Some(next.number),
                            "TRACK has no INDEX 01",
                        );
                    }
                },
                None => file_frames,
            };
            if index01.frames >= boundary || boundary > file_frames {
                return refuse(
                    RefusalKind::Invalid,
                    Some(n),
                    "INDEX is outside its source file or the track has no program frames",
                );
            }
            // Additional indexes: strictly increasing, inside the program area.
            let mut extras = Vec::new();
            let mut last = index01.frames;
            for (number, ts) in &track.extra_indexes {
                if ts.frames <= last || ts.frames >= boundary {
                    return refuse(
                        RefusalKind::Invalid,
                        Some(n),
                        format!("INDEX {number:02} is out of order or outside the track"),
                    );
                }
                last = ts.frames;
                extras.push((*number, ts.frames));
            }
            let pregap = match (track.index_00, track.pregap) {
                (Some(i0), None) => {
                    let frames = index01.frames - i0.frames;
                    PregapEvidence::SourceBacked(SourceRange {
                        start_frame: i0.frames,
                        frames,
                        byte_offset: bytes_for(i0.frames, sector, n)?,
                        byte_len: bytes_for(frames, sector, n)?,
                    })
                }
                (None, Some(p)) => PregapEvidence::Synthetic { frames: p.frames },
                (None, None) => PregapEvidence::None,
                (Some(_), Some(_)) => unreachable!("rejected above"),
            };
            let synth_pre = match pregap {
                PregapEvidence::Synthetic { frames } => frames,
                _ => 0,
            };
            let postgap = track.postgap.map_or(0, |t| t.frames);
            let program_frames = boundary - index01.frames;
            let program = SourceRange {
                start_frame: index01.frames,
                frames: program_frames,
                byte_offset: bytes_for(index01.frames, sector, n)?,
                byte_len: bytes_for(program_frames, sector, n)?,
            };
            let cue_start = add(add(file_base, region_start, n)?, inserted, n)?;
            inserted = add(inserted, synth_pre, n)?;
            let cue_index01 = add(add(file_base, index01.frames, n)?, inserted, n)?;
            let cue_end = add(add(file_base, boundary, n)?, inserted, n)?;
            inserted = add(inserted, postgap, n)?;
            tracks.push(TrackTimeline {
                number: n,
                mode: track.mode.clone(),
                file_ordinal: *ordinal,
                path: track.path.clone(),
                sector_bytes: sector,
                pregap,
                program,
                extra_indexes: extras,
                synthetic_postgap_frames: postgap,
                cue_frame_start: cue_start,
                cue_frame_index01: cue_index01,
                cue_frame_end: cue_end,
                lba_index01: 0,
            });
        }
        disc_pos = add(
            add(file_base, file_frames, first.number)?,
            inserted,
            first.number,
        )?;
    }

    // LBA is relative to track 1's INDEX 01 (LBA 0 on a CD).
    let origin = tracks[0].cue_frame_index01;
    for track in &mut tracks {
        let delta = i128::from(track.cue_frame_index01) - i128::from(origin);
        track.lba_index01 = match i64::try_from(delta) {
            Ok(v) => v,
            Err(_) => {
                return refuse(
                    RefusalKind::Invalid,
                    Some(track.number),
                    "LBA is outside i64",
                );
            }
        };
    }

    let evidence = LayoutEvidence {
        source_backed_pregap_tracks: tracks
            .iter()
            .filter(|t| matches!(t.pregap, PregapEvidence::SourceBacked(_)))
            .map(|t| t.number)
            .collect(),
        synthetic_pregap_tracks: tracks
            .iter()
            .filter(|t| matches!(t.pregap, PregapEvidence::Synthetic { .. }))
            .map(|t| t.number)
            .collect(),
        synthetic_postgap_tracks: tracks
            .iter()
            .filter(|t| t.synthetic_postgap_frames > 0)
            .map(|t| t.number)
            .collect(),
        hidden_track_one_frames: match (&tracks[0].mode, tracks[0].pregap) {
            (CueTrackMode::Audio, PregapEvidence::SourceBacked(range))
                if range.frames > STANDARD_PREGAP_FRAMES =>
            {
                Some(range.frames - STANDARD_PREGAP_FRAMES)
            }
            _ => None,
        },
        multi_file: files.len() > 1,
        redump_equivalence_verified: false,
    };
    Ok(DiscTimeline {
        files,
        tracks,
        total_cue_frames: disc_pos,
        evidence,
    })
}

// ---------------------------------------------------------------- diagnostics

impl DiscTimeline {
    /// Human-readable, deterministic projection for diagnostics.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for t in &self.tracks {
            let kind = match t.mode {
                CueTrackMode::Audio => "AUDIO".to_owned(),
                CueTrackMode::Data(mode) => format!("{mode:?}"),
            };
            out.push_str(&format!(
                "Track {:02} {kind}\n  Source: {}\n",
                t.number,
                t.path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            ));
            match t.pregap {
                PregapEvidence::SourceBacked(r) => out.push_str(&format!(
                    "  Source-backed pregap: {} via INDEX 00\n  Synthetic pregap: none\n",
                    frames_to_msf(r.frames)
                )),
                PregapEvidence::Synthetic { frames } => out.push_str(&format!(
                    "  Source-backed pregap: none\n  Synthetic pregap: {} via PREGAP\n",
                    frames_to_msf(frames)
                )),
                PregapEvidence::None => {
                    out.push_str("  Source-backed pregap: none\n  Synthetic pregap: none\n")
                }
            }
            out.push_str(&format!(
                "  Program area begins: INDEX 01 at file {} (disc {}, LBA {})\n  Absolute disc start: {}\n",
                frames_to_msf(t.program.start_frame),
                frames_to_msf(t.cue_frame_index01),
                t.lba_index01,
                frames_to_msf(t.cue_frame_start),
            ));
            if t.synthetic_postgap_frames > 0 {
                out.push_str(&format!(
                    "  Synthetic postgap: {}\n",
                    frames_to_msf(t.synthetic_postgap_frames)
                ));
            }
        }
        out
    }

    /// Semantics-only signature: formatting-independent, distinguishes
    /// `INDEX 00` from `PREGAP`. Used to prove a rewrite preserved the disc.
    pub fn semantic_signature(&self) -> String {
        let mut s = String::new();
        for f in &self.files {
            s.push_str(&format!("F{}:{}:{};", f.ordinal, f.file_type, f.frames));
        }
        for t in &self.tracks {
            let gap = match t.pregap {
                PregapEvidence::SourceBacked(r) => format!("I00@{}+{}", r.start_frame, r.frames),
                PregapEvidence::Synthetic { frames } => format!("PRE{frames}"),
                PregapEvidence::None => "-".into(),
            };
            s.push_str(&format!(
                "T{}:{:?}:f{}:{gap}:I01@{}+{}:x{:?}:post{};",
                t.number,
                t.mode,
                t.file_ordinal,
                t.program.start_frame,
                t.program.frames,
                t.extra_indexes,
                t.synthetic_postgap_frames
            ));
        }
        s
    }
}

// ------------------------------------------------------------- canonical text

/// Renders a layout as canonical CUE text. Formatting is normalised (spacing,
/// ordering, `REM`/`TITLE` dropped); **semantics are not**: `INDEX 00` stays
/// `INDEX 00` and `PREGAP` stays `PREGAP`. File names are written as the
/// final path component (references are same-directory by construction).
pub fn render_canonical_cue(layout: &CueLayout) -> Result<String, TimelineRefusal> {
    let mut out = String::new();
    let mut current: Option<u32> = None;
    for track in &layout.tracks {
        if current != Some(track.file_ordinal) {
            let Some(name) = track.path.file_name().and_then(|n| n.to_str()) else {
                return refuse(
                    RefusalKind::Unsupported,
                    Some(track.number),
                    "FILE name is not UTF-8",
                );
            };
            if name.contains('"') {
                return refuse(
                    RefusalKind::Unsupported,
                    Some(track.number),
                    "FILE name contains a quote",
                );
            }
            out.push_str(&format!("FILE \"{name}\" {}\n", track.file_type));
            current = Some(track.file_ordinal);
        }
        let mode = match track.mode {
            CueTrackMode::Audio => "AUDIO",
            CueTrackMode::Data(CueDataTrackMode::Mode1_2048) => "MODE1/2048",
            CueTrackMode::Data(CueDataTrackMode::Mode1_2352) => "MODE1/2352",
            CueTrackMode::Data(CueDataTrackMode::Mode2_2352) => "MODE2/2352",
        };
        out.push_str(&format!("  TRACK {:02} {mode}\n", track.number));
        let msf = |t: CueTimestamp| frames_to_msf(t.frames);
        if let Some(p) = track.pregap {
            out.push_str(&format!("    PREGAP {}\n", msf(p)));
        }
        if let Some(i) = track.index_00 {
            out.push_str(&format!("    INDEX 00 {}\n", msf(i)));
        }
        if let Some(i) = track.index_01 {
            out.push_str(&format!("    INDEX 01 {}\n", msf(i)));
        }
        for (n, t) in &track.extra_indexes {
            out.push_str(&format!("    INDEX {n:02} {}\n", msf(*t)));
        }
        if let Some(p) = track.postgap {
            out.push_str(&format!("    POSTGAP {}\n", msf(p)));
        }
    }
    Ok(out)
}

// ------------------------------------------------------- conversion preservation

/// Why a timeline cannot be handed to the current CUE-to-CHD converter
/// without losing something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionBlocker {
    SourceBackedPregap { track: u32, frames: u64 },
    SyntheticPregap { track: u32, frames: u64 },
    SyntheticPostgap { track: u32, frames: u64 },
    HiddenTrackOne { frames: u64 },
    ExtraIndexes { track: u32 },
    MultipleFiles,
    MultipleTracks,
    NonDataTrack { track: u32 },
    UnverifiedMode { track: u32 },
    FirstIndexNotAtFileStart { track: u32 },
}

impl ConversionBlocker {
    pub fn reason(&self) -> String {
        match self {
            Self::SourceBackedPregap { track, frames } => format!(
                "Track {track} has {} of source-backed pregap (INDEX 00); the converter does not carry it, so converting would drop it.",
                frames_to_msf(*frames)
            ),
            Self::SyntheticPregap { track, frames } => format!(
                "Track {track} declares a synthetic PREGAP of {}; the converter would not materialise it with proven semantics.",
                frames_to_msf(*frames)
            ),
            Self::SyntheticPostgap { track, frames } => format!(
                "Track {track} declares a synthetic POSTGAP of {}; not preserved by the converter.",
                frames_to_msf(*frames)
            ),
            Self::HiddenTrackOne { frames } => format!(
                "Hidden audio before track 1 ({}) would be lost.",
                frames_to_msf(*frames)
            ),
            Self::ExtraIndexes { track } => {
                format!("Track {track} has INDEX 02+ markers the converter does not preserve.")
            }
            Self::MultipleFiles => {
                "The sheet references several FILEs; only a single-file layout is verified.".into()
            }
            Self::MultipleTracks => "Only a single-track disc is verified for conversion.".into(),
            Self::NonDataTrack { track } => {
                format!("Track {track} is audio; only a data track is verified.")
            }
            Self::UnverifiedMode { track } => {
                format!("Track {track} uses a sector mode the converter contract does not verify.")
            }
            Self::FirstIndexNotAtFileStart { track } => {
                format!("Track {track} INDEX 01 is not at file frame 0.")
            }
        }
    }
}

/// The only layout the current converter is known to preserve (the
/// `optical_preservation` contract): one BINARY file, one MODE1/2048 track,
/// `INDEX 01` at frame 0, no gaps and no extra indexes.
pub fn chd_conversion_preservation(
    timeline: &DiscTimeline,
) -> Result<&'static str, Vec<ConversionBlocker>> {
    let mut blockers = Vec::new();
    if timeline.files.len() > 1 {
        blockers.push(ConversionBlocker::MultipleFiles);
    }
    if timeline.tracks.len() > 1 {
        blockers.push(ConversionBlocker::MultipleTracks);
    }
    if let Some(frames) = timeline.evidence.hidden_track_one_frames {
        blockers.push(ConversionBlocker::HiddenTrackOne { frames });
    }
    for t in &timeline.tracks {
        match t.pregap {
            PregapEvidence::SourceBacked(r) => {
                blockers.push(ConversionBlocker::SourceBackedPregap {
                    track: t.number,
                    frames: r.frames,
                })
            }
            PregapEvidence::Synthetic { frames } => {
                blockers.push(ConversionBlocker::SyntheticPregap {
                    track: t.number,
                    frames,
                })
            }
            PregapEvidence::None => {}
        }
        if t.synthetic_postgap_frames > 0 {
            blockers.push(ConversionBlocker::SyntheticPostgap {
                track: t.number,
                frames: t.synthetic_postgap_frames,
            });
        }
        if !t.extra_indexes.is_empty() {
            blockers.push(ConversionBlocker::ExtraIndexes { track: t.number });
        }
        match t.mode {
            CueTrackMode::Audio => {
                blockers.push(ConversionBlocker::NonDataTrack { track: t.number })
            }
            CueTrackMode::Data(CueDataTrackMode::Mode1_2048) => {}
            CueTrackMode::Data(_) => {
                blockers.push(ConversionBlocker::UnverifiedMode { track: t.number })
            }
        }
        if t.program.start_frame != 0 && matches!(t.pregap, PregapEvidence::None) {
            blockers.push(ConversionBlocker::FirstIndexNotAtFileStart { track: t.number });
        }
    }
    if blockers.is_empty() {
        Ok(
            "single BINARY MODE1/2048 track, INDEX 01 at file frame 0, no gaps: matches the verified optical_preservation layout contract",
        )
    } else {
        Err(blockers)
    }
}

#[cfg(test)]
mod tests;
