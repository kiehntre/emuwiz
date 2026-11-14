//! The drawn launch input is reused across frames, dropped the moment
//! anything it was built from changes, and never used to start a game.
use super::*;
use std::time::{Duration, Instant};

/// A selected game whose evidence is ready and whose emulator scans have all
/// answered, as on a page that has been open for a moment. The file is of no
/// known system, so nothing here can be launched.
fn settled(directory: &Path) -> (NativeWorkflows, PathBuf) {
    let path = directory.join("Unknown Thing.zzz");
    std::fs::write(&path, b"not a game").unwrap();
    let report = selected_evidence_page::gather_selected_evidence_fast(&path, None).unwrap();
    let mut workflows = NativeWorkflows::new(egui::Context::default());
    let readiness = &mut workflows.app.emulator_readiness;
    readiness.retroarch_profiles =
        RetroArchProfilesState::Ready(crate::tests::cheat_discovery(vec![
            crate::tests::cheat_profile("native-user", true),
        ]));
    readiness.dolphin_local_profiles = DolphinLocalProfilesState::Error("none".into());
    readiness.pcsx2_launch_profiles = Pcsx2LaunchProfilesState::Error("none".into());
    readiness.pcsx2_firmware_evidence = Pcsx2FirmwareEvidenceState::Ready(Vec::new());
    workflows.selected_game = Some(1);
    workflows.select(&path);
    workflows.app.selected_evidence_ui.selected_evidence =
        selected_evidence_page::SelectedEvidenceState::Ready {
            generation: 1,
            report: Box::new(report),
            hasheous: selected_evidence_page::HasheousState::Idle,
        };
    workflows.launch_input_memo.freeze(Instant::now());
    (workflows, path)
}

/// Moves the stopped clock forward.
fn advance(workflows: &NativeWorkflows, by: Duration) {
    let memo = &workflows.launch_input_memo;
    memo.freeze(memo.now() + by);
}

fn builds(workflows: &NativeWorkflows) -> u64 {
    workflows.launch_input_memo.builds()
}

#[test]
fn drawing_many_frames_builds_the_launch_input_once() {
    let directory = tempfile::tempdir().unwrap();
    let (workflows, path) = settled(directory.path());
    // Sixty frames in a quarter of a second, each asking the way the pages
    // do: the summary, the save-state target and a third reader.
    for _ in 0..60 {
        let _ = workflows.readiness_summary(1, &path);
        let _ = workflows.save_state_target(1, &path);
        workflows.shown_launch_input();
        advance(&workflows, Duration::from_millis(4));
    }
    assert_eq!(builds(&workflows), 1, "180 reads, one build");
    // Left alone, it is rebuilt on its timer: once here, then reused again.
    advance(&workflows, Duration::from_millis(20));
    workflows.shown_launch_input();
    workflows.shown_launch_input();
    assert_eq!(builds(&workflows), 2);
}

#[test]
fn the_key_changes_with_everything_the_drawn_input_depends_on() {
    let directory = tempfile::tempdir().unwrap();
    let (mut workflows, path) = settled(directory.path());
    let now = workflows.launch_input_memo.now();
    let base = workflows
        .launch_input_key(now)
        .expect("a settled game has a key");
    assert_eq!(workflows.launch_input_key(now), Some(base), "stable");

    // Another game, or the same game at another path.
    workflows.selected_game = Some(2);
    assert_ne!(workflows.launch_input_key(now), Some(base));
    workflows.selected_game = Some(1);
    workflows.selected = Some(directory.path().join("other.zzz"));
    assert_ne!(workflows.launch_input_key(now), Some(base));
    workflows.selected = Some(path.clone());
    assert_eq!(workflows.launch_input_key(now), Some(base));

    // Identity evidence: a new check, and the deferred checksum finishing.
    let selected_evidence_page::SelectedEvidenceState::Ready { generation, .. } =
        &mut workflows.app.selected_evidence_ui.selected_evidence
    else {
        unreachable!()
    };
    *generation = 2;
    let after_recheck = workflows.launch_input_key(now);
    assert_ne!(after_recheck, Some(base));
    let selected_evidence_page::SelectedEvidenceState::Ready { report, .. } =
        &mut workflows.app.selected_evidence_ui.selected_evidence
    else {
        unreachable!()
    };
    report.enrichment =
        selected_evidence_page::SelectedEvidenceEnrichmentStatus::Failed("unreadable".into());
    let after_checksum = workflows.launch_input_key(now);
    assert_ne!(after_checksum, after_recheck);

    // Emulator configuration: a path override and a remembered choice.
    workflows
        .app
        .emulator_readiness
        .retroarch_core_directory_override = Some(directory.path().join("cores"));
    let after_override = workflows.launch_input_key(now);
    assert_ne!(after_override, after_checksum);

    // The library and what the catalogue says about this file.
    workflows.app.database_generation.0 += 1;
    let after_library = workflows.launch_input_key(now);
    assert_ne!(after_library, after_override);
    workflows.app.catalogued_platform_hint = Some((path.clone(), "SNES".into()));
    let after_hint = workflows.launch_input_key(now);
    assert_ne!(after_hint, after_library);
    workflows.app.launch_system_trial = Some((path.clone(), "snes"));
    assert_ne!(workflows.launch_input_key(now), after_hint);
}

#[test]
fn nothing_is_reused_while_evidence_or_an_emulator_scan_is_unfinished() {
    let directory = tempfile::tempdir().unwrap();
    let (mut workflows, path) = settled(directory.path());
    let now = workflows.launch_input_memo.now();
    workflows.shown_launch_input();
    assert_eq!(builds(&workflows), 1);

    // Firmware evidence goes back to loading: no key, a build per read, and
    // the answer from before is not kept for afterwards.
    workflows.app.emulator_readiness.pcsx2_firmware_evidence =
        Pcsx2FirmwareEvidenceState::NotLoaded;
    assert_eq!(workflows.launch_input_key(now), None);
    workflows.shown_launch_input();
    workflows.shown_launch_input();
    assert_eq!(builds(&workflows), 3);
    workflows.app.emulator_readiness.pcsx2_firmware_evidence =
        Pcsx2FirmwareEvidenceState::Ready(Vec::new());
    workflows.shown_launch_input();
    assert_eq!(builds(&workflows), 4, "rebuilt once the scan answered");
    workflows.shown_launch_input();
    assert_eq!(builds(&workflows), 4);

    // A failed evidence check for the same game: again no key.
    workflows.app.selected_evidence_ui.selected_evidence =
        selected_evidence_page::SelectedEvidenceState::Error {
            generation: 2,
            path,
            message: "gone".into(),
        };
    assert_eq!(workflows.launch_input_key(now), None);
}

#[test]
fn play_and_the_bezel_preview_build_their_own_input_and_never_read_the_drawn_one() {
    let directory = tempfile::tempdir().unwrap();
    let (mut workflows, path) = settled(directory.path());
    workflows.shown_launch_input();
    assert_eq!(builds(&workflows), 1);

    // Play, pressed while the drawn answer is still inside its reuse window.
    assert_ne!(workflows.try_play(1), PlayOutcome::Started);
    assert_eq!(builds(&workflows), 2, "Play built a new input");
    assert!(!workflows.launch_active());
    assert_ne!(workflows.try_play(1), PlayOutcome::Started);
    assert_eq!(builds(&workflows), 3, "and builds again on every press");

    let _ = workflows.retroarch_bezel_request(1, &path);
    assert_eq!(builds(&workflows), 4, "the bezel preview built its own");

    // None of those replaced or refreshed what the page is drawing.
    workflows.shown_launch_input();
    assert_eq!(builds(&workflows), 4);
}

#[test]
fn an_explicit_recheck_forgets_the_drawn_input() {
    let directory = tempfile::tempdir().unwrap();
    let (workflows, _path) = settled(directory.path());
    workflows.shown_launch_input();
    workflows.launch_input_memo.forget();
    workflows.shown_launch_input();
    assert_eq!(builds(&workflows), 2);
}

#[test]
fn the_library_and_the_drawn_input_share_one_environment_signature() {
    let directory = tempfile::tempdir().unwrap();
    let (mut workflows, _path) = settled(directory.path());
    let now = Instant::now();
    let signature = workflows.environment_signature(now);
    assert!(signature.is_some());
    assert_eq!(
        signature,
        crate::gui_v2::bg_readiness::EnvSnapshot::signature(&workflows.app.emulator_readiness),
        "the value the background readiness checks are invalidated by"
    );
    // A scan starting again withdraws it at once, for both users.
    workflows.app.emulator_readiness.pcsx2_firmware_evidence =
        Pcsx2FirmwareEvidenceState::NotLoaded;
    assert_eq!(workflows.environment_signature(now), None);
}
