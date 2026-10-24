//! Read-only classification of a CUE/BIN or GDI disc layout for display.
//!
//! This composes the canonical parsers ([`crate::ingestion::cue_bin`],
//! [`crate::ingestion::cue_timeline`], [`crate::ingestion::gdi`]); it is not a
//! parser. The only filesystem access beyond those parsers is one bounded
//! directory listing, used to report case-only look-alikes of a missing
//! companion. Nothing is opened for content, hashed, resolved by guesswork,
//! repaired or written.
//!
//! The verification basis is *structure and file lengths from metadata*. A
//! [`OpticalLayoutState::Valid`] result never means the bytes are intact,
//! match a DAT, or play; see [`OpticalLayoutReport::limits`].

use std::path::Path;

use crate::ingestion::cue_bin::{
    CueError, CueTrackMode, resolve_cue_all_files_lenient, resolve_cue_layout,
};
use crate::ingestion::cue_timeline::{PregapEvidence, RefusalKind, build_timeline, frames_to_msf};
use crate::ingestion::gdi::{
    GdiError, GdiTrackType, parse_gdi_descriptor, resolve_gdi_all_tracks_lenient,
};

const MAX_LISTED_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpticalContainer {
    CueBin,
    Gdi,
}

/// Worst-first outcome. Variants are mutually exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpticalLayoutState {
    /// Sheet parses and every companion exists with lengths consistent with it.
    Valid,
    /// A referenced companion file does not exist.
    Missing,
    /// Several readings are possible (case-only look-alike companion, several
    /// data tracks, `INDEX 00` together with `PREGAP`). Nothing is chosen.
    Ambiguous,
    /// Companion lengths contradict the sheet: truncated, replaced or edited
    /// after the sheet was written. A stateless check cannot tell which.
    Mismatch,
    /// A reference leaves the sheet's directory or is not a regular file.
    Unsafe,
    /// Valid text this model does not represent (non-BINARY FILE, odd sector size).
    Unsupported,
    /// The descriptor itself could not be read or parsed.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompanionStatus {
    Present {
        bytes: u64,
    },
    Missing,
    /// Case-only look-alikes exist; none was selected.
    Ambiguous {
        candidates: Vec<String>,
    },
    Unsafe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionRow {
    pub name: String,
    pub status: CompanionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRow {
    pub number: u32,
    pub kind: String,
    pub companion: String,
    /// Where the program area starts, in the format's own coordinates.
    pub start: String,
    /// Frames of known length, or `None` when it cannot be derived safely.
    pub frames: Option<u64>,
    pub gap: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpticalLayoutReport {
    pub container: OpticalContainer,
    pub state: OpticalLayoutState,
    pub summary: String,
    pub tracks: Vec<TrackRow>,
    pub companions: Vec<CompanionRow>,
    /// Gap/index semantics that apply to this particular sheet.
    pub notes: Vec<String>,
    /// What was actually checked.
    pub basis: &'static str,
    /// What this result must never be read as.
    pub limits: Vec<&'static str>,
}

const CUE_BASIS: &str = "CUE text parsed; each FILE resolved beside the sheet; track positions checked against file sizes from metadata. No file content was read or hashed.";
const GDI_BASIS: &str = "GDI text parsed; each track file resolved beside the descriptor; sector alignment checked from file sizes. No file content was read or hashed.";

fn limits() -> Vec<&'static str> {
    vec![
        "Not byte verification: filenames, headers and sizes cannot show that the data is intact.",
        "Not a DAT or Redump match, and not evidence that the disc plays.",
    ]
}

pub fn inspect_optical_layout(path: &Path) -> Option<OpticalLayoutReport> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase();
    match extension.as_str() {
        "cue" => Some(inspect_cue(path)),
        "gdi" => Some(inspect_gdi(path)),
        _ => None,
    }
}

fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Case-only look-alikes of a missing name in the sheet's own directory.
/// Bounded; used for reporting only, never to resolve the reference.
fn case_lookalikes(missing: &Path) -> Vec<String> {
    let (Some(dir), Some(wanted)) = (missing.parent(), missing.file_name()) else {
        return Vec::new();
    };
    let wanted = wanted.to_string_lossy().to_ascii_lowercase();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .take(MAX_LISTED_ENTRIES)
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.to_ascii_lowercase() == wanted)
        .collect();
    found.sort();
    found
}

fn missing_status(missing: &Path) -> CompanionStatus {
    let candidates = case_lookalikes(missing);
    if candidates.is_empty() {
        CompanionStatus::Missing
    } else {
        CompanionStatus::Ambiguous { candidates }
    }
}

fn state_of_companions(rows: &[CompanionRow]) -> Option<OpticalLayoutState> {
    let mut worst = None;
    for row in rows {
        let state = match row.status {
            CompanionStatus::Present { .. } => continue,
            CompanionStatus::Unsafe => OpticalLayoutState::Unsafe,
            CompanionStatus::Missing => OpticalLayoutState::Missing,
            CompanionStatus::Ambiguous { .. } => OpticalLayoutState::Ambiguous,
        };
        // Unsafe outranks Missing outranks Ambiguous for the headline.
        let rank = |s: OpticalLayoutState| match s {
            OpticalLayoutState::Unsafe => 0,
            OpticalLayoutState::Missing => 1,
            _ => 2,
        };
        if worst.is_none_or(|current| rank(state) < rank(current)) {
            worst = Some(state);
        }
    }
    worst
}

fn companion_summary(state: OpticalLayoutState) -> &'static str {
    match state {
        OpticalLayoutState::Unsafe => {
            "A referenced file is outside the disc folder or is not a regular file; it was not opened."
        }
        OpticalLayoutState::Missing => "A referenced companion file is missing.",
        _ => "A referenced file has only case-different look-alikes; none was chosen.",
    }
}

fn report(
    container: OpticalContainer,
    state: OpticalLayoutState,
    summary: impl Into<String>,
    basis: &'static str,
) -> OpticalLayoutReport {
    OpticalLayoutReport {
        container,
        state,
        summary: summary.into(),
        tracks: Vec::new(),
        companions: Vec::new(),
        notes: Vec::new(),
        basis,
        limits: limits(),
    }
}

fn inspect_cue(path: &Path) -> OpticalLayoutReport {
    let container = OpticalContainer::CueBin;
    let outcomes = match resolve_cue_all_files_lenient(path) {
        Ok(outcomes) => outcomes,
        Err(error) => return cue_error_report(error, CUE_BASIS),
    };
    let mut companions = Vec::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        companions.push(match outcome {
            Ok(resolved) => {
                let bytes = std::fs::metadata(resolved).map_or(0, |m| m.len());
                CompanionRow {
                    name: file_label(resolved),
                    status: CompanionStatus::Present { bytes },
                }
            }
            Err(CueError::MissingDataFile(missing)) => CompanionRow {
                name: file_label(missing),
                status: missing_status(missing),
            },
            Err(_) => CompanionRow {
                name: format!("FILE reference #{}", index + 1),
                status: CompanionStatus::Unsafe,
            },
        });
    }
    if let Some(state) = state_of_companions(&companions) {
        let mut out = report(container, state, companion_summary(state), CUE_BASIS);
        out.companions = companions;
        return out;
    }

    let layout = match resolve_cue_layout(path) {
        Ok(layout) => layout,
        Err(error) => {
            let mut out = cue_error_report(error, CUE_BASIS);
            out.companions = companions;
            return out;
        }
    };
    let timeline = match build_timeline(&layout) {
        Ok(timeline) => timeline,
        Err(refusal) => {
            let state = match refusal.kind {
                RefusalKind::Invalid => OpticalLayoutState::Mismatch,
                RefusalKind::Unsupported => OpticalLayoutState::Unsupported,
                RefusalKind::ReviewRequired => OpticalLayoutState::Ambiguous,
            };
            let mut out = report(container, state, refusal.to_string(), CUE_BASIS);
            out.companions = companions;
            if refusal.kind == RefusalKind::ReviewRequired {
                out.notes.push(index00_pregap_note());
            }
            return out;
        }
    };

    let tracks = timeline
        .tracks
        .iter()
        .map(|track| TrackRow {
            number: track.number,
            kind: match track.mode {
                CueTrackMode::Audio => "Audio".to_owned(),
                CueTrackMode::Data(mode) => format!("Data {mode:?}"),
            },
            companion: file_label(&track.path),
            start: format!(
                "INDEX 01 at disc {}",
                frames_to_msf(track.cue_frame_index01)
            ),
            frames: Some(track.program.frames),
            gap: match track.pregap {
                PregapEvidence::None => "none".to_owned(),
                PregapEvidence::SourceBacked(range) => {
                    format!(
                        "INDEX 00: {} stored in the file",
                        frames_to_msf(range.frames)
                    )
                }
                PregapEvidence::Synthetic { frames } => {
                    format!("PREGAP: {} not stored in any file", frames_to_msf(frames))
                }
            },
        })
        .collect();

    let mut notes = Vec::new();
    if !timeline.evidence.source_backed_pregap_tracks.is_empty()
        || !timeline.evidence.synthetic_pregap_tracks.is_empty()
        || !timeline.evidence.synthetic_postgap_tracks.is_empty()
    {
        notes.push(index00_pregap_note());
    }
    if timeline.evidence.hidden_track_one_frames.is_some() {
        notes.push(
            "Track 1 is audio with stored bytes before its INDEX 01 beyond the standard lead-in (a hidden track); converting or re-splitting can drop it.".to_owned(),
        );
    }
    if timeline.evidence.multi_file {
        notes.push("Several BIN files: every one must travel with the sheet.".to_owned());
    }

    OpticalLayoutReport {
        container,
        state: OpticalLayoutState::Valid,
        summary: "Sheet and companion sizes agree. This is a structural check only.".to_owned(),
        tracks,
        companions,
        notes,
        basis: CUE_BASIS,
        limits: limits(),
    }
}

fn index00_pregap_note() -> String {
    "INDEX 00 describes gap bytes that are stored in the file; PREGAP is a gap that is not stored anywhere. They are shown separately and never summed. Tools disagree on a track that has both, so that case needs a human decision. EmuWiz does not claim a pregap matches a Redump dump.".to_owned()
}

fn cue_error_report(error: CueError, basis: &'static str) -> OpticalLayoutReport {
    let container = OpticalContainer::CueBin;
    let state = match &error {
        CueError::MissingDataFile(_) => OpticalLayoutState::Missing,
        CueError::UnsafeReference => OpticalLayoutState::Unsafe,
        CueError::AmbiguousDataTracks => OpticalLayoutState::Ambiguous,
        CueError::UnsupportedTrackMode(_) => OpticalLayoutState::Unsupported,
        // The canonical parser reports an INDEX past the end of its file this
        // way; a test pins the wording.
        CueError::Malformed(detail) if detail.contains("is outside FILE") => {
            OpticalLayoutState::Mismatch
        }
        CueError::Io(_)
        | CueError::TooLarge
        | CueError::NoFileReferences
        | CueError::Malformed(_) => OpticalLayoutState::Unreadable,
    };
    report(container, state, error.to_string(), basis)
}

fn inspect_gdi(path: &Path) -> OpticalLayoutReport {
    let container = OpticalContainer::Gdi;
    let descriptor = match parse_gdi_descriptor(path) {
        Ok(descriptor) => descriptor,
        Err(error) => return gdi_error_report(path, error),
    };
    let topology = descriptor.logical_topology();
    let mut tracks = Vec::new();
    let mut companions = Vec::new();
    let mut misaligned = Vec::new();
    for (track, logical) in descriptor.tracks.iter().zip(&topology.tracks) {
        if logical.frame_count.is_none() {
            misaligned.push(track.number);
        }
        tracks.push(TrackRow {
            number: track.number,
            kind: match track.track_type {
                GdiTrackType::Data => format!("Data {} B sectors", track.sector_size.bytes()),
                GdiTrackType::Audio => "Audio".to_owned(),
            },
            companion: file_label(&track.source_path),
            start: format!("LBA {}", track.start_lba),
            frames: logical.frame_count,
            gap: "not described by GDI".to_owned(),
        });
        companions.push(CompanionRow {
            name: file_label(&track.source_path),
            status: CompanionStatus::Present {
                bytes: track.source_length_bytes,
            },
        });
    }
    let mut notes = vec![
        "GDI records each track's start LBA only; it has no INDEX 00 or PREGAP fields, so pregap layout cannot be shown or checked here.".to_owned(),
    ];
    let mut state = OpticalLayoutState::Valid;
    let mut summary =
        "Descriptor and track file sizes agree. This is a structural check only.".to_owned();
    if !misaligned.is_empty() {
        state = OpticalLayoutState::Mismatch;
        summary = format!(
            "Track {} file size is not a whole number of sectors (or the descriptor offset is nonzero), so its length cannot be derived.",
            misaligned
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else if let Err(error) = crate::ingestion::gdi::data_track_from_descriptor(&descriptor) {
        state = match error {
            GdiError::AmbiguousDataTracks => OpticalLayoutState::Ambiguous,
            _ => OpticalLayoutState::Unsupported,
        };
        summary = error.to_string();
    }
    if companions.len() > 3 {
        notes.push("More than three tracks is unusual for a GD-ROM dump.".to_owned());
    }
    OpticalLayoutReport {
        container,
        state,
        summary,
        tracks,
        companions,
        notes,
        basis: GDI_BASIS,
        limits: limits(),
    }
}

fn gdi_error_report(path: &Path, error: GdiError) -> OpticalLayoutReport {
    let container = OpticalContainer::Gdi;
    // The strict parse refuses on the first bad track; list every track so one
    // missing file does not hide the others.
    if matches!(
        error,
        GdiError::MissingTrackFile(_) | GdiError::UnsafeReference
    ) && let Ok(outcomes) = resolve_gdi_all_tracks_lenient(path)
    {
        {
            let companions: Vec<CompanionRow> = outcomes
                .iter()
                .enumerate()
                .map(|(index, outcome)| match outcome {
                    Ok(resolved) => CompanionRow {
                        name: file_label(resolved),
                        status: CompanionStatus::Present {
                            bytes: std::fs::metadata(resolved).map_or(0, |m| m.len()),
                        },
                    },
                    Err(GdiError::MissingTrackFile(missing)) => CompanionRow {
                        name: file_label(missing),
                        status: missing_status(missing),
                    },
                    Err(_) => CompanionRow {
                        name: format!("Track reference #{}", index + 1),
                        status: CompanionStatus::Unsafe,
                    },
                })
                .collect();
            if let Some(state) = state_of_companions(&companions) {
                let mut out = report(container, state, companion_summary(state), GDI_BASIS);
                out.companions = companions;
                return out;
            }
        }
    }
    let state = match &error {
        GdiError::MissingTrackFile(_) => OpticalLayoutState::Missing,
        GdiError::UnsafeReference => OpticalLayoutState::Unsafe,
        GdiError::UnsupportedSectorSize(_) => OpticalLayoutState::Unsupported,
        GdiError::AmbiguousDataTracks => OpticalLayoutState::Ambiguous,
        _ => OpticalLayoutState::Unreadable,
    };
    report(container, state, error.to_string(), GDI_BASIS)
}

#[cfg(test)]
mod tests;
