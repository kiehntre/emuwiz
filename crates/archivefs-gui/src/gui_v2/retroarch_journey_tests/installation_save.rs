//! Selection-save completion through the normal update while Setup is absent.
use super::*;
use crate::emulator_setup::RetroArchProfilesState;

#[test]
fn stale_installation_save_completion_revokes_cached_guidance() {
    let profile = Profile::new();
    let before = profile.snapshot();
    profile.run("save-stale", None);
    assert_eq!(profile.snapshot(), before);
}

#[test]
fn failed_installation_save_is_reported_on_return_and_refreshes_discovery() {
    for scenario in ["save-missing", "save-write-failed"] {
        let profile = Profile::new();
        let before = profile.snapshot();
        profile.run(scenario, None);
        assert_eq!(profile.snapshot(), before);
    }
}

pub(super) fn assert_guidance_withdrawn(blue: &Blue, fixture: &VerifiedGameFixture) {
    let workflows = blue.app.native_workflows.as_ref().unwrap();
    let inner = &workflows.app;
    let live = match &inner.state {
        crate::LoadState::Ready(data) => Some(data.as_ref()),
        _ => None,
    };
    let input = inner.build_launch_readiness_input(live);
    assert!(
        !matches!(
            crate::launch_readiness_page::gamer_play_action(&input),
            crate::launch_readiness_page::GamerPlayAction::Launch(_)
        ),
        "neither plain nor cheat Play may use the previous selection"
    );
    assert!(
        workflows
            .save_state_target(blue.game, &fixture.rom_path)
            .is_none()
    );
    assert!(workflows.retroarch_bezel_scope().is_none());
    assert!(
        workflows
            .retroarch_bezel_request(blue.game, &fixture.rom_path)
            .is_err()
    );
}

pub(super) fn refused_save(
    blue: &mut Blue,
    fixture: &VerifiedGameFixture,
    root: &Path,
    scenario: &str,
) {
    let old_request = current_retroarch_request(blue);
    let sidecar = archivefs_core::app_dirs::config_path(
        archivefs_core::emulator_environment::retroarch::installation_selection::EXECUTABLE_OVERRIDE_FILE,
    ).unwrap();
    assert!(sidecar.starts_with(root));
    assert!(!sidecar.exists());
    let selected = if scenario == "save-missing" {
        root.join("removed/retroarch")
    } else {
        root.join("retroarch/bin/retroarch")
    };
    if scenario == "save-write-failed" {
        // A non-file selection sidecar must refuse, never enable automatic fallback.
        fs::create_dir_all(&sidecar).unwrap();
    }
    let context = blue.context.clone();
    blue.app
        .native_workflows
        .as_mut()
        .unwrap()
        .app
        .emulator_readiness
        .retroarch_installation_picker
        .start(Some(selected.clone()), context.clone());
    if scenario == "save-stale" {
        // Let the real worker publish its choice without rendering or polling Setup,
        // then remove the program before the controller consumes completion.
        let deadline = Instant::now() + Duration::from_secs(20);
        while !sidecar.is_file() {
            assert!(Instant::now() < deadline, "save worker did not publish");
            std::thread::sleep(Duration::from_millis(20));
        }
        fs::remove_file(&selected).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let inner = &mut blue.app.native_workflows.as_mut().unwrap().app;
        crate::app_polling::poll_and_reconcile(inner, &context);
        if matches!(
            inner.emulator_readiness.retroarch_profiles,
            RetroArchProfilesState::Scanning { .. }
        ) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "offscreen save did not refresh discovery"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_guidance_withdrawn(blue, fixture);
    loop {
        let inner = &mut blue.app.native_workflows.as_mut().unwrap().app;
        crate::app_polling::poll_and_reconcile(inner, &context);
        if !matches!(
            inner.emulator_readiness.retroarch_profiles,
            RetroArchProfilesState::Scanning { .. }
        ) {
            break;
        }
        assert!(Instant::now() < deadline, "refresh did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
    let inner = &mut blue.app.native_workflows.as_mut().unwrap().app;
    let output = context.run(Default::default(), |context| {
        eframe::egui::CentralPanel::default().show(context, |ui| {
            inner.show_retroarch_core_folder_card(ui, context, false);
        });
    });
    if scenario == "save-missing" {
        assert!(matches!(
            inner.emulator_readiness.retroarch_profiles,
            RetroArchProfilesState::Ready(_)
        ));
        assert!(
            texts(&output)
                .iter()
                .any(|text| text.contains("unavailable"))
        );
        assert!(
            !sidecar.exists(),
            "failed validation never persists a selection"
        );
        // The unchanged automatic choice is freshly discovered, not silently
        // substituted for a saved but stale explicit choice.
        assert_eq!(
            current_retroarch_request(blue).expected_executable_binding,
            old_request.expected_executable_binding
        );
    } else {
        assert!(matches!(
            inner.emulator_readiness.retroarch_profiles,
            RetroArchProfilesState::Error(_)
        ));
        if scenario == "save-write-failed" {
            assert!(
                texts(&output)
                    .iter()
                    .any(|text| text.contains("not a regular file"))
            );
            assert!(sidecar.is_dir());
        }
        assert_guidance_withdrawn(blue, fixture);
        let environment = archivefs_core::emulator_environment::retroarch::DiscoveryEnvironment::from_process_environment();
        assert!(
            archivefs_core::launch::preflight_retroarch_launch(
                &old_request,
                &archivefs_core::emulator_environment::HostReadOnlyFilesystem,
                &environment
            )
            .is_err()
        );
    }
    assert!(!root.join("retroarch/record/args.txt").exists());
}
