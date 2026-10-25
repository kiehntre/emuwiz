use super::*;
use archivefs_core::launch::{VerifiedWHDLoadTarget, WHDLoadProfileInput, WhdloadPackageFormat};
use archivefs_core::patch_manager::{AmigaEmulatorKind, AmigaKickstartState};

fn request() -> WHDLoadLaunchInput {
    // Deliberately blocked: retries must re-enter preflight, never run content.
    WHDLoadLaunchInput {
        identity: archivefs_core::launch::CanonicalIdentityStatus::Unknown,
        target: Some(VerifiedWHDLoadTarget {
            package_path: "/disposable/game.lha".into(),
            format: WhdloadPackageFormat::Lha,
        }),
        slave: None,
        profile: WHDLoadProfileInput {
            emulator: AmigaEmulatorKind::FsUae,
            profile_id: "synthetic".into(),
            executable: None,
            configuration: None,
            candidate_count: 1,
            eligible: false,
            kickstart: AmigaKickstartState::Missing,
            verified_identity: String::new(),
        },
    }
}

fn process() -> AmigaWHDLoadProcess {
    AmigaWHDLoadProcess::Running(
        archivefs_core::launch::spawn_amiga_whdload(
            archivefs_core::launch::process_spawn::PreparedProcessCommand {
                executable: "/bin/true".into(),
                arguments: vec![],
                working_directory: None,
            },
        )
        .unwrap(),
    )
}

fn state(stage: AmigaWHDLoadStage) -> AmigaWHDLoadLaunchState {
    AmigaWHDLoadLaunchState {
        tracked: Some(("/disposable/game.lha".into(), "fsuae".into(), stage)),
    }
}

fn render(state: &mut AmigaWHDLoadLaunchState) -> egui::FullOutput {
    egui::Context::default().run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_amiga_whdload_launch_action(ui, state, request());
        });
    })
}

fn contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn text(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(t) => t.galley.text().contains(needle),
            egui::Shape::Vec(v) => v.iter().any(|s| text(s, needle)),
            _ => false,
        }
    }
    output.shapes.iter().any(|s| text(&s.shape, needle))
}

#[test]
fn panel_exit_preserves_history_and_offers_retry() {
    let mut state = state(AmigaWHDLoadStage::Exited(process()));
    let output = render(&mut state);
    assert!(contains(&output, "WHDLoad process exited"));
    assert!(contains(&output, "Retry FS-UAE WHDLoad"));
    assert!(matches!(
        state.tracked,
        Some((_, _, AmigaWHDLoadStage::Exited(_)))
    ));
}

#[test]
fn panel_failure_preserves_error_and_offers_retry() {
    let mut state = state(AmigaWHDLoadStage::Failed("synthetic failure".into()));
    let output = render(&mut state);
    assert!(contains(&output, "synthetic failure"));
    assert!(contains(&output, "Retry FS-UAE WHDLoad"));
    assert_eq!(state.failure_detail().as_deref(), Some("synthetic failure"));
}

#[test]
fn panel_active_preserves_process_without_retry() {
    let (sender, receiver) = mpsc::channel();
    for stage in [
        AmigaWHDLoadStage::Starting(receiver),
        AmigaWHDLoadStage::Running(process()),
    ] {
        let mut state = state(stage);
        let output = render(&mut state);
        assert!(!contains(&output, "Retry FS-UAE"));
        assert!(!contains(&output, "Launch FS-UAE"));
        assert!(state.is_active());
    }
    drop(sender);
}

#[test]
fn game_details_repeat_dispatch_reenters_preflight_after_exit_and_failure() {
    for stage in [
        AmigaWHDLoadStage::Exited(process()),
        AmigaWHDLoadStage::Failed("old error".into()),
    ] {
        let mut state = state(stage);
        // The same typed dispatch used by Game Details, independently of panel UI.
        TypedLaunchRequest::AmigaWHDLoad(request()).start(
            &mut RetroArchLaunchState::default(),
            &mut DolphinLaunchState::default(),
            &mut Pcsx2LaunchState::default(),
            &mut StandaloneLaunchState::default(),
            &mut state,
        );
        assert!(state.is_active());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.is_active() && std::time::Instant::now() < deadline {
            state.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!state.is_active());
        assert_ne!(state.failure_detail().as_deref(), Some("old error"));
        assert!(
            state.failure_detail().is_some(),
            "fresh blocked input must refuse"
        );
    }
}

#[test]
fn shared_start_refuses_duplicates_even_after_selection_changes() {
    let (sender, receiver) = mpsc::channel();
    let mut state = state(AmigaWHDLoadStage::Starting(receiver));
    let original_path = state.tracked.as_ref().unwrap().0.clone();
    let mut other = request();
    other.target.as_mut().unwrap().package_path = "/disposable/other.lha".into();
    state.start(other);
    assert_eq!(state.tracked.as_ref().unwrap().0, original_path);
    assert!(state.is_active());
    drop(sender);
}
