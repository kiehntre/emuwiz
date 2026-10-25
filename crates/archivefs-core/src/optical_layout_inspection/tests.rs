use super::*;
use std::fs;

const SECTOR: usize = 2352;

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write_bin(dir: &Path, name: &str, sectors: usize) {
    fs::write(dir.join(name), vec![0u8; sectors * SECTOR]).unwrap();
}

fn cue(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("game.cue");
    fs::write(&path, text).unwrap();
    path
}

use std::path::PathBuf;

const SIMPLE: &str = "FILE \"game.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n";

#[test]
fn non_optical_extension_is_not_inspected() {
    assert!(inspect_optical_layout(Path::new("x.iso")).is_none());
}

#[test]
fn valid_cue_lists_tracks_and_states_limits() {
    let d = dir();
    write_bin(d.path(), "game.bin", 300);
    let r = inspect_optical_layout(&cue(d.path(), SIMPLE)).unwrap();
    assert_eq!(r.state, OpticalLayoutState::Valid);
    assert_eq!(r.tracks.len(), 1);
    assert_eq!(r.tracks[0].frames, Some(300));
    assert_eq!(r.tracks[0].companion, "game.bin");
    assert!(r.basis.contains("No file content was read or hashed"));
    assert!(r.limits.iter().any(|l| l.contains("Not byte verification")));
    assert!(r.notes.is_empty());
}

#[test]
fn missing_companion_is_missing() {
    let d = dir();
    let r = inspect_optical_layout(&cue(d.path(), SIMPLE)).unwrap();
    assert_eq!(r.state, OpticalLayoutState::Missing);
    assert_eq!(r.companions[0].status, CompanionStatus::Missing);
}

#[test]
fn case_only_lookalike_is_ambiguous_and_not_chosen() {
    let d = dir();
    write_bin(d.path(), "GAME.BIN", 10);
    let r = inspect_optical_layout(&cue(d.path(), SIMPLE)).unwrap();
    assert_eq!(r.state, OpticalLayoutState::Ambiguous);
    assert_eq!(
        r.companions[0].status,
        CompanionStatus::Ambiguous {
            candidates: vec!["GAME.BIN".into()]
        }
    );
    assert!(r.tracks.is_empty());
}

#[test]
fn truncated_companion_is_mismatch() {
    let d = dir();
    fs::write(d.path().join("game.bin"), vec![0u8; SECTOR + 7]).unwrap();
    let r = inspect_optical_layout(&cue(d.path(), SIMPLE)).unwrap();
    assert_eq!(r.state, OpticalLayoutState::Mismatch);
}

#[test]
fn index_beyond_file_is_mismatch() {
    let d = dir();
    write_bin(d.path(), "game.bin", 10);
    let text = "FILE \"game.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:05:00\n";
    let r = inspect_optical_layout(&cue(d.path(), text)).unwrap();
    assert_eq!(r.state, OpticalLayoutState::Mismatch, "{}", r.summary);
}

#[test]
fn parent_traversal_is_unsafe_and_not_opened() {
    let d = dir();
    let text = "FILE \"../x.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n";
    let r = inspect_optical_layout(&cue(d.path(), text)).unwrap();
    assert_eq!(r.state, OpticalLayoutState::Unsafe);
}

#[test]
fn index00_and_pregap_together_needs_review() {
    let d = dir();
    write_bin(d.path(), "game.bin", 600);
    let text = "FILE \"game.bin\" BINARY\n  TRACK 01 MODE2/2352\n    PREGAP 00:02:00\n    INDEX 00 00:00:00\n    INDEX 01 00:02:00\n";
    let r = inspect_optical_layout(&cue(d.path(), text)).unwrap();
    assert_eq!(r.state, OpticalLayoutState::Ambiguous);
    assert!(r.notes[0].contains("never summed"));
}

#[test]
fn index00_is_reported_as_stored_not_synthetic() {
    let d = dir();
    write_bin(d.path(), "a.bin", 600);
    let text = "FILE \"a.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 00 00:04:00\n    INDEX 01 00:06:00\n";
    let r = inspect_optical_layout(&cue(d.path(), text)).unwrap();
    assert_eq!(r.state, OpticalLayoutState::Valid);
    assert!(r.tracks[1].gap.starts_with("INDEX 00"));
    assert!(r.tracks[1].gap.contains("stored in the file"));
    assert!(!r.notes.is_empty());
}

fn gdi(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("disc.gdi");
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn valid_gdi_lists_tracks_and_has_no_pregap_claim() {
    let d = dir();
    write_bin(d.path(), "track01.raw", 4);
    write_bin(d.path(), "track03.bin", 8);
    let r = inspect_optical_layout(&gdi(
        d.path(),
        "2\n1 0 4 2352 track01.raw 0\n2 45000 4 2352 track03.bin 0\n",
    ))
    .unwrap();
    assert_eq!(r.state, OpticalLayoutState::Valid, "{}", r.summary);
    assert_eq!(r.tracks[1].start, "LBA 45000");
    assert_eq!(r.tracks[1].frames, Some(8));
    assert!(r.notes[0].contains("no INDEX 00 or PREGAP"));
}

#[test]
fn gdi_lists_every_missing_track() {
    let d = dir();
    write_bin(d.path(), "track01.raw", 4);
    let r = inspect_optical_layout(&gdi(
        d.path(),
        "2\n1 0 4 2352 track01.raw 0\n2 45000 4 2352 track03.bin 0\n",
    ))
    .unwrap();
    assert_eq!(r.state, OpticalLayoutState::Missing);
    assert_eq!(r.companions.len(), 2);
    assert!(matches!(
        r.companions[0].status,
        CompanionStatus::Present { .. }
    ));
    assert_eq!(r.companions[1].status, CompanionStatus::Missing);
}

#[test]
fn gdi_misaligned_track_is_mismatch() {
    let d = dir();
    write_bin(d.path(), "track01.raw", 4);
    fs::write(d.path().join("track03.bin"), vec![0u8; SECTOR * 8 + 1]).unwrap();
    let r = inspect_optical_layout(&gdi(
        d.path(),
        "2\n1 0 4 2352 track01.raw 0\n2 45000 4 2352 track03.bin 0\n",
    ))
    .unwrap();
    assert_eq!(r.state, OpticalLayoutState::Mismatch);
}

#[test]
fn inspection_writes_nothing() {
    let d = dir();
    write_bin(d.path(), "game.bin", 300);
    let c = cue(d.path(), SIMPLE);
    let before: Vec<_> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    inspect_optical_layout(&c);
    let mut after: Vec<_> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let mut before = before;
    before.sort();
    after.sort();
    assert_eq!(before, after);
}
