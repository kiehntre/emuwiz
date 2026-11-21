//! Blue must look at what RetroArch printed for failed saves before it lets
//! go of the capture, whatever the exit status. The output is real
//! RetroArch 1.21.0 output, replayed by a stand-in process.

use super::tests::spawn_test_process;
use super::*;
use std::time::{Duration, Instant};

pub(super) const SRAM_FAILED: &str = include_str!(
    "../../../archivefs-core/tests/fixtures/daedalus/retroarch-1.21.0/save-dir-read-only.log"
);
const SRAM_FAILED_QUIET: &str = include_str!(
    "../../../archivefs-core/tests/fixtures/daedalus/retroarch-1.21.0/save-dir-read-only-quiet.stderr"
);
pub(super) const SRAM_OK: &str =
    include_str!("../../../archivefs-core/tests/fixtures/daedalus/retroarch-1.21.0/save-ok.log");
const STATE_FAILED: &str = include_str!(
    "../../../archivefs-core/tests/fixtures/daedalus/retroarch-1.21.0/state-dir-read-only.log"
);

fn request() -> RetroArchLaunchRequest {
    super::tests::request_for("/library/Game.bin")
}

/// A stand-in RetroArch that prints `output` on standard error and exits
/// with `code`.
fn finished_launch(output: &str, code: i32) -> RetroArchLaunchState {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("output.log");
    std::fs::write(&file, output).unwrap();
    let script = format!("cat \"$1\" >&2; exit {code}");
    let process = spawn_test_process("/bin/sh", &["-c", &script, "sh", file.to_str().unwrap()]);
    let mut state = RetroArchLaunchState {
        tracked: Some((
            RetroArchLaunchKey::from_request(&request()),
            RetroArchLaunchStage::running(process),
        )),
        ..Default::default()
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.is_active() {
        state.poll();
        assert!(Instant::now() < deadline, "the stand-in never exited");
        std::thread::sleep(Duration::from_millis(5));
    }
    state
}

fn saves_of(display: &RetroArchLaunchDisplay) -> SaveCheck {
    match display {
        RetroArchLaunchDisplay::Exited { saves, .. } => saves.clone(),
        _ => panic!("expected an exited launch"),
    }
}

fn kept_output(state: &mut RetroArchLaunchState) -> usize {
    match state.tracked.as_mut() {
        Some((_, RetroArchLaunchStage::Exited { process, .. })) => {
            process.poll().map_or(0, |report| report.stderr.len())
        }
        _ => panic!("expected an exited launch"),
    }
}

pub(super) fn rendered(display: &RetroArchLaunchDisplay) -> String {
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(shape) => {
                text.push_str(shape.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, text)),
            _ => {}
        }
    }
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_retroarch_launch_feedback(ui, display);
        });
    });
    let mut text = String::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| collect(&clipped.shape, &mut text));
    text
}

#[test]
fn a_normal_exit_with_a_failed_save_warns_and_never_says_all_is_well() {
    let mut state = finished_launch(SRAM_FAILED, 0);
    // Every frame, not only the first: the warning must survive the frame
    // that would once have let go of the output.
    for _ in 0..3 {
        let display = state.display_for(&request());
        assert!(matches!(
            display,
            RetroArchLaunchDisplay::Exited { success: true, .. }
        ));
        assert!(
            matches!(saves_of(&display), SaveCheck::WriteFailed { .. }),
            "{:?}",
            saves_of(&display)
        );
        let text = rendered(&display);
        assert!(
            text.contains("Your progress may not have been saved"),
            "{text}"
        );
        assert!(
            text.contains("RetroArch reported that it could not write this game's save."),
            "{text}"
        );
        assert!(text.contains("RetroArch-recovery"), "{text}");
        assert!(!text.contains("RetroArch closed normally."), "{text}");
        assert!(text.contains("reported a save problem"), "{text}");
    }
    // The capture behind a save problem is kept for its details.
    assert!(kept_output(&mut state) > 0);
}

#[test]
fn a_failed_automatic_save_state_on_a_normal_exit_warns_as_a_save_state() {
    let mut state = finished_launch(STATE_FAILED, 0);
    let display = state.display_for(&request());
    let SaveCheck::WriteFailed { explanation, .. } = saves_of(&display) else {
        panic!("expected a write failure");
    };
    assert!(explanation.contains("save state"), "{explanation}");
    let text = rendered(&display);
    assert!(
        text.contains("Your progress may not have been saved"),
        "{text}"
    );
    assert!(!text.contains("RetroArch closed normally."), "{text}");
}

#[test]
fn a_failed_exit_still_shows_its_failed_save() {
    let mut state = finished_launch(SRAM_FAILED, 1);
    let display = state.display_for(&request());
    assert!(matches!(saves_of(&display), SaveCheck::WriteFailed { .. }));
    let text = rendered(&display);
    assert!(text.contains("exited unexpectedly"), "{text}");
    assert!(
        text.contains("Your progress may not have been saved"),
        "{text}"
    );
}

#[test]
fn a_successful_save_is_no_failure_detected_and_the_output_is_then_let_go() {
    let mut state = finished_launch(SRAM_OK, 0);
    let display = state.display_for(&request());
    let SaveCheck::NoFailureDetected { coverage } = saves_of(&display) else {
        panic!("expected no failure detected");
    };
    assert_eq!(
        coverage,
        "Save check: no error reported for in-game (battery) saves. Not checked: save states \
         and save files the emulator writes by itself."
    );
    let text = rendered(&display);
    assert!(text.contains("RetroArch closed normally."), "{text}");
    assert!(text.contains(&coverage), "{text}");
    assert!(!text.contains("may not have been saved"), "{text}");
    // Only now, after the check, is the capture dropped; the check stays.
    assert_eq!(kept_output(&mut state), 0);
    let again = state.display_for(&request());
    assert_eq!(saves_of(&again), SaveCheck::NoFailureDetected { coverage });
}

#[test]
fn missing_quiet_or_ambiguous_output_is_unknown_never_a_clean_bill() {
    let ambiguous = "[INFO] [Core]: Loading\n\
        [ERROR] [State]: Failed to save state to \"/scratch/states/game.state\".\n\
        [WARN] [SRAM]: could not save, will retry\n";
    for output in ["", SRAM_FAILED_QUIET, ambiguous] {
        let mut state = finished_launch(output, 0);
        let display = state.display_for(&request());
        assert_eq!(
            saves_of(&display),
            SaveCheck::Unavailable {
                coverage: "Save check: not available for this launch.".into()
            },
            "{output:?}"
        );
        let text = rendered(&display);
        assert!(
            text.contains("Save check: not available for this launch."),
            "{text}"
        );
        assert!(!text.contains("may not have been saved"), "{text}");
    }
}

#[test]
fn the_technical_details_of_a_save_problem_hide_private_paths() {
    let home = std::env::var("HOME").unwrap_or_default();
    let output = SRAM_FAILED.replace("/scratch/home", &home);
    let mut state = finished_launch(&output, 0);
    let RetroArchLaunchDisplay::Exited { stderr_tail, .. } = state.display_for(&request()) else {
        panic!("expected an exited launch");
    };
    let tail = stderr_tail.expect("a save problem keeps its details");
    assert!(tail.contains("Failed to save SRAM"), "{tail}");
    if home.len() > 1 {
        assert!(!tail.contains(&home), "{tail}");
    }
}

#[test]
fn a_stop_request_never_shows_a_success_banner_beside_a_failed_save() {
    use archivefs_core::launch::process_spawn::GracefulStopOutcome;
    let mut state = finished_launch(SRAM_FAILED, 0);
    let RetroArchLaunchDisplay::Exited {
        success,
        status_detail,
        stderr_tail,
        saves,
        ..
    } = state.display_for(&request())
    else {
        panic!("expected an exited launch");
    };
    let display = RetroArchLaunchDisplay::Exited {
        success,
        status_detail,
        stderr_tail,
        stop_outcome: Some(GracefulStopOutcome::ExitedAfterRequest),
        saves,
    };
    let text = rendered(&display);
    assert!(
        text.contains("Your progress may not have been saved"),
        "{text}"
    );
    assert!(text.contains("but it reported a save problem"), "{text}");
    assert!(
        !text.contains("closed normally after you asked it to stop"),
        "{text}"
    );
}
