//! Job #22: an unusable RetroArch installation is named truthfully and never
//! left looking Ready. Each scenario changes the installation while Blue is
//! on the Launch page (outside Setup), checks Play, the bezel panel's reason
//! and Emulator Setup, then repairs it and sees Play return.
use super::*;
use crate::emulator_setup::RetroArchProfilesState;
use archivefs_core::emulator_environment::retroarch::installation_selection::EXECUTABLE_OVERRIDE_FILE;

#[test]
fn unusable_installations_are_named_and_play_returns_after_repair() {
    for scenario in [
        "truth-selected-missing",
        "truth-selected-wrapper",
        "truth-program-missing",
        "truth-window",
    ] {
        let profile = Profile::new();
        let second_dir = profile.fixture.root().join("second");
        fs::create_dir_all(&second_dir).unwrap();
        fs::copy(&profile.fake.executable, second_dir.join("retroarch")).unwrap();
        let before = profile.snapshot();
        profile.run(scenario, Some(&second_dir));
        assert!(
            !profile.fake.record_dir.join("args.txt").exists(),
            "{scenario}: nothing is launched"
        );
        assert_eq!(profile.snapshot(), before, "{scenario}");
    }
}

const SETUP_HEADLINE: &str = "The chosen RetroArch installation can't be used";
const GAME_FORMAT_BLAME: [&str; 3] = ["multi-disc", "archive", "another emulator"];

pub(super) fn run(blue: &mut Blue, fixture: &VerifiedGameFixture, root: &Path, scenario: &str) {
    let second = root.join("second/retroarch");
    let sidecar = archivefs_core::app_dirs::config_path(EXECUTABLE_OVERRIDE_FILE).unwrap();
    assert!(sidecar.starts_with(root));
    match scenario {
        "truth-selected-missing" => {
            let parked = root.join("second/parked");
            fs::rename(&second, &parked).unwrap();
            withdrawn(blue, fixture, "is no longer there");
            fs::rename(&parked, &second).unwrap();
            // Restoring the chosen program is noticed without opening Setup.
            blue.wait_until(|b| b.shown(PLAY), 30);
        }
        "truth-selected-wrapper" => {
            let wrapper = root.join("wrapper/retroarch");
            fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
            fs::write(
                &wrapper,
                "#!/usr/bin/env bash\nexec flatpak run \"org.libretro.RetroArch\" \"$@\"\n",
            )
            .unwrap();
            fs::set_permissions(
                &wrapper,
                <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
            )
            .unwrap();
            // Changed outside EmuWiz, as a hand edit or another tool would.
            fs::write(&sidecar, format!("{}\n", wrapper.display())).unwrap();
            withdrawn(blue, fixture, "launcher for the Flatpak RetroArch");
            fs::write(&sidecar, format!("{}\n", second.display())).unwrap();
            blue.wait_until(|b| b.shown(PLAY), 30);
        }
        "truth-program-missing" => {
            // Automatic detection: no saved choice and a single program, the
            // fixture's own; then that program goes.
            fs::remove_file(&sidecar).unwrap();
            fs::remove_file(&second).unwrap();
            refresh_via_setup(blue);
            blue.wait_until(|b| b.shown(PLAY) && !scanning(b), 30);
            let program = root.join("retroarch/bin/retroarch");
            assert_eq!(blue.discovered_executables(), vec![program.display().to_string()]);
            let parked = root.join("retroarch/parked");
            fs::rename(&program, &parked).unwrap();
            blue.wait_until(|b| !b.shown(PLAY) && !scanning(b), 30);
            // Settled, not just a scanning frame: Play stays withheld.
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(100));
                blue.paint();
                assert!(!blue.shown(PLAY), "{:?}", blue.last);
                assert!(!ready_badge(blue), "{:?}", blue.last);
            }
            let reason = bezel_request(blue, fixture).expect_err("bezel Apply is refused");
            for blame in GAME_FORMAT_BLAME {
                assert!(!reason.contains(blame), "{reason}");
            }
            // Reinstalling is picked up by Setup's rediscovery, which now
            // also refreshes RetroArch readiness.
            fs::rename(&parked, &program).unwrap();
            refresh_via_setup(blue);
            blue.wait_until(|b| b.shown(PLAY), 30);
        }
        "truth-window" => {
            // Before revalidation's next tick the cached scan still lists the
            // program: the Launch page itself must withdraw Ready and Play.
            blue.app
                .native_workflows
                .as_mut()
                .unwrap()
                .app
                .emulator_readiness
                .retroarch_revalidated_at = Some(Instant::now());
            let parked = root.join("second/parked");
            fs::rename(&second, &parked).unwrap();
            crate::retroarch_installation_setup::forget_selection_issue();
            blue.paint();
            assert!(
                matches!(
                    blue.app
                        .native_workflows
                        .as_ref()
                        .unwrap()
                        .app
                        .emulator_readiness
                        .retroarch_profiles,
                    RetroArchProfilesState::Ready(_)
                ),
                "still the cached scan"
            );
            assert!(!blue.shown(PLAY), "{:?}", blue.last);
            assert!(blue.shown("Needs attention"), "{:?}", blue.last);
            assert!(blue.shown("is no longer there"), "{:?}", blue.last);
            assert!(!ready_badge(blue), "{:?}", blue.last);
            fs::rename(&parked, &second).unwrap();
            blue.wait_until(|b| b.shown(PLAY), 30);
        }
        other => panic!("unknown selection-truth scenario {other}"),
    }
}

/// Play is withdrawn and every surface names `reason`, never a game-format
/// guess or a green status.
fn withdrawn(blue: &mut Blue, fixture: &VerifiedGameFixture, reason: &str) {
    blue.wait_until(|b| !b.shown(PLAY) && !scanning(b), 30);
    assert!(!ready_badge(blue), "{:?}", blue.last);
    let bezel = bezel_request(blue, fixture).expect_err("bezel Apply is refused");
    assert!(bezel.contains(reason), "{bezel}");
    for blame in GAME_FORMAT_BLAME {
        assert!(!bezel.contains(blame), "{bezel}");
    }
    let setup = setup_card(blue);
    assert!(
        setup.iter().any(|t| t.contains(SETUP_HEADLINE)),
        "{setup:?}"
    );
    assert!(setup.iter().any(|t| t.contains(reason)), "{setup:?}");
    assert!(!setup.iter().any(|t| t == "Ready"), "{setup:?}");
}

/// A Ready status badge on the Launch page (the footer's "Ready · ..." is
/// the app status, not readiness).
fn ready_badge(blue: &Blue) -> bool {
    blue.last
        .iter()
        .any(|text| text == "Ready" || text.contains("Ready to play"))
}

fn scanning(blue: &Blue) -> bool {
    matches!(
        blue.app
            .native_workflows
            .as_ref()
            .unwrap()
            .app
            .emulator_readiness
            .retroarch_profiles,
        RetroArchProfilesState::Scanning { .. }
    )
}

fn bezel_request(
    blue: &Blue,
    fixture: &VerifiedGameFixture,
) -> Result<archivefs_core::launch::RetroArchLaunchRequest, String> {
    blue.app
        .native_workflows
        .as_ref()
        .unwrap()
        .retroarch_bezel_request(blue.game, &fixture.rom_path)
}

/// Emulator Setup's RetroArch card, painted on its own.
fn setup_card(blue: &mut Blue) -> Vec<String> {
    let context = blue.context.clone();
    let inner = &mut blue.app.native_workflows.as_mut().unwrap().app;
    let output = context.run(Default::default(), |context| {
        eframe::egui::CentralPanel::default().show(context, |ui| {
            inner.show_retroarch_core_folder_card(ui, context, false);
        });
    });
    texts(&output)
}

/// "Check installations again" in Setup: its completion must start a fresh
/// RetroArch readiness scan.
fn refresh_via_setup(blue: &mut Blue) {
    let context = blue.context.clone();
    blue.app
        .native_workflows
        .as_mut()
        .unwrap()
        .app
        .emulator_readiness
        .retroarch_installation_picker
        .check_again(context.clone());
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
            "Setup rediscovery did not refresh RetroArch readiness"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
