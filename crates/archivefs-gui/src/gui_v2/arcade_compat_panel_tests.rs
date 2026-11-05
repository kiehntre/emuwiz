//! Synthetic catalogue and ZIPs in a private temp directory only.
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use super::*;

const VERSION: &str = "MAME 0.280 (mame0280)";
const CRC: &str = "ed82cd11"; // CRC-32 of "abcd"

fn fixture() -> (tempfile::TempDir, Inputs) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("listxml.xml"),
        format!(r#"<mame build="0.280"><machine name="game"><rom name="g.bin" size="4" crc="{CRC}"/></machine></mame>"#),
    )
    .unwrap();
    write_zip(&dir.path().join("game.zip"), b"abcd");
    std::fs::write(dir.path().join("mame"), b"never executed").unwrap();
    let inputs = Inputs {
        game_id: 7,
        machine: "game".into(),
        archive: dir.path().join("game.zip"),
        mame_catalogue: Some(dir.path().join("listxml.xml")),
        mame_version: VERSION.into(),
        mame_exe: dir.path().join("mame").to_string_lossy().into_owned(),
        fbneo_dat: None,
        fbneo_core: String::new(),
        fbneo_version: String::new(),
    };
    (dir, inputs)
}

fn write_zip(path: &Path, bytes: &[u8]) {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    writer
        .start_file("g.bin", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap();
}

fn finish(state: &mut State) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while state.running.is_some() {
        state.poll();
        assert!(Instant::now() < deadline, "check did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn nothing_runs_until_the_user_asks() {
    let (_dir, inputs) = fixture();
    let mut state = State::default();
    state.check_freshness(Some(&inputs));
    assert_eq!(state.view(), View::NoEvidenceYet);
    assert!(state.running.is_none());
}

#[test]
fn explicit_check_recommends_without_touching_the_choice() {
    let (_dir, inputs) = fixture();
    let mut state = State::default();
    state.start(inputs.clone());
    assert_eq!(state.view(), View::Checking);
    finish(&mut state);
    match state.view() {
        View::Recommended { emulator, .. } => assert_eq!(emulator, ArcadeEmulator::Mame),
        other => panic!("{other:?}"),
    }
    // The request carries no preference and no launch input at all.
    assert!(inputs.request().user_preference.is_none());
}

#[test]
fn missing_version_evidence_is_insufficient_not_failed() {
    let (_dir, mut inputs) = fixture();
    inputs.mame_version.clear();
    let mut state = State::default();
    state.start(inputs);
    finish(&mut state);
    assert_eq!(state.view(), View::Insufficient);
}

#[test]
fn wrong_content_is_never_recommended() {
    let (dir, inputs) = fixture();
    write_zip(&dir.path().join("game.zip"), b"zzzz");
    let mut state = State::default();
    state.start(inputs);
    finish(&mut state);
    // MAME mismatches, FBNeo was not evaluated: not recommended, and Core
    // does not call one-sided evidence a conflict.
    assert_eq!(state.view(), View::Insufficient);
}

#[test]
fn a_changed_archive_or_changed_evidence_discards_the_result() {
    let (dir, inputs) = fixture();
    let mut state = State::default();
    state.start(inputs.clone());
    finish(&mut state);
    state.check_freshness(Some(&inputs));
    assert!(matches!(state.view(), View::Recommended { .. }));

    let mut edited = inputs.clone();
    edited.mame_version = "MAME 0.281 (mame0281)".into();
    state.check_freshness(Some(&edited));
    assert!(matches!(state.view(), View::Stale(_)));

    state.start(inputs.clone());
    finish(&mut state);
    state.next_freshness = None;
    write_zip(&dir.path().join("game.zip"), b"abcd-and-more");
    state.check_freshness(Some(&inputs));
    assert!(matches!(state.view(), View::Stale(_)));
}

#[test]
fn errors_are_reported_and_a_new_run_clears_them() {
    let (dir, inputs) = fixture();
    std::fs::remove_file(dir.path().join("listxml.xml")).unwrap();
    let mut state = State::default();
    state.start(inputs.clone());
    finish(&mut state);
    assert!(matches!(state.view(), View::Failed(_)));
    state.start(inputs);
    assert_eq!(state.view(), View::Checking);
}

#[test]
fn dropping_a_running_check_cancels_it() {
    let (_dir, inputs) = fixture();
    let mut state = State::default();
    state.start(inputs);
    let cancel = Arc::clone(&state.running.as_ref().unwrap().cancel);
    state.running = None;
    assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn an_old_error_is_cleared_when_the_inputs_change() {
    let (dir, inputs) = fixture();
    std::fs::remove_file(dir.path().join("listxml.xml")).unwrap();
    let mut state = State::default();
    state.start(inputs.clone());
    finish(&mut state);
    state.check_freshness(Some(&inputs));
    assert!(matches!(state.view(), View::Failed(_)));
    let mut changed = inputs;
    changed.mame_version.clear();
    state.check_freshness(Some(&changed));
    assert_eq!(state.view(), View::NoEvidenceYet);
}

#[test]
fn version_text_with_a_missing_executable_is_never_a_recommendation() {
    let (_dir, mut inputs) = fixture();
    inputs.mame_exe = "/nonexistent/mame".into();
    let mut state = State::default();
    state.start(inputs);
    finish(&mut state);
    assert_eq!(state.view(), View::Insufficient);
}
