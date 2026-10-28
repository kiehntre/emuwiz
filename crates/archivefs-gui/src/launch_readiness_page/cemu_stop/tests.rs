use super::*;
use archivefs_core::launch::{
    cemu_command::{CemuCommand, CemuCommandSelection},
    cemu_execution::spawn_cemu,
};
use std::{path::Path, time::Instant};

fn session(executable: &str, args: &[&str], content: &Path) -> Session {
    Session::new(
        spawn_cemu(CemuCommand {
            executable: executable.into(),
            arguments: args.iter().map(|arg| (*arg).into()).collect(),
            working_directory: None,
            selection: CemuCommandSelection {
                profile_id: "private-cemu-fixture".into(),
                platform_id: "WiiU".into(),
                content_path: content.into(),
                rpx_path: content.join("code/game.rpx"),
                title_id: Some("0005000010101A00".into()),
            },
        })
        .unwrap(),
    )
}

fn wait_for_exit(session: &mut Session) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.poll().is_none() {
        assert!(Instant::now() < deadline, "synthetic child did not exit");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn text(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                text.push_str(t.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(v) => {
                for s in v {
                    collect(s, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in &output.shapes {
        collect(&shape.shape, &mut text);
    }
    text
}

#[test]
fn cemu_stop_requires_confirmation_and_keeps_the_existing_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    let mut session = session("/bin/sleep", &["10"], root.path());
    session.request();
    assert!(session.outcome.is_none());
    assert!(session.poll().is_none());
    session.confirmed = true;
    session.request();
    assert!(!session.confirmed);
    assert!(matches!(
        session.outcome,
        Some(GracefulStopOutcome::StillRunning { .. } | GracefulStopOutcome::ExitedAfterRequest)
    ));
    let outcome = session.outcome;
    session.confirmed = true;
    session.request();
    assert_eq!(session.outcome, outcome);
    let mut state = StandaloneLaunchState {
        tracked: Some((
            root.path().into(),
            "Cemu".into(),
            StandaloneLaunchStage::Running(StandaloneProcess::Cemu(session)),
        )),
        ..Default::default()
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.is_active() {
        state.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let Some((_, _, StandaloneLaunchStage::Exited(StandaloneProcess::Cemu(mut session)))) =
        state.tracked
    else {
        panic!("exit watcher lost its Cemu process");
    };
    assert_eq!(session.outcome, outcome);
    assert!(exit_message(session.poll().unwrap(), outcome).starts_with("Exited"));
}

#[test]
fn cemu_failed_stop_keeps_running_and_cannot_redirect_to_a_foreign_pid() {
    let root = tempfile::tempdir().unwrap();
    let mut session = session("/bin/sleep", &["1"], root.path());
    session.process.pid = std::process::id();
    session.confirmed = true;
    session.request();
    assert!(matches!(
        session.outcome,
        Some(GracefulStopOutcome::Refused(_))
    ));
    assert!(session.poll().is_none());
    let context = egui::Context::default();
    let output = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| session.show(ui));
    });
    let rendered = text(&output);
    assert!(rendered.contains("Running · Stop failed"));
    assert!(rendered.contains("Stop refused"));
    assert!(!rendered.contains("Stopping Cemu"));
    wait_for_exit(&mut session);
}

#[test]
fn cemu_exit_without_stop_and_failed_exit_are_distinguished() {
    let root = tempfile::tempdir().unwrap();
    for (exe, prefix) in [("/bin/true", "Exited"), ("/bin/false", "Failed")] {
        let mut session = session(exe, &[], root.path());
        wait_for_exit(&mut session);
        assert!(exit_message(session.poll().unwrap(), None).starts_with(prefix));
        session.confirmed = true;
        session.request();
        assert_eq!(session.outcome, Some(GracefulStopOutcome::AlreadyExited));
    }
}

#[test]
fn cemu_ignored_stop_stays_stopping_until_the_watcher_observes_exit() {
    let root = tempfile::tempdir().unwrap();
    let script = "import pathlib,signal,sys,time; r=pathlib.Path(sys.argv[1]); signal.signal(signal.SIGTERM,signal.SIG_IGN); (r/'ready').touch(); deadline=time.monotonic()+10\nwhile not (r/'release').exists() and time.monotonic()<deadline: time.sleep(0.005)";
    let mut session = session(
        "/usr/bin/python3",
        &["-c", script, root.path().to_str().unwrap()],
        root.path(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.path().join("ready").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    session.confirmed = true;
    session.request();
    assert_eq!(
        session.outcome,
        Some(GracefulStopOutcome::StillRunning {
            request_already_pending: false
        })
    );
    session.confirmed = true;
    session.request();
    assert_eq!(
        session.outcome,
        Some(GracefulStopOutcome::StillRunning {
            request_already_pending: false
        })
    );
    assert!(session.poll().is_none());
    let context = egui::Context::default();
    let output = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| session.show(ui));
    });
    assert!(text(&output).contains("Stopping Cemu"));
    assert!(!text(&output).contains("Stop Cemu"));
    std::fs::write(root.path().join("release"), b"release").unwrap();
    wait_for_exit(&mut session);
    let outcome = session.outcome;
    assert!(exit_message(session.poll().unwrap(), outcome).starts_with("Exited"));
}

#[test]
fn cemu_stop_is_visible_at_small_size_without_a_current_readiness_plan() {
    let root = tempfile::tempdir().unwrap();
    let session = session("/bin/sleep", &["1"], root.path());
    let mut state = StandaloneLaunchState {
        tracked: Some((
            root.path().into(),
            "Cemu".into(),
            StandaloneLaunchStage::Running(StandaloneProcess::Cemu(session)),
        )),
        ..Default::default()
    };
    let context = egui::Context::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024., 600.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::super::show_launch_readiness_panel(
                    ui,
                    &super::super::LaunchReadinessInput::IdentityUnknown,
                    &mut Default::default(),
                    &mut Default::default(),
                    &mut Default::default(),
                    &mut state,
                    &mut Default::default(),
                );
            });
        },
    );
    let rendered = text(&output);
    assert!(rendered.contains("Stop Cemu"), "{rendered}");
    assert!(rendered.contains("I confirm"));
    assert!(rendered.contains("Identity unresolved"));
    // The old session keeps its own confirmation; it cannot leak to another
    // process or turn another launch's readiness into Stop authorization.
    let Some((_, _, StandaloneLaunchStage::Running(StandaloneProcess::Cemu(mut session)))) =
        state.tracked
    else {
        panic!("missing owned Cemu session");
    };
    assert!(!session.confirmed);
    wait_for_exit(&mut session);
}
