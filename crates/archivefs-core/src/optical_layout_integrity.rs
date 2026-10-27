//! Read-only comparison of meaningful optical-disc layout semantics.
//!
//! CUE/BIN structure comes from [`crate::ingestion::cue_bin`]. This module
//! does not rewrite a sheet or an image, and it does not infer equivalence
//! when gap or payload evidence is absent.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ingestion::cue_bin::{CueLayout, CueTrackMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpticalLayoutRepresentation {
    CueBin,
    Gdi,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpticalTrackMode {
    Audio,
    Data {
        sector_size: u32,
        mode: OpticalDataMode,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpticalDataMode {
    Mode1_2048,
    Mode1_2352,
    Mode2_2352,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpticalIndexPoint {
    pub number: u8,
    pub frame: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpticalPregapEncoding {
    Index00,
    ExplicitPregap,
    None,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpticalGapLayout {
    pub encoding: OpticalPregapEncoding,
    pub index_00_frame: Option<u64>,
    pub index_01_frame: Option<u64>,
    pub explicit_pregap_frames: Option<u64>,
    pub postgap_frames: Option<u64>,
    /// Set only when an upstream inspector has proved that two different gap
    /// encodings describe the same disc timing, rather than merely having the
    /// same displayed duration.
    pub proven_effective_pregap_frames: Option<u64>,
}

impl OpticalGapLayout {
    pub fn none(index_01_frame: Option<u64>) -> Self {
        Self {
            encoding: OpticalPregapEncoding::None,
            index_00_frame: None,
            index_01_frame,
            explicit_pregap_frames: None,
            postgap_frames: None,
            proven_effective_pregap_frames: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpticalTrackLayout {
    pub order: usize,
    pub number: u32,
    pub mode: OpticalTrackMode,
    pub referenced_file: PathBuf,
    pub indexes: Vec<OpticalIndexPoint>,
    pub gap: OpticalGapLayout,
    pub payload_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OpticalLayoutEvidence {
    pub source_description: String,
    pub structural: bool,
    pub audio_payloads_complete: bool,
    pub data_payloads_complete: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpticalLayout {
    pub representation: OpticalLayoutRepresentation,
    pub source: PathBuf,
    pub tracks: Vec<OpticalTrackLayout>,
    pub evidence: OpticalLayoutEvidence,
}

impl OpticalLayout {
    /// Converts the existing validated CUE parser output into the shared
    /// comparison model. No referenced payload is read or modified.
    pub fn from_cue_layout(layout: &CueLayout) -> Self {
        let tracks = layout
            .tracks
            .iter()
            .enumerate()
            .map(|(order, track)| {
                let mode = match track.mode {
                    CueTrackMode::Audio => OpticalTrackMode::Audio,
                    CueTrackMode::Data(data_mode) => OpticalTrackMode::Data {
                        sector_size: match data_mode {
                            crate::ingestion::cue_bin::CueDataTrackMode::Mode1_2048 => 2048,
                            crate::ingestion::cue_bin::CueDataTrackMode::Mode1_2352
                            | crate::ingestion::cue_bin::CueDataTrackMode::Mode2_2352 => 2352,
                        },
                        mode: match data_mode {
                            crate::ingestion::cue_bin::CueDataTrackMode::Mode1_2048 => {
                                OpticalDataMode::Mode1_2048
                            }
                            crate::ingestion::cue_bin::CueDataTrackMode::Mode1_2352 => {
                                OpticalDataMode::Mode1_2352
                            }
                            crate::ingestion::cue_bin::CueDataTrackMode::Mode2_2352 => {
                                OpticalDataMode::Mode2_2352
                            }
                        },
                    },
                };
                let index_00_frame = track.index_00.map(|point| point.frames);
                let index_01_frame = track.index_01.map(|point| point.frames);
                let encoding = if index_00_frame.is_some() {
                    OpticalPregapEncoding::Index00
                } else if track.pregap.is_some() {
                    OpticalPregapEncoding::ExplicitPregap
                } else {
                    OpticalPregapEncoding::None
                };
                let mut indexes: Vec<OpticalIndexPoint> = [track.index_00, track.index_01]
                    .into_iter()
                    .enumerate()
                    .filter_map(|(number, point)| {
                        point.map(|point| OpticalIndexPoint {
                            number: number as u8,
                            frame: point.frames,
                        })
                    })
                    .collect();
                indexes.extend(track.extra_indexes.iter().map(|(number, point)| {
                    OpticalIndexPoint {
                        number: *number,
                        frame: point.frames,
                    }
                }));
                OpticalTrackLayout {
                    order,
                    number: track.number,
                    mode,
                    referenced_file: track.path.clone(),
                    indexes,
                    gap: OpticalGapLayout {
                        encoding,
                        index_00_frame,
                        index_01_frame,
                        explicit_pregap_frames: track.pregap.map(|point| point.frames),
                        postgap_frames: track.postgap.map(|point| point.frames),
                        proven_effective_pregap_frames: None,
                    },
                    payload_sha256: None,
                }
            })
            .collect();
        Self {
            representation: OpticalLayoutRepresentation::CueBin,
            source: layout.cue_path.clone(),
            tracks,
            evidence: OpticalLayoutEvidence {
                source_description: "validated CUE/BIN structure".into(),
                structural: true,
                audio_payloads_complete: false,
                data_payloads_complete: false,
                notes: vec![
                    "payload hashes are not read by structural conversion".into(),
                    "GDI full-track mapping remains a separate future boundary".into(),
                ],
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpticalLayoutComparisonKind {
    ExactEquivalent,
    LayoutEquivalent,
    AudioEquivalent,
    DataEquivalent,
    PregapMismatch,
    IndexMismatch,
    TrackOrderMismatch,
    SectorModeMismatch,
    AudioMismatch,
    IncompleteEvidence,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpticalLayoutIssue {
    MissingStructuralEvidence,
    TrackCountMismatch,
    TrackOrderMismatch { left: Vec<u32>, right: Vec<u32> },
    DuplicateTrackNumber(u32),
    SectorModeMismatch { track: u32 },
    IndexMismatch { track: u32 },
    PregapMismatch { track: u32 },
    PostgapMismatch { track: u32 },
    UnprovenGapEncoding { track: u32 },
    AudioMismatch { track: u32 },
    AudioEvidenceMissing { track: u32 },
    DataMismatch { track: u32 },
    DataEvidenceMissing { track: u32 },
    RepresentationUnsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpticalLayoutComparison {
    pub kind: OpticalLayoutComparisonKind,
    pub issues: Vec<OpticalLayoutIssue>,
}

pub fn compare_optical_layouts(
    left: &OpticalLayout,
    right: &OpticalLayout,
) -> OpticalLayoutComparison {
    let mut issues = Vec::new();
    if !left.evidence.structural || !right.evidence.structural {
        issues.push(OpticalLayoutIssue::MissingStructuralEvidence);
    }
    if left.representation != OpticalLayoutRepresentation::CueBin
        || right.representation != OpticalLayoutRepresentation::CueBin
    {
        issues.push(OpticalLayoutIssue::RepresentationUnsupported);
    }
    for layout in [left, right] {
        let mut numbers = layout
            .tracks
            .iter()
            .map(|track| track.number)
            .collect::<Vec<_>>();
        numbers.sort_unstable();
        for pair in numbers.windows(2) {
            if pair[0] == pair[1] {
                issues.push(OpticalLayoutIssue::DuplicateTrackNumber(pair[0]));
            }
        }
    }
    if left.tracks.len() != right.tracks.len() {
        issues.push(OpticalLayoutIssue::TrackCountMismatch);
    }
    let left_numbers = left
        .tracks
        .iter()
        .map(|track| track.number)
        .collect::<Vec<_>>();
    let right_numbers = right
        .tracks
        .iter()
        .map(|track| track.number)
        .collect::<Vec<_>>();
    if left_numbers != right_numbers {
        issues.push(OpticalLayoutIssue::TrackOrderMismatch {
            left: left_numbers,
            right: right_numbers,
        });
    }

    let mut audio_mismatch = false;
    let mut data_mismatch = false;
    for (left_track, right_track) in left.tracks.iter().zip(&right.tracks) {
        if left_track.number != right_track.number {
            continue;
        }
        if left_track.mode != right_track.mode {
            issues.push(OpticalLayoutIssue::SectorModeMismatch {
                track: left_track.number,
            });
            continue;
        }
        let left_indexes = left_track
            .indexes
            .iter()
            .filter(|index| index.number != 0)
            .collect::<Vec<_>>();
        let right_indexes = right_track
            .indexes
            .iter()
            .filter(|index| index.number != 0)
            .collect::<Vec<_>>();
        if left_indexes != right_indexes {
            issues.push(OpticalLayoutIssue::IndexMismatch {
                track: left_track.number,
            });
        }
        compare_gaps(left_track, right_track, &mut issues);
        match left_track.mode {
            OpticalTrackMode::Audio => {
                match (&left_track.payload_sha256, &right_track.payload_sha256) {
                    (Some(left_hash), Some(right_hash)) if left_hash != right_hash => {
                        audio_mismatch = true;
                        issues.push(OpticalLayoutIssue::AudioMismatch {
                            track: left_track.number,
                        });
                    }
                    (None, _) | (_, None) => {
                        issues.push(OpticalLayoutIssue::AudioEvidenceMissing {
                            track: left_track.number,
                        });
                    }
                    _ => {}
                }
            }
            OpticalTrackMode::Data { .. } => {
                match (&left_track.payload_sha256, &right_track.payload_sha256) {
                    (Some(left_hash), Some(right_hash)) if left_hash != right_hash => {
                        data_mismatch = true;
                        issues.push(OpticalLayoutIssue::DataMismatch {
                            track: left_track.number,
                        });
                    }
                    (None, _) | (_, None) => issues.push(OpticalLayoutIssue::DataEvidenceMissing {
                        track: left_track.number,
                    }),
                    _ => {}
                }
            }
        }
    }
    let has_audio = left
        .tracks
        .iter()
        .any(|track| track.mode == OpticalTrackMode::Audio)
        || right
            .tracks
            .iter()
            .any(|track| track.mode == OpticalTrackMode::Audio);
    let audio_missing = issues
        .iter()
        .any(|issue| matches!(issue, OpticalLayoutIssue::AudioEvidenceMissing { .. }));
    if issues
        .iter()
        .any(|issue| matches!(issue, OpticalLayoutIssue::DuplicateTrackNumber(_)))
    {
        return result(OpticalLayoutComparisonKind::Unsupported, issues);
    }
    if audio_mismatch {
        return result(OpticalLayoutComparisonKind::AudioMismatch, issues);
    }
    if data_mismatch {
        return result(OpticalLayoutComparisonKind::Unsupported, issues);
    }
    if issues.iter().any(|issue| {
        matches!(
            issue,
            OpticalLayoutIssue::TrackCountMismatch | OpticalLayoutIssue::TrackOrderMismatch { .. }
        )
    }) {
        return result(OpticalLayoutComparisonKind::TrackOrderMismatch, issues);
    }
    if issues
        .iter()
        .any(|issue| matches!(issue, OpticalLayoutIssue::SectorModeMismatch { .. }))
    {
        return result(OpticalLayoutComparisonKind::SectorModeMismatch, issues);
    }
    if issues
        .iter()
        .any(|issue| matches!(issue, OpticalLayoutIssue::IndexMismatch { .. }))
    {
        return result(OpticalLayoutComparisonKind::IndexMismatch, issues);
    }
    if issues.iter().any(|issue| {
        matches!(
            issue,
            OpticalLayoutIssue::PregapMismatch { .. }
                | OpticalLayoutIssue::PostgapMismatch { .. }
                | OpticalLayoutIssue::UnprovenGapEncoding { .. }
        )
    }) {
        return result(OpticalLayoutComparisonKind::PregapMismatch, issues);
    }
    if issues
        .iter()
        .any(|issue| matches!(issue, OpticalLayoutIssue::RepresentationUnsupported))
    {
        return result(OpticalLayoutComparisonKind::Unsupported, issues);
    }
    if !issues.is_empty()
        && issues
            .iter()
            .any(|issue| matches!(issue, OpticalLayoutIssue::MissingStructuralEvidence))
    {
        return result(OpticalLayoutComparisonKind::IncompleteEvidence, issues);
    }
    if has_audio && audio_missing {
        return result(OpticalLayoutComparisonKind::IncompleteEvidence, issues);
    }
    let same_gap_encoding = left
        .tracks
        .iter()
        .zip(&right.tracks)
        .all(|(a, b)| a.gap == b.gap);
    let proven_gap_equivalence = left.tracks.iter().zip(&right.tracks).any(|(a, b)| {
        a.gap.encoding != b.gap.encoding
            && a.gap.proven_effective_pregap_frames.is_some()
            && a.gap.proven_effective_pregap_frames == b.gap.proven_effective_pregap_frames
    });
    let same_payload_evidence = left.tracks.iter().zip(&right.tracks).all(|(a, b)| {
        a.payload_sha256
            .as_ref()
            .zip(b.payload_sha256.as_ref())
            .is_some_and(|(left, right)| left == right)
    });
    if left.tracks.is_empty() || !same_payload_evidence {
        return result(OpticalLayoutComparisonKind::IncompleteEvidence, issues);
    }
    if same_gap_encoding && same_payload_evidence {
        return result(OpticalLayoutComparisonKind::ExactEquivalent, issues);
    }
    if proven_gap_equivalence {
        return result(OpticalLayoutComparisonKind::LayoutEquivalent, issues);
    }
    let has_data = left
        .tracks
        .iter()
        .any(|track| matches!(track.mode, OpticalTrackMode::Data { .. }));
    if has_data && !has_audio {
        return result(OpticalLayoutComparisonKind::DataEquivalent, issues);
    }
    if has_audio && !has_data {
        return result(OpticalLayoutComparisonKind::AudioEquivalent, issues);
    }
    result(OpticalLayoutComparisonKind::LayoutEquivalent, issues)
}

fn compare_gaps(
    left: &OpticalTrackLayout,
    right: &OpticalTrackLayout,
    issues: &mut Vec<OpticalLayoutIssue>,
) {
    if left.gap.postgap_frames != right.gap.postgap_frames {
        issues.push(OpticalLayoutIssue::PostgapMismatch { track: left.number });
    }
    if left.gap.encoding == right.gap.encoding {
        if left.gap.index_00_frame != right.gap.index_00_frame
            || left.gap.index_01_frame != right.gap.index_01_frame
            || left.gap.explicit_pregap_frames != right.gap.explicit_pregap_frames
        {
            issues.push(OpticalLayoutIssue::PregapMismatch { track: left.number });
        }
    } else if left.gap.proven_effective_pregap_frames.is_some()
        && left.gap.proven_effective_pregap_frames == right.gap.proven_effective_pregap_frames
    {
        // A caller-provided proof is the only allowed bridge between encoded
        // INDEX 00 and a synthetic PREGAP representation.
    } else {
        issues.push(OpticalLayoutIssue::UnprovenGapEncoding { track: left.number });
    }
}

fn result(
    kind: OpticalLayoutComparisonKind,
    issues: Vec<OpticalLayoutIssue>,
) -> OpticalLayoutComparison {
    OpticalLayoutComparison { kind, issues }
}

/// Compare one bounded observation of each local CUE sheet. Payload bytes are
/// never read or hashed; the result cannot establish preservation or freshness.
#[cfg(unix)]
pub fn compare_cue_paths(
    left: &std::path::Path,
    right: &std::path::Path,
) -> Result<OpticalLayoutComparison, String> {
    use std::io::Read;
    fn observe(path: &std::path::Path) -> Result<OpticalLayout, String> {
        use crate::launch::process_spawn::{CapturedFileIdentity, open_regular_file_nonblocking};
        let file = open_regular_file_nonblocking(path).map_err(|e| e.to_string())?;
        let before = CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?);
        let mut text = String::new();
        (&file)
            .take(262145)
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        if text.len() > 262144 {
            return Err("CUE exceeds 256 KiB.".into());
        }
        let layout = crate::ingestion::cue_bin::resolve_cue_layout_text(path, &text)
            .map_err(|e| e.to_string())?;
        crate::ingestion::cue_timeline::build_timeline(&layout)
            .map_err(|e| format!("CUE timeline refused: {e:?}"))?;
        if CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?) != before {
            return Err("CUE changed during observation; compare again.".into());
        }
        Ok(OpticalLayout::from_cue_layout(&layout))
    }
    Ok(compare_optical_layouts(&observe(left)?, &observe(right)?))
}
#[cfg(not(unix))]
pub fn compare_cue_paths(
    _: &std::path::Path,
    _: &std::path::Path,
) -> Result<OpticalLayoutComparison, String> {
    Err("Bounded no-follow CUE comparison is available on Unix only.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(number: u32, mode: OpticalTrackMode) -> OpticalTrackLayout {
        OpticalTrackLayout {
            order: number as usize,
            number,
            mode,
            referenced_file: format!("track{number}.bin").into(),
            indexes: vec![OpticalIndexPoint {
                number: 1,
                frame: number as u64 * 100,
            }],
            gap: OpticalGapLayout::none(Some(number as u64 * 100)),
            payload_sha256: None,
        }
    }

    fn layout(tracks: Vec<OpticalTrackLayout>) -> OpticalLayout {
        OpticalLayout {
            representation: OpticalLayoutRepresentation::CueBin,
            source: "disc.cue".into(),
            tracks,
            evidence: OpticalLayoutEvidence {
                source_description: "synthetic".into(),
                structural: true,
                audio_payloads_complete: true,
                data_payloads_complete: true,
                notes: Vec::new(),
            },
        }
    }

    #[test]
    fn proven_payload_identical_layout_is_exact() {
        let mut left = layout(vec![track(
            1,
            OpticalTrackMode::Data {
                sector_size: 2048,
                mode: OpticalDataMode::Mode1_2048,
            },
        )]);
        // Positive equivalence fixtures must supply payload evidence.
        left.tracks[0].payload_sha256 = Some("ab".repeat(32));
        assert_eq!(
            compare_optical_layouts(&left, &left).kind,
            OpticalLayoutComparisonKind::ExactEquivalent
        );
    }

    #[test]
    fn proven_payload_index_00_preserved_is_exact_and_removed_is_index_mismatch() {
        let mut left_track = track(
            1,
            OpticalTrackMode::Data {
                sector_size: 2352,
                mode: OpticalDataMode::Mode1_2352,
            },
        );
        left_track.payload_sha256 = Some("ab".repeat(32));
        left_track.gap.encoding = OpticalPregapEncoding::Index00;
        left_track.gap.index_00_frame = Some(0);
        let left = layout(vec![left_track.clone()]);
        assert_eq!(
            compare_optical_layouts(&left, &layout(vec![left_track.clone()])).kind,
            OpticalLayoutComparisonKind::ExactEquivalent
        );
        let mut removed = left_track;
        removed.gap.encoding = OpticalPregapEncoding::None;
        removed.gap.index_00_frame = None;
        assert_eq!(
            compare_optical_layouts(&left, &layout(vec![removed])).kind,
            OpticalLayoutComparisonKind::PregapMismatch
        );
    }

    #[test]
    fn proven_payload_explicit_pregap_and_index_00_need_proof() {
        let mut indexed = track(
            1,
            OpticalTrackMode::Data {
                sector_size: 2352,
                mode: OpticalDataMode::Mode1_2352,
            },
        );
        indexed.payload_sha256 = Some("ab".repeat(32));
        indexed.gap.encoding = OpticalPregapEncoding::Index00;
        indexed.gap.index_00_frame = Some(0);
        indexed.gap.index_01_frame = Some(150);
        let mut explicit = indexed.clone();
        explicit.gap.encoding = OpticalPregapEncoding::ExplicitPregap;
        explicit.gap.index_00_frame = None;
        explicit.gap.explicit_pregap_frames = Some(150);
        assert_eq!(
            compare_optical_layouts(
                &layout(vec![indexed.clone()]),
                &layout(vec![explicit.clone()])
            )
            .kind,
            OpticalLayoutComparisonKind::PregapMismatch
        );
        indexed.gap.proven_effective_pregap_frames = Some(150);
        explicit.gap.proven_effective_pregap_frames = Some(150);
        assert_eq!(
            compare_optical_layouts(&layout(vec![indexed]), &layout(vec![explicit])).kind,
            OpticalLayoutComparisonKind::LayoutEquivalent
        );
    }

    #[test]
    fn postgap_track_order_and_sector_mode_mismatch() {
        let mut left_track = track(
            1,
            OpticalTrackMode::Data {
                sector_size: 2048,
                mode: OpticalDataMode::Mode1_2048,
            },
        );
        left_track.gap.postgap_frames = Some(2);
        let right_track = track(
            1,
            OpticalTrackMode::Data {
                sector_size: 2352,
                mode: OpticalDataMode::Mode1_2352,
            },
        );
        assert_eq!(
            compare_optical_layouts(&layout(vec![left_track]), &layout(vec![right_track])).kind,
            OpticalLayoutComparisonKind::SectorModeMismatch
        );
        let mut a = track(1, OpticalTrackMode::Audio);
        a.gap.postgap_frames = Some(2);
        assert_eq!(
            compare_optical_layouts(
                &layout(vec![a]),
                &layout(vec![track(1, OpticalTrackMode::Audio)])
            )
            .kind,
            OpticalLayoutComparisonKind::PregapMismatch
        );
        assert_eq!(
            compare_optical_layouts(
                &layout(vec![
                    track(1, OpticalTrackMode::Audio),
                    track(2, OpticalTrackMode::Audio)
                ]),
                &layout(vec![
                    track(2, OpticalTrackMode::Audio),
                    track(1, OpticalTrackMode::Audio)
                ])
            )
            .kind,
            OpticalLayoutComparisonKind::TrackOrderMismatch
        );
    }

    #[test]
    fn mixed_mode_order_is_checked() {
        let left = layout(vec![
            track(
                1,
                OpticalTrackMode::Data {
                    sector_size: 2352,
                    mode: OpticalDataMode::Mode1_2352,
                },
            ),
            track(2, OpticalTrackMode::Audio),
        ]);
        let right = layout(vec![
            track(
                1,
                OpticalTrackMode::Data {
                    sector_size: 2352,
                    mode: OpticalDataMode::Mode1_2352,
                },
            ),
            track(2, OpticalTrackMode::Audio),
        ]);
        assert_eq!(
            compare_optical_layouts(&left, &right).kind,
            OpticalLayoutComparisonKind::IncompleteEvidence
        );
    }

    #[test]
    fn audio_hash_change_and_missing_evidence_fail_closed() {
        let mut left = track(1, OpticalTrackMode::Audio);
        let mut right = left.clone();
        left.payload_sha256 = Some("a".into());
        right.payload_sha256 = Some("b".into());
        assert_eq!(
            compare_optical_layouts(&layout(vec![left]), &layout(vec![right])).kind,
            OpticalLayoutComparisonKind::AudioMismatch
        );
        let right = track(1, OpticalTrackMode::Audio);
        assert_eq!(
            compare_optical_layouts(&layout(vec![right.clone()]), &layout(vec![right])).kind,
            OpticalLayoutComparisonKind::IncompleteEvidence
        );
    }

    #[test]
    fn duplicate_track_number_is_refused_by_comparison() {
        let left = layout(vec![
            track(1, OpticalTrackMode::Audio),
            track(1, OpticalTrackMode::Audio),
        ]);
        assert!(
            compare_optical_layouts(&left, &left)
                .issues
                .iter()
                .any(|issue| matches!(issue, OpticalLayoutIssue::DuplicateTrackNumber(1)))
        );
    }

    #[test]
    fn cue_projection_is_structural_and_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let cue = dir.path().join("disc.cue");
        let bin = dir.path().join("disc.bin");
        std::fs::write(&bin, vec![0_u8; 2048]).unwrap();
        let text = "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
        std::fs::write(&cue, text).unwrap();
        let before = std::fs::read(&cue).unwrap();
        let parsed = crate::ingestion::cue_bin::resolve_cue_layout(&cue).unwrap();
        let projection = OpticalLayout::from_cue_layout(&parsed);
        assert_eq!(projection.tracks.len(), 1);
        assert_eq!(std::fs::read(&cue).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn observed_comparison_retains_extra_indexes_without_payload_authority() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("disc.bin"), vec![0u8; 2352 * 10]).unwrap();
        let left = dir.path().join("left.cue");
        let right = dir.path().join("right.cue");
        let cue = "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n";
        std::fs::write(&left, cue).unwrap();
        std::fs::write(&right, cue).unwrap();
        assert_eq!(
            compare_cue_paths(&left, &right).unwrap().kind,
            OpticalLayoutComparisonKind::IncompleteEvidence
        );
        std::fs::write(&right, format!("{cue}INDEX 02 00:00:02\n")).unwrap();
        assert_eq!(
            compare_cue_paths(&left, &right).unwrap().kind,
            OpticalLayoutComparisonKind::IndexMismatch
        );
        assert_eq!(std::fs::read_to_string(&left).unwrap(), cue);
        assert_eq!(
            std::fs::read(dir.path().join("disc.bin")).unwrap(),
            vec![0u8; 2352 * 10]
        );
    }
    #[cfg(unix)]
    #[test]
    fn bounded_comparison_refuses_links_directories_and_large_sheets() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("big.cue");
        std::fs::write(&file, vec![b' '; 262145]).unwrap();
        assert!(compare_cue_paths(&file, &file).is_err());
        let link = d.path().join("linked.cue");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(compare_cue_paths(&link, &file).is_err());
        assert!(compare_cue_paths(d.path(), &file).is_err());
    }
}
