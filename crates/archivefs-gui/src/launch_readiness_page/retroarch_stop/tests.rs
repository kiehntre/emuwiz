use super::super::tests::spawn_test_process;
use super::super::{RetroArchLaunchDisplay, RetroArchLaunchKey, show_retroarch_launch_feedback};
use super::*;
use archivefs_core::emulator_environment::retroarch::ProfileKind;
use archivefs_core::launch::{LaunchExecutionError, LaunchSpawnError, RetroArchLaunchRequest};
use std::{
    process::{Child, Command},
    time::Instant,
};

fn request() -> RetroArchLaunchRequest {
    super::super::tests::request_for("/library/Game.bin")
}

fn tracking(process: LaunchedRetroArchProcess) -> RetroArchLaunchState {
    RetroArchLaunchState {
        tracked: Some((
            RetroArchLaunchKey::from_request(&request()),
            RetroArchLaunchStage::running(process),
        )),
        ..Default::default()
    }
}

fn wait_for_exit(state: &mut RetroArchLaunchState) -> RetroArchLaunchDisplay {
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.is_active() {
        state.poll();
        assert!(Instant::now() < deadline, "the fixture never exited");
        std::thread::sleep(Duration::from_millis(5));
    }
    state.display_for(&request())
}

fn is_running(child: &mut Child) -> bool {
    child.try_wait().unwrap().is_none()
}

fn unrelated_sleep() -> Child {
    Command::new("/bin/sleep").arg("30").spawn().unwrap()
}

fn rendered(show: impl FnMut(&mut egui::Ui)) -> String {
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
    let mut show = show;
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| show(ui));
    });
    let mut text = String::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| collect(&clipped.shape, &mut text));
    text
}

#[test]
fn stop_needs_confirmation_and_sends_one_request_to_the_tracked_launch() {
    let mut process = spawn_test_process("/bin/sleep", &["30"]);
    let mut stop = StopControl::default();
    stop.request(&mut process);
    assert!(stop.outcome().is_none(), "unconfirmed Stop must do nothing");
    assert!(process.poll().is_none());

    let mut state = tracking(process);
    let first = state.stop_for_test();
    assert!(matches!(
        first,
        Some(GracefulStopOutcome::StillRunning { .. } | GracefulStopOutcome::ExitedAfterRequest)
    ));
    assert_eq!(state.stop_for_test(), first, "a repeat is not a request");

    let RetroArchLaunchDisplay::Exited {
        success,
        stop_outcome,
        ref status_detail,
        ..
    } = wait_for_exit(&mut state)
    else {
        panic!("expected an exited launch");
    };
    assert!(!success);
    assert_eq!(stop_outcome, first);
    assert!(status_detail.contains("15"), "{status_detail}");
    let feedback = rendered(|ui| {
        show_retroarch_launch_feedback(ui, &state.display_for(&request()));
    });
    assert!(feedback.contains("asked it to stop"), "{feedback}");
    assert!(feedback.contains("may not have been saved"), "{feedback}");
    assert!(!feedback.contains("unexpectedly"), "{feedback}");
}

#[test]
fn a_clean_exit_after_stop_is_reported_as_closed_normally() {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let script = format!(
        "trap 'exit 0' TERM; : > '{}'; while :; do sleep 0.01; done",
        ready.display()
    );
    let mut state = tracking(spawn_test_process("/bin/sh", &["-c", &script]));
    wait_for_file(&ready);
    state.stop_for_test();
    wait_for_exit(&mut state);
    let display = state.display_for(&request());
    assert!(matches!(
        display,
        RetroArchLaunchDisplay::Exited { success: true, .. }
    ));
    let feedback = rendered(|ui| show_retroarch_launch_feedback(ui, &display));
    assert!(
        feedback.contains("closed normally after you asked"),
        "{feedback}"
    );
}

#[test]
fn unrelated_processes_survive_even_if_the_diagnostic_pid_is_changed() {
    let mut unrelated = unrelated_sleep();
    let mut same_program = unrelated_sleep();
    let mut process = spawn_test_process("/bin/sleep", &["30"]);
    // The public `pid` is diagnostic: pointing it elsewhere redirects nothing.
    process.pid = unrelated.id();
    let mut state = tracking(process);

    state.stop_for_test();
    assert!(matches!(
        wait_for_exit(&mut state),
        RetroArchLaunchDisplay::Exited { .. }
    ));
    assert!(is_running(&mut unrelated));
    assert!(is_running(&mut same_program));
    for child in [&mut unrelated, &mut same_program] {
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

#[test]
fn another_games_card_can_only_reach_the_tracked_launch() {
    let mut unrelated = unrelated_sleep();
    let mut state = tracking(spawn_test_process("/bin/sleep", &["30"]));
    let other = super::super::tests::request_for("/library/OtherGame.bin");
    assert!(matches!(
        state.display_for(&other),
        RetroArchLaunchDisplay::Idle
    ));
    assert!(state.blocked_by_other_launch(&other));
    let text = rendered(|ui| state.show_stop_control(ui));
    assert!(text.contains("Stop RetroArch"), "{text}");
    assert!(
        text.contains("Game.bin"),
        "the running game is named: {text}"
    );

    state.stop_for_test();
    wait_for_exit(&mut state);
    assert!(!state.blocked_by_other_launch(&other));
    assert!(is_running(&mut unrelated));
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[test]
fn a_launch_that_already_exited_is_not_reported_as_stopped() {
    let mut process = spawn_test_process("/bin/true", &[]);
    super::super::tests::wait_until_exited(&mut process);
    // The stage still says Running: Stop reached a launch the poll has not
    // caught up with.
    let mut state = tracking(process);
    assert_eq!(
        state.stop_for_test(),
        Some(GracefulStopOutcome::AlreadyExited)
    );
    let display = wait_for_exit(&mut state);
    assert!(matches!(
        display,
        RetroArchLaunchDisplay::Exited { success: true, .. }
    ));
    let feedback = rendered(|ui| show_retroarch_launch_feedback(ui, &display));
    assert!(
        feedback.contains("RetroArch closed normally."),
        "{feedback}"
    );
    assert!(!feedback.contains("asked it to stop"), "{feedback}");
}

#[test]
fn a_process_ignoring_stop_gets_one_request_stays_tracked_and_is_never_escalated() {
    let dir = tempfile::tempdir().unwrap();
    let (release, ready, count) = (
        dir.path().join("release"),
        dir.path().join("ready"),
        dir.path().join("count"),
    );
    let script = format!(
        "trap 'printf x >> \"{count}\"' TERM; : > '{ready}'; while [ -d '{dir}' ] && [ ! -e '{release}' ]; do sleep 0.01; done",
        count = count.display(),
        ready = ready.display(),
        dir = dir.path().display(),
        release = release.display()
    );
    let mut state = tracking(spawn_test_process("/bin/sh", &["-c", &script]));
    wait_for_file(&ready);
    let first = state.stop_for_test();
    assert!(matches!(
        first,
        Some(GracefulStopOutcome::StillRunning {
            request_already_pending: false
        })
    ));
    std::thread::sleep(Duration::from_millis(200));
    state.poll();
    assert!(state.is_active(), "Play stays gated until exit is observed");
    assert!(matches!(
        state.display_for(&request()),
        RetroArchLaunchDisplay::Running { .. }
    ));
    // Neither the GUI nor Core's own repeat sends a second signal.
    state.stop_for_test();
    let Some((_, RetroArchLaunchStage::Running { process, .. })) = state.tracked.as_mut() else {
        panic!("expected a running launch");
    };
    process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO);
    wait_for_file(&count);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(std::fs::read(&count).unwrap(), b"x");
    let text = rendered(|ui| state.show_stop_control(ui));
    assert!(text.contains("has not closed yet"), "{text}");
    assert!(text.contains("will not force"), "{text}");
    assert!(!text.contains("I confirm"), "{text}");

    std::fs::write(&release, b"").unwrap();
    assert!(matches!(
        wait_for_exit(&mut state),
        RetroArchLaunchDisplay::Exited { success: true, .. }
    ));
}

#[test]
fn a_refused_request_keeps_the_launch_tracked_and_says_to_close_it() {
    let mut state = tracking(spawn_test_process("/bin/sleep", &["30"]));
    let Some((_, RetroArchLaunchStage::Running { process, stop })) = state.tracked.as_mut() else {
        panic!("expected a running launch");
    };
    assert_eq!(
        process.request_graceful_stop(GracefulStopScope::ProcessTree, Duration::ZERO),
        GracefulStopOutcome::Refused(GracefulStopRefusal::ProcessTreeNotSupervised)
    );
    assert!(process.poll().is_none(), "a refusal sends nothing");
    stop.outcome = Some(GracefulStopOutcome::Refused(
        GracefulStopRefusal::ProcessTreeNotSupervised,
    ));
    let text = rendered(|ui| state.show_stop_control(ui));
    assert!(text.contains("did not reach RetroArch"), "{text}");
    assert!(state.is_active());
    assert_eq!(
        requested_exit(
            Some(GracefulStopOutcome::Refused(
                GracefulStopRefusal::ProcessTreeNotSupervised
            )),
            false
        ),
        None,
        "a refused request never takes credit for the exit"
    );
    let Some((_, RetroArchLaunchStage::Running { process, .. })) = state.tracked.as_mut() else {
        panic!("expected a running launch");
    };
    process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO);
    wait_for_exit(&mut state);
}

/// A fixture launcher: a script, so the owned child is its interpreter.
fn launcher(dir: &std::path::Path, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("retroarch");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_str().unwrap().into()
}

fn assert_refused_and_still_tracked(state: &mut RetroArchLaunchState) {
    assert_eq!(
        state.stop_for_test(),
        Some(GracefulStopOutcome::Refused(
            GracefulStopRefusal::TargetNotEmulator
        ))
    );
    state.poll();
    assert!(state.is_active(), "a refusal must not end the launch");
    let text = rendered(|ui| state.show_stop_control(ui));
    assert!(text.contains("Stop is not available"), "{text}");
    assert!(!text.contains("I confirm"), "{text}");
}

fn release_and_assert_no_stop_claim(state: &mut RetroArchLaunchState, release: &std::path::Path) {
    std::fs::write(release, b"").unwrap();
    let display = wait_for_exit(state);
    let RetroArchLaunchDisplay::Exited { stop_outcome, .. } = &display else {
        panic!("expected an exited launch");
    };
    assert!(requested_exit(*stop_outcome, true).is_none());
    let feedback = rendered(|ui| show_retroarch_launch_feedback(ui, &display));
    assert!(!feedback.contains("asked it to stop"), "{feedback}");
}

fn wait_for_file(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "the fixture never started");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_launcher_script_is_refused_and_exit_never_reads_as_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let (ready, release) = (dir.path().join("ready"), dir.path().join("release"));
    let script = launcher(
        dir.path(),
        &format!(
            ": > '{}'; while [ -d '{}' ] && [ ! -e '{}' ]; do sleep 0.01; done",
            ready.display(),
            dir.path().display(),
            release.display()
        ),
    );
    let mut state = tracking(spawn_test_process(&script, &[]));
    wait_for_file(&ready);
    let text = rendered(|ui| state.show_stop_control(ui));
    assert!(text.contains("Stop is not available"), "{text}");
    assert_refused_and_still_tracked(&mut state);
    release_and_assert_no_stop_claim(&mut state, &release);
}

#[test]
fn a_launcher_that_execs_another_program_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let script = launcher(
        dir.path(),
        &format!(": > '{}'; exec /bin/sleep 30", ready.display()),
    );
    let process = spawn_test_process(&script, &[]);
    let pid = process.pid;
    let running = format!("/proc/{pid}/exe");
    let sleep = std::fs::canonicalize("/bin/sleep").unwrap();
    wait_for_file(&ready);
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read_link(&running).ok().as_ref() != Some(&sleep) {
        assert!(
            Instant::now() < deadline,
            "the launcher never replaced itself"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut state = tracking(process);
    assert_refused_and_still_tracked(&mut state);
    // Stop refused it, so the fixture is ended from outside.
    Command::new("kill").arg(pid.to_string()).status().unwrap();
    let display = wait_for_exit(&mut state);
    let RetroArchLaunchDisplay::Exited { stop_outcome, .. } = display else {
        panic!("expected an exited launch");
    };
    assert_eq!(requested_exit(stop_outcome, true), None);
}

#[test]
fn stop_never_reaches_a_launchers_child_or_claims_it_closed_when_the_launcher_exits() {
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    let (started, alive, let_go, child_gone) = (
        path("started"),
        path("alive"),
        path("let-go"),
        path("child-gone"),
    );
    std::fs::write(&alive, b"").unwrap();
    let script = launcher(
        dir.path(),
        &format!(
            "(: > '{started}'; while [ -e '{alive}' ]; do sleep 0.01; done; : > '{child_gone}') >/dev/null 2>&1 &\n\
             while [ -d '{dir}' ] && [ ! -e '{let_go}' ]; do sleep 0.01; done\nexit 0",
            started = started.display(),
            alive = alive.display(),
            child_gone = child_gone.display(),
            dir = dir.path().display(),
            let_go = let_go.display(),
        ),
    );
    let mut state = tracking(spawn_test_process(&script, &[]));
    wait_for_file(&started);
    assert_refused_and_still_tracked(&mut state);
    release_and_assert_no_stop_claim(&mut state, &let_go);
    assert!(
        state.stop_for_test().is_none(),
        "an exited launch has no Stop"
    );
    assert!(!rendered(|ui| state.show_stop_control(ui)).contains("Stop RetroArch"));
    assert!(
        !child_gone.exists(),
        "the launcher's child must be untouched"
    );
    std::fs::remove_file(&alive).unwrap();
    wait_for_file(&child_gone);
}

#[test]
fn replacing_or_deleting_the_program_on_disk_does_not_block_stop() {
    use std::os::unix::fs::PermissionsExt;
    for delete in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("retroarch");
        // A copied shell keeps working under any name; a multi-call
        // coreutils `sleep` would not.
        std::fs::copy(std::fs::canonicalize("/bin/sh").unwrap(), &program).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let script = format!(
            "while [ -d '{}' ]; do sleep 0.01; done",
            dir.path().display()
        );
        let process = spawn_test_process(program.to_str().unwrap(), &["-c", &script]);
        assert_eq!(process.stop_target(), OwnedExecutable::Matches);
        std::fs::remove_file(&program).unwrap();
        if !delete {
            std::fs::write(&program, b"a newer version").unwrap();
        }
        assert_eq!(process.stop_target(), OwnedExecutable::Matches);
        let mut state = tracking(process);
        assert!(matches!(
            state.stop_for_test(),
            Some(
                GracefulStopOutcome::StillRunning { .. } | GracefulStopOutcome::ExitedAfterRequest
            )
        ));
        wait_for_exit(&mut state);
    }
}

#[test]
fn the_stop_result_stays_visible_from_another_games_card() {
    let mut state = tracking(spawn_test_process("/bin/sleep", &["30"]));
    state.stop_for_test();
    let other = super::super::tests::request_for("/library/OtherGame.bin");
    assert!(matches!(
        wait_for_exit(&mut state),
        RetroArchLaunchDisplay::Exited { .. }
    ));
    assert!(matches!(
        state.display_for(&other),
        RetroArchLaunchDisplay::Idle
    ));
    let text = rendered(|ui| state.show_stop_control(ui));
    assert!(text.contains("Game.bin"), "{text}");
    assert!(text.contains("may not have been saved"), "{text}");
}

#[test]
fn an_appimage_launch_is_refused_even_when_its_file_is_running() {
    let mut process = spawn_test_process("/bin/sleep", &["30"]);
    assert_eq!(process.stop_target(), OwnedExecutable::Matches);
    process.command_facts.profile.profile_kind = ProfileKind::AppImage;
    assert_eq!(process.stop_target(), OwnedExecutable::Differs);
    let mut state = tracking(process);
    assert_refused_and_still_tracked(&mut state);
    let Some((_, RetroArchLaunchStage::Running { process, .. })) = state.tracked.as_mut() else {
        panic!("expected a running launch");
    };
    process.command_facts.profile.profile_kind = ProfileKind::Native;
    process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO);
    wait_for_exit(&mut state);
}

#[test]
fn no_stop_is_offered_without_a_running_launch() {
    let mut state = RetroArchLaunchState::default();
    assert!(!rendered(|ui| state.show_stop_control(ui)).contains("Stop RetroArch"));

    let _sender = state.track_starting_for_test(&request());
    assert!(!rendered(|ui| state.show_stop_control(ui)).contains("Stop RetroArch"));

    state.tracked = Some((
        RetroArchLaunchKey::from_request(&request()),
        RetroArchLaunchStage::Failed {
            error: LaunchExecutionError::Spawn(LaunchSpawnError::Spawn(std::io::Error::other(
                "fixture",
            ))),
        },
    ));
    assert!(!rendered(|ui| state.show_stop_control(ui)).contains("Stop RetroArch"));

    let mut state = tracking(spawn_test_process("/bin/true", &[]));
    wait_for_exit(&mut state);
    assert!(!rendered(|ui| state.show_stop_control(ui)).contains("Stop RetroArch"));
}
