use super::*;
use crate::emulator_setup::{DolphinProfilesState, Pcsx2ProfilesState};
use archivefs_core::game_identity::inspect_game_identity;
use archivefs_core::patch_manager::{DolphinProfileDiscovery, Pcsx2ProfileDiscovery};

fn report(path: &Path, platform: IdentityPlatform) -> GameIdentityReport {
    let mut report = inspect_game_identity(path, None);
    report.archive_path = path.to_path_buf();
    report.platform = platform;
    report
}

fn fixture(
    context: &egui::Context,
    platform: Option<IdentityPlatform>,
) -> (NativeWorkflows, crate::gui_v2::library::Game) {
    let mut workflows = NativeWorkflows::new(context.clone());
    workflows.app = crate::tests::app_with_cheats_mods_context();
    // Never let a test scan the real machine for emulators.
    workflows.app.emulator_readiness.pcsx2_profiles =
        Pcsx2ProfilesState::Ready(Pcsx2ProfileDiscovery {
            profiles: vec![],
            warnings: vec![],
            complete: true,
        });
    workflows.app.emulator_readiness.dolphin_profiles =
        DolphinProfilesState::Ready(DolphinProfileDiscovery {
            profiles: vec![],
            warnings: vec![],
            complete: true,
        });
    let path = workflows
        .app
        .cheat_workflow
        .as_ref()
        .unwrap()
        .archive_path
        .clone();
    let mut row = crate::gui_v2::tests::archive(7, "Synthetic", None);
    row.absolute_path = path.clone();
    row.identity_report = platform.map(|platform| report(&path, platform));
    (workflows, crate::gui_v2::library::Game::from_archive(row))
}

fn run(
    context: &egui::Context,
    workflows: &mut NativeWorkflows,
    game: Option<&crate::gui_v2::library::Game>,
    events: Vec<egui::Event>,
) -> (String, Option<Route>) {
    let mut route = None;
    let output = context.run(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                route = workflows.show_texture_packs(ui, game, &mut Activity::default());
            });
        },
    );
    let text = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, route)
}

#[test]
fn engines_follow_structural_platform_for_the_selected_file_only() {
    let path = Path::new("/fixture/a.iso");
    let other = Path::new("/fixture/b.iso");
    for (platform, engine) in [
        (IdentityPlatform::GameCube, Engine::Dolphin),
        (IdentityPlatform::Wii, Engine::Dolphin),
        (IdentityPlatform::PlayStation2, Engine::Pcsx2),
        (IdentityPlatform::Psp, Engine::Ppsspp),
    ] {
        let r = report(path, platform);
        assert_eq!(engine_for(Some(&r), path), Ok(engine));
        assert_eq!(engine_for(Some(&r), other), Err(Unavailable::NoIdentity));
    }
    assert_eq!(engine_for(None, path), Err(Unavailable::NoIdentity));
    assert!(matches!(
        engine_for(Some(&report(path, IdentityPlatform::Snes)), path),
        Err(Unavailable::Platform(_))
    ));
    assert_eq!(
        engine_for(Some(&report(path, IdentityPlatform::Other)), path),
        Err(Unavailable::UnknownPlatform)
    );
}

#[test]
fn unsupported_platform_is_shown_unavailable_and_binds_nothing() {
    let context = egui::Context::default();
    let (mut workflows, game) = fixture(&context, Some(IdentityPlatform::Snes));
    workflows.app.cheat_workflow = None;
    let (text, route) = run(&context, &mut workflows, Some(&game), vec![]);
    assert!(text.contains("Texture packs are unavailable for"), "{text}");
    assert!(route.is_none());
    assert!(workflows.app.cheat_workflow.is_none());
    assert!(!workflows.texture_busy());
}

#[test]
fn missing_or_mismatched_identity_asks_for_review_instead_of_guessing() {
    let context = egui::Context::default();
    let (mut workflows, mut game) = fixture(&context, None);
    workflows.app.cheat_workflow = None;
    let (text, _) = run(&context, &mut workflows, Some(&game), vec![]);
    assert!(text.contains("Review identity"), "{text}");
    assert!(workflows.app.cheat_workflow.is_none());
    // A report for some other file is not evidence for this one.
    game.archive.identity_report = Some(report(
        Path::new("/fixture/other.iso"),
        IdentityPlatform::Psp,
    ));
    let (text, route) = run(&context, &mut workflows, Some(&game), vec![]);
    assert!(text.contains("Review identity"), "{text}");
    assert!(route.is_none());
    assert!(workflows.app.cheat_workflow.is_none());
}

#[test]
fn ps2_and_gamecube_without_a_profile_offer_no_install_controls() {
    let context = egui::Context::default();
    for (platform, expected) in [
        (IdentityPlatform::PlayStation2, "No PCSX2 profile was found"),
        (IdentityPlatform::GameCube, "No Dolphin profile was found"),
        (IdentityPlatform::Wii, "No Dolphin profile was found"),
    ] {
        let (mut workflows, game) = fixture(&context, Some(platform));
        let (text, _) = run(&context, &mut workflows, Some(&game), vec![]);
        assert!(text.contains(expected), "{text}");
        assert!(!text.contains("Choose pack"));
        assert!(!workflows.texture_busy());
    }
}

#[test]
fn another_games_retained_workflow_blocks_this_selection() {
    let context = egui::Context::default();
    let (mut workflows, mut game) = fixture(&context, Some(IdentityPlatform::PlayStation2));
    // The retained workflow is busy, so a different game cannot rebind it.
    workflows.app.cheat_workflow.as_mut().unwrap().rpcs3 =
        crate::cheats_mods::rpcs3::Rpcs3PageState::pending_activation_fixture();
    let other = Path::new("/fixture/another.iso").to_path_buf();
    game.archive.absolute_path = other.clone();
    game.archive.identity_report = Some(report(&other, IdentityPlatform::PlayStation2));
    let (text, _) = run(&context, &mut workflows, Some(&game), vec![]);
    assert!(
        text.contains("Previous game's workflow is still retained"),
        "{text}"
    );
    assert!(!text.contains("Choose pack"));
}

#[test]
fn no_game_is_reachable_and_activatable_from_the_keyboard_or_controller() {
    // A controller reaches egui as Tab/Enter focus events, as in pad_shell.
    let context = egui::Context::default();
    let (mut workflows, _) = fixture(&context, None);
    let key = |key, pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = run(&context, &mut workflows, None, vec![]);
    let _ = run(
        &context,
        &mut workflows,
        None,
        vec![key(egui::Key::Tab, true), key(egui::Key::Tab, false)],
    );
    let _ = run(&context, &mut workflows, None, vec![]);
    let (_, route) = run(
        &context,
        &mut workflows,
        None,
        vec![key(egui::Key::Enter, true), key(egui::Key::Enter, false)],
    );
    assert_eq!(route, Some(Route::Section(Section::Games)));
}

#[test]
fn one_app_level_pump_collects_results_on_any_page_exactly_once() {
    let context = egui::Context::default();
    let (mut workflows, _) = fixture(&context, None);
    let pack = std::path::PathBuf::from("/disposable/pack");
    workflows
        .app
        .ppsspp_texture_mod
        .queue_picked_for_test(Some(pack.clone()));
    assert!(workflows.texture_busy());
    // No texture page or tab is rendered: the per-frame poll alone collects it.
    workflows.poll(&context, &mut Activity::default());
    assert!(!workflows.texture_busy(), "the result was consumed");
    assert_eq!(
        workflows.app.ppsspp_texture_mod.source_root(),
        Some(pack.as_path())
    );
    // A second pump (the Texture packs tab rendering) has nothing to consume
    // and does not disturb the collected state.
    workflows.poll_texture_jobs(&context);
    let _ = run(&context, &mut workflows, None, vec![]);
    assert_eq!(
        workflows.app.ppsspp_texture_mod.source_root(),
        Some(pack.as_path())
    );
}

#[test]
fn a_result_waits_in_its_worker_until_polled_and_is_not_lost_by_page_switches() {
    let context = egui::Context::default();
    let (mut workflows, game) = fixture(&context, Some(IdentityPlatform::Wii));
    workflows
        .app
        .ppsspp_texture_mod
        .queue_picked_for_test(Some("/disposable/pack".into()));
    // Rendering other pages / the tab with a different engine does not consume it.
    let _ = run(&context, &mut workflows, Some(&game), vec![]);
    assert!(workflows.texture_busy());
    workflows.poll_texture_jobs(&context);
    assert!(!workflows.texture_busy());
    assert!(workflows.app.ppsspp_texture_mod.source_root().is_some());
}
