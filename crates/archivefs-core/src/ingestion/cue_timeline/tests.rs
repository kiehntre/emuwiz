use super::*;
use crate::ingestion::cue_bin::{CueError, resolve_cue_layout};
use std::fs;

const SECTOR: u64 = 2352;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(files: &[(&str, u64, u64)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (name, frames, sector) in files {
            // Sparse: only metadata matters; no data is ever read.
            let f = fs::File::create(dir.path().join(name)).unwrap();
            f.set_len(frames * sector).unwrap();
        }
        Self { dir }
    }
    fn cue(&self, name: &str, text: &str) -> PathBuf {
        let p = self.dir.path().join(name);
        fs::write(&p, text).unwrap();
        p
    }
    fn timeline(&self, text: &str) -> Result<DiscTimeline, String> {
        let cue = self.cue("t.cue", text);
        let layout = resolve_cue_layout(&cue).map_err(|e| format!("parse: {e}"))?;
        build_timeline(&layout).map_err(|e| format!("timeline: {e}"))
    }
}

const TWO_TRACK_INDEX00: &str = "FILE \"disc.bin\" BINARY\n\
  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n\
  TRACK 02 AUDIO\n    INDEX 00 00:00:50\n    INDEX 01 00:00:52\n";

#[test]
fn msf_75_fps_boundaries_use_checked_integers() {
    assert_eq!(CueTimestamp::parse("00:00:74").unwrap().frames, 74);
    assert_eq!(CueTimestamp::parse("00:01:00").unwrap().frames, 75);
    assert_eq!(
        CueTimestamp::parse("00:59:74").unwrap().frames,
        59 * 75 + 74
    );
    assert_eq!(CueTimestamp::parse("01:00:00").unwrap().frames, 4500);
    assert!(CueTimestamp::parse("00:00:75").is_err());
    assert!(CueTimestamp::parse("00:60:00").is_err());
    assert!(CueTimestamp::parse("+1:00:00").is_err());
    assert!(CueTimestamp::parse("1::00").is_err());
    assert_eq!(msf_to_frames(1, 0, 0), Some(4500));
    assert_eq!(msf_to_frames(0, 0, 75), None);
    for f in [0, 1, 74, 75, 4499, 4500, 123_456] {
        let text = frames_to_msf(f);
        assert_eq!(CueTimestamp::parse(&text).unwrap().frames, f, "{text}");
    }
}

#[test]
fn huge_timestamps_fail_closed() {
    assert!(CueTimestamp::parse("99999999999999999999:00:00").is_err());
    assert!(CueTimestamp::parse(&format!("{}:00:00", u64::MAX / 100)).is_err());
    assert_eq!(msf_to_frames(u64::MAX, 0, 0), None);
    // A position that fits u64 but overflows when turned into bytes.
    let fx = Fixture::new(&[("disc.bin", 10, SECTOR)]);
    let cue = fx.cue(
        "t.cue",
        "FILE \"disc.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n",
    );
    let mut layout = resolve_cue_layout(&cue).unwrap();
    layout.tracks[0].index_01 = Some(CueTimestamp {
        frames: u64::MAX / 2,
    });
    let refusal = build_timeline(&layout).unwrap_err();
    assert_eq!(refusal.kind, RefusalKind::Invalid);
    // Enormous reported file length: checked, not wrapped.
    let layout = resolve_cue_layout(&cue).unwrap();
    let huge = u64::MAX - (u64::MAX % SECTOR);
    let ok = build_timeline_with(&layout, |_| Ok(huge));
    assert!(ok.is_ok(), "{ok:?}");
}

#[test]
fn single_data_track() {
    let fx = Fixture::new(&[("disc.bin", 100, SECTOR)]);
    let t = fx
        .timeline("FILE \"disc.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n")
        .unwrap();
    assert_eq!(t.tracks.len(), 1);
    assert_eq!(
        t.tracks[0].program,
        SourceRange {
            start_frame: 0,
            frames: 100,
            byte_offset: 0,
            byte_len: 100 * SECTOR
        }
    );
    assert_eq!(t.tracks[0].pregap, PregapEvidence::None);
    assert_eq!(t.total_cue_frames, 100);
    assert!(!t.evidence.redump_equivalence_verified);
}

#[test]
fn index00_is_source_backed_and_ends_the_previous_track() {
    let fx = Fixture::new(&[("disc.bin", 100, SECTOR)]);
    let t = fx.timeline(TWO_TRACK_INDEX00).unwrap();
    let t2 = &t.tracks[1];
    assert_eq!(
        t2.pregap,
        PregapEvidence::SourceBacked(SourceRange {
            start_frame: 50,
            frames: 2,
            byte_offset: 50 * SECTOR,
            byte_len: 2 * SECTOR
        })
    );
    assert_eq!(t2.program.start_frame, 52);
    assert_eq!(t2.program.frames, 48);
    assert_eq!(
        t.tracks[0].program.frames, 50,
        "track 1 stops where INDEX 00 begins"
    );
    assert_eq!(t2.cue_frame_start, 50);
    assert_eq!(t2.cue_frame_index01, 52);
    assert_eq!(t2.lba_index01, 52);
    assert_eq!(t.evidence.source_backed_pregap_tracks, vec![2]);
    assert!(t.evidence.synthetic_pregap_tracks.is_empty());
    assert_eq!(t.total_cue_frames, 100, "INDEX 00 adds no frames");
}

#[test]
fn pregap_is_synthetic_and_moves_no_file_offset() {
    let fx = Fixture::new(&[("disc.bin", 100, SECTOR)]);
    let t = fx.timeline("FILE \"disc.bin\" BINARY\n  TRACK 01 MODE1/2352\n    PREGAP 00:02:00\n    INDEX 01 00:00:00\n").unwrap();
    let tr = &t.tracks[0];
    assert_eq!(tr.pregap, PregapEvidence::Synthetic { frames: 150 });
    assert_eq!(
        tr.program.byte_offset, 0,
        "PREGAP never shifts the file offset"
    );
    assert_eq!(tr.cue_frame_start, 0);
    assert_eq!(
        tr.cue_frame_index01, 150,
        "track 1 INDEX 01 is not disc zero"
    );
    assert_eq!(tr.lba_index01, 0, "but it is LBA 0");
    assert_eq!(t.total_cue_frames, 250);
    assert_eq!(t.evidence.synthetic_pregap_tracks, vec![1]);
}

#[test]
fn index00_plus_pregap_requires_review_and_is_never_summed() {
    let fx = Fixture::new(&[("disc.bin", 100, SECTOR)]);
    let err = fx.timeline("FILE \"disc.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    PREGAP 00:02:00\n    INDEX 00 00:00:50\n    INDEX 01 00:00:52\n").unwrap_err();
    assert!(
        err.starts_with("timeline: ReviewRequired (track 2)"),
        "{err}"
    );
}

#[test]
fn first_track_hidden_audio_is_represented_not_normalised() {
    let fx = Fixture::new(&[("disc.bin", 600, SECTOR)]);
    let t = fx.timeline("FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:04:00\n").unwrap();
    assert_eq!(t.tracks[0].cue_frame_start, 0);
    assert_eq!(
        t.tracks[0].cue_frame_index01, 300,
        "INDEX 01 of track 1 is not assumed to be disc zero"
    );
    assert_eq!(t.tracks[0].lba_index01, 0);
    assert_eq!(t.evidence.hidden_track_one_frames, Some(150));
    // A standard 2-second INDEX 00 is not "hidden".
    let t = fx.timeline("FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:02:00\n").unwrap();
    assert_eq!(t.evidence.hidden_track_one_frames, None);
}

#[test]
fn multi_file_timestamps_reset_per_file_and_build_a_global_timeline() {
    let fx = Fixture::new(&[
        ("t1.bin", 100, SECTOR),
        ("t2.bin", 200, SECTOR),
        ("t3.bin", 60, SECTOR),
    ]);
    let t = fx
        .timeline(
            "FILE \"t1.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n\
             FILE \"t2.bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:02:00\n\
             FILE \"t3.bin\" BINARY\n  TRACK 03 AUDIO\n    PREGAP 00:01:00\n    INDEX 01 00:00:00\n",
        )
        .unwrap();
    assert!(t.evidence.multi_file);
    let [a, b, c] = [&t.tracks[0], &t.tracks[1], &t.tracks[2]];
    assert_eq!((a.cue_frame_start, a.cue_frame_index01), (0, 0));
    // File 2 starts after file 1's 100 frames; its INDEX 00/01 are file-relative.
    assert_eq!((b.cue_frame_start, b.cue_frame_index01), (100, 250));
    assert_eq!(b.program.start_frame, 150);
    // File 3: 75 synthetic frames precede its data, after 100 + 200.
    assert_eq!((c.cue_frame_start, c.cue_frame_index01), (300, 375));
    assert_eq!(c.lba_index01, 375);
    assert_eq!(t.total_cue_frames, 435);
    assert_eq!(c.program.byte_offset, 0);
}

#[test]
fn mixed_data_audio_with_postgap_accounts_synthetic_frames() {
    let fx = Fixture::new(&[("d.bin", 100, SECTOR)]);
    let t = fx
        .timeline("FILE \"d.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n    POSTGAP 00:00:30\n  TRACK 02 AUDIO\n    INDEX 01 00:00:60\n")
        .unwrap();
    assert_eq!(t.tracks[0].synthetic_postgap_frames, 30);
    assert_eq!(t.tracks[0].cue_frame_end, 60);
    // Track 2's file frame 60 is pushed out by the 30 synthetic frames.
    assert_eq!(t.tracks[1].cue_frame_index01, 90);
    assert_eq!(t.total_cue_frames, 130);
    assert_eq!(t.evidence.synthetic_postgap_tracks, vec![1]);
}

#[test]
fn extra_indexes_are_kept_and_must_increase() {
    let fx = Fixture::new(&[("d.bin", 100, SECTOR)]);
    let t = fx.timeline("FILE \"d.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n    INDEX 02 00:00:20\n    INDEX 03 00:00:40\n").unwrap();
    assert_eq!(t.tracks[0].extra_indexes, vec![(2, 20), (3, 40)]);
    let err = fx.timeline("FILE \"d.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:10\n    INDEX 02 00:00:05\n").unwrap_err();
    assert!(err.contains("out of order"), "{err}");
    assert!(fx.timeline("FILE \"d.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n    INDEX 02 00:00:20\n    INDEX 02 00:00:30\n").is_err());
}

#[test]
fn invalid_orderings_and_bounds_are_rejected() {
    let fx = Fixture::new(&[("d.bin", 100, SECTOR)]);
    // INDEX 00 after INDEX 01.
    let e = fx.timeline("FILE \"d.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 00 00:00:30\n    INDEX 01 00:00:10\n").unwrap_err();
    assert!(e.contains("INDEX 00 must precede"), "{e}");
    // INDEX outside the source.
    let e = fx
        .timeline("FILE \"d.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:02:00\n")
        .unwrap_err();
    assert!(e.contains("outside FILE"), "{e}");
    // Decreasing INDEX 01 across tracks in one file.
    let e = fx.timeline("FILE \"d.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:40\n  TRACK 02 AUDIO\n    INDEX 01 00:00:20\n").unwrap_err();
    assert!(e.starts_with("timeline: Invalid"), "{e}");
    // Track whose INDEX 01 equals the boundary has no program frames.
    assert!(fx.timeline("FILE \"d.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n").is_err());
    // Non-consecutive numbering.
    assert!(
        fx.timeline("FILE \"d.bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n")
            .is_err()
    );
}

#[test]
fn truncated_and_missing_sources_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    // 100 frames and 5 stray bytes: not a whole-sector stream.
    fs::File::create(dir.path().join("t.bin"))
        .unwrap()
        .set_len(100 * SECTOR + 5)
        .unwrap();
    let cue = dir.path().join("t.cue");
    fs::write(
        &cue,
        "FILE \"t.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let layout = resolve_cue_layout(&cue).unwrap();
    let e = build_timeline(&layout).unwrap_err();
    assert_eq!(e.kind, RefusalKind::Invalid);
    assert!(e.reason.contains("whole-sector"), "{e}");
    // Missing file is refused by the canonical parser.
    fs::write(
        &cue,
        "FILE \"gone.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    assert!(matches!(
        resolve_cue_layout(&cue),
        Err(CueError::MissingDataFile(_))
    ));
    // File shrinks after parsing: the timeline re-checks lengths.
    fs::write(
        &cue,
        "FILE \"t.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(dir.path().join("t.bin"))
        .unwrap()
        .set_len(100 * SECTOR)
        .unwrap();
    let layout = resolve_cue_layout(&cue).unwrap();
    fs::File::options()
        .write(true)
        .open(dir.path().join("t.bin"))
        .unwrap()
        .set_len(0)
        .unwrap();
    assert!(build_timeline(&layout).is_err());
}

#[test]
fn unsupported_modes_and_file_types_are_refused_not_guessed() {
    let fx = Fixture::new(&[("d.bin", 10, SECTOR)]);
    let e = fx
        .timeline("FILE \"d.bin\" BINARY\n  TRACK 01 MODE2/2336\n    INDEX 01 00:00:00\n")
        .unwrap_err();
    assert!(e.contains("unsupported CUE track mode"), "{e}");
    let e = fx
        .timeline("FILE \"d.bin\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n")
        .unwrap_err();
    assert!(e.starts_with("timeline: Unsupported"), "{e}");
}

#[test]
fn canonical_rewrite_keeps_index00_and_pregap_distinct() {
    let fx = Fixture::new(&[("disc.bin", 100, SECTOR)]);
    // INDEX 00 survives as INDEX 00, never PREGAP.
    let cue = fx.cue(
        "a.cue",
        &format!(
            "REM COMMENT messy\n  {}",
            TWO_TRACK_INDEX00.replace("  TRACK", "TRACK")
        ),
    );
    let layout = resolve_cue_layout(&cue).unwrap();
    let before = build_timeline(&layout).unwrap();
    let text = render_canonical_cue(&layout).unwrap();
    assert!(
        text.contains("INDEX 00 00:00:50") && !text.contains("PREGAP"),
        "{text}"
    );
    let again = fx.cue("b.cue", &text);
    let after = build_timeline(&resolve_cue_layout(&again).unwrap()).unwrap();
    assert_eq!(before.semantic_signature(), after.semantic_signature());
    assert_eq!(
        render_canonical_cue(&resolve_cue_layout(&again).unwrap()).unwrap(),
        text,
        "idempotent"
    );

    // PREGAP survives as PREGAP, never INDEX 00.
    let cue = fx.cue("c.cue", "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2352\nPREGAP 00:02:00\nINDEX 01 00:00:00\nPOSTGAP 00:00:10\n");
    let layout = resolve_cue_layout(&cue).unwrap();
    let before = build_timeline(&layout).unwrap();
    let text = render_canonical_cue(&layout).unwrap();
    assert!(
        text.contains("PREGAP 00:02:00")
            && !text.contains("INDEX 00")
            && text.contains("POSTGAP 00:00:10"),
        "{text}"
    );
    let after = build_timeline(&resolve_cue_layout(&fx.cue("d.cue", &text)).unwrap()).unwrap();
    assert_eq!(before.semantic_signature(), after.semantic_signature());
    // And the two kinds really have different signatures.
    let i00 =
        build_timeline(&resolve_cue_layout(&fx.cue("e.cue", TWO_TRACK_INDEX00)).unwrap()).unwrap();
    assert_ne!(i00.semantic_signature(), after.semantic_signature());
}

#[test]
fn diagnostics_projection_names_the_source_backed_pregap() {
    let fx = Fixture::new(&[("disc.bin", 100, SECTOR)]);
    let t = fx.timeline(TWO_TRACK_INDEX00).unwrap();
    let text = t.describe();
    assert!(text.contains("Track 02 AUDIO"), "{text}");
    assert!(text.contains("Source: disc.bin"), "{text}");
    assert!(
        text.contains("Source-backed pregap: 00:00:02 via INDEX 00"),
        "{text}"
    );
    assert!(text.contains("Synthetic pregap: none"), "{text}");
    assert!(text.contains("Program area begins: INDEX 01"), "{text}");
}

#[test]
fn conversion_blocks_what_the_converter_cannot_preserve() {
    use crate::optical_preservation::source_layout;
    use crate::repair::optical_conversion::ChdConversionSourceMode;
    // Preservable baseline: agrees with the existing converter gate.
    let fx = Fixture::new(&[("d.bin", 100, 2048), ("e.bin", 100, SECTOR)]);
    let ok_cue = fx.cue(
        "ok.cue",
        "FILE \"d.bin\" BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00\n",
    );
    let layout = resolve_cue_layout(&ok_cue).unwrap();
    // The timeline model measures MODE1/2048 files at 2048-byte sectors.
    let t = build_timeline(&layout).unwrap();
    assert!(chd_conversion_preservation(&t).is_ok());
    assert!(source_layout(&ok_cue, ChdConversionSourceMode::KeepSource).is_ok());

    // Source-backed pregap: identified and blocked; the existing gate agrees.
    let fx2 = Fixture::new(&[("disc.bin", 100, SECTOR)]);
    let t = fx2.timeline(TWO_TRACK_INDEX00).unwrap();
    let blockers = chd_conversion_preservation(&t).unwrap_err();
    assert!(
        blockers.contains(&ConversionBlocker::SourceBackedPregap {
            track: 2,
            frames: 2
        }),
        "{blockers:?}"
    );
    assert!(blockers.contains(&ConversionBlocker::MultipleTracks));
    assert!(
        blockers
            .iter()
            .any(|b| b.reason().contains("source-backed pregap"))
    );

    // Single-track synthetic pregap: blocked with its own reason.
    let cue = fx2.cue("p.cue", "FILE \"disc.bin\" BINARY\n  TRACK 01 MODE1/2352\n    PREGAP 00:02:00\n    INDEX 01 00:00:00\n");
    let t = build_timeline(&resolve_cue_layout(&cue).unwrap()).unwrap();
    let blockers = chd_conversion_preservation(&t).unwrap_err();
    assert!(blockers.contains(&ConversionBlocker::SyntheticPregap {
        track: 1,
        frames: 150
    }));
    assert!(
        source_layout(&cue, ChdConversionSourceMode::KeepSource).is_err(),
        "downstream gate refuses too"
    );

    // Hidden track one audio.
    let fx3 = Fixture::new(&[("disc.bin", 600, SECTOR)]);
    let t = fx3.timeline("FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:04:00\n").unwrap();
    assert!(
        chd_conversion_preservation(&t)
            .unwrap_err()
            .contains(&ConversionBlocker::HiddenTrackOne { frames: 150 })
    );
}
