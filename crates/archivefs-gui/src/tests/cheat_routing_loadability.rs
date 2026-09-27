use super::*;
use archivefs_core::patch_manager::{
    CheatLoadabilityIssue, CheatLoadabilityState, CheatRouteDecision, CheatRouteRequest,
    CheatRouteTarget, EmulatorProcessObservation, route_cheat_install,
};
use sha2::{Digest, Sha256};

fn app_with_game(path: &str, platform: &str) -> ArchiveFsApp {
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut game = record(path, MountState::Pending);
        game.identity.platform = Some(platform.to_string());
        data.records.push(game);
    }
    app
}

fn routed_target(app: &ArchiveFsApp) -> Option<CheatRouteTarget> {
    app.cheat_workflow
        .as_ref()
        .unwrap()
        .routing
        .decision
        .route()
        .map(|route| route.target.clone())
}

#[test]
fn ps3_workspace_routes_to_rpcs3_and_offers_no_retroarch_install() {
    let mut app = app_with_game("/roms/ps3.iso", "PS3");
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/ps3.iso")));
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().adapter,
        CheatEmulatorAdapter::Unsupported
    );
    assert_eq!(
        routed_target(&app),
        Some(CheatRouteTarget::standalone("rpcs3"))
    );
}

#[test]
fn ps1_workspace_honours_an_explicit_duckstation_choice() {
    let mut app = app_with_game("/roms/ff7.chd", "PSX");
    app.cheat_emulator_selections.insert(
        PathBuf::from("/roms/ff7.chd"),
        CheatRouteTarget::standalone("duckstation"),
    );
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/ff7.chd")));
    let workflow = app.cheat_workflow.as_ref().unwrap();
    // DuckStation is routed but EmuWiz cannot install for it yet, so no
    // RetroArch workflow is substituted.
    assert_eq!(workflow.adapter, CheatEmulatorAdapter::Unsupported);
    assert_eq!(
        routed_target(&app),
        Some(CheatRouteTarget::standalone("duckstation"))
    );
    assert_eq!(
        workflow.routing.selected_emulator,
        Some(CheatRouteTarget::standalone("duckstation"))
    );
}

#[test]
fn ps1_workspace_honours_an_explicit_retroarch_choice() {
    let mut app = app_with_game("/roms/ff7.chd", "PSX");
    app.cheat_emulator_selections.insert(
        PathBuf::from("/roms/ff7.chd"),
        CheatRouteTarget::retroarch(Some("mednafen_psx_hw")),
    );
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/ff7.chd")));
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().adapter,
        CheatEmulatorAdapter::RetroArch
    );
    assert_eq!(
        routed_target(&app),
        Some(CheatRouteTarget::retroarch(Some("mednafen_psx_hw")))
    );
}

#[test]
fn selecting_an_emulator_for_another_platform_is_refused_not_switched() {
    let mut app = app_with_game("/roms/gow.iso", "PS2");
    app.cheat_emulator_selections.insert(
        PathBuf::from("/roms/gow.iso"),
        CheatRouteTarget::standalone("duckstation"),
    );
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/gow.iso")));
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(workflow.adapter, CheatEmulatorAdapter::Unsupported);
    assert!(matches!(
        workflow.routing.decision,
        CheatRouteDecision::Refused { .. }
    ));
    assert_eq!(
        workflow.routing.decision.choices(),
        &[CheatRouteTarget::standalone("pcsx2")]
    );
}

#[test]
fn route_panel_names_the_selected_emulator_and_its_limits() {
    let mut app = app_with_game("/roms/ff7.chd", "PSX");
    app.cheat_emulator_selections.insert(
        PathBuf::from("/roms/ff7.chd"),
        CheatRouteTarget::standalone("duckstation"),
    );
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/ff7.chd")));
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheat_route_panel(ui, app.cheat_workflow.as_ref().unwrap());
        });
    });
    for expected in [
        "DuckStation",
        "Apply not supported yet",
        "Clear my emulator choice",
    ] {
        assert!(rendered_text_contains(&output, expected), "{expected}");
    }
}

fn dolphin_install_fixture(
    temp: &Path,
    activation: CheatActivationReadiness,
) -> (ArchiveFsApp, SharedApplyResult) {
    let mut app = dolphin_workflow_with_matched_identity(temp, "GAFE01");
    let game_settings = temp.join("GameSettings");
    std::fs::create_dir_all(&game_settings).unwrap();
    let bytes = b"[Gecko]\n$Infinite Health\n04123456 00000063\n";
    std::fs::write(game_settings.join("GAFE01.ini"), bytes).unwrap();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.routing.decision = route_cheat_install(&CheatRouteRequest {
        platform: Some("GameCube".to_string()),
        ..Default::default()
    });
    workflow.dolphin_activation = activation;
    let mut result = successful_shared_apply_result();
    let entry = &mut result.journal.entries[0];
    entry.plan_entry.destination_root = SharedTransactionPath::from_path(&game_settings);
    entry.final_destination_digest = Some(
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    );
    (app, result)
}

fn not_running(_: &CheatRouteTarget) -> EmulatorProcessObservation {
    EmulatorProcessObservation::NotRunning
}

#[test]
fn dolphin_install_with_cheats_enabled_is_ready_and_leaves_the_journal_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let (app, result) = dolphin_install_fixture(temp.path(), CheatActivationReadiness::Enabled);
    let journal_before = result.journal.clone();
    let journal_path_before = result.journal_path.clone();
    let report = cheat_install_loadability(
        app.cheat_workflow.as_ref().unwrap(),
        &app.emulator_readiness,
        &result,
        not_running,
    )
    .unwrap();
    assert!(report.file_installed);
    assert_eq!(
        report.state,
        CheatLoadabilityState::LoadableVerifiedByConfig
    );
    assert_eq!(report.headline(), "Installed and ready.");
    // Loadability is read-only: the transaction/history record is unchanged.
    assert_eq!(result.journal, journal_before);
    assert_eq!(result.journal_path, journal_path_before);
}

#[test]
fn dolphin_install_with_cheats_disabled_is_not_reported_as_ready() {
    let temp = tempfile::tempdir().unwrap();
    let (app, result) = dolphin_install_fixture(temp.path(), CheatActivationReadiness::Disabled);
    let report = cheat_install_loadability(
        app.cheat_workflow.as_ref().unwrap(),
        &app.emulator_readiness,
        &result,
        not_running,
    )
    .unwrap();
    assert!(report.file_installed);
    assert_eq!(report.state, CheatLoadabilityState::EmulatorCheatsDisabled);
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_cheat_loadability(ui, &report);
        });
    });
    for expected in [
        "Installed file:",
        "Yes, bytes verified",
        "Selected emulator will load it:",
        "Installed, but cheats are disabled in the selected emulator.",
    ] {
        assert!(rendered_text_contains(&output, expected), "{expected}");
    }
}

#[test]
fn dolphin_install_while_emulator_runs_requires_restart() {
    let temp = tempfile::tempdir().unwrap();
    let (app, result) = dolphin_install_fixture(temp.path(), CheatActivationReadiness::Enabled);
    let report = cheat_install_loadability(
        app.cheat_workflow.as_ref().unwrap(),
        &app.emulator_readiness,
        &result,
        |_| EmulatorProcessObservation::Running,
    )
    .unwrap();
    assert_eq!(report.state, CheatLoadabilityState::RestartRequired);
}

#[test]
fn install_outside_the_profile_game_settings_is_path_not_observed() {
    let temp = tempfile::tempdir().unwrap();
    let (app, mut result) = dolphin_install_fixture(temp.path(), CheatActivationReadiness::Enabled);
    let elsewhere = temp.path().join("Elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::copy(
        temp.path().join("GameSettings/GAFE01.ini"),
        elsewhere.join("GAFE01.ini"),
    )
    .unwrap();
    result.journal.entries[0].plan_entry.destination_root =
        SharedTransactionPath::from_path(&elsewhere);
    let report = cheat_install_loadability(
        app.cheat_workflow.as_ref().unwrap(),
        &app.emulator_readiness,
        &result,
        not_running,
    )
    .unwrap();
    assert!(report.file_installed);
    assert_eq!(report.state, CheatLoadabilityState::PathNotObserved);
    assert!(matches!(
        report.issues[0],
        CheatLoadabilityIssue::DestinationOutsideEmulatorDirectory { .. }
    ));
}

#[test]
fn retroarch_install_without_a_known_core_is_ambiguous() {
    let temp = tempfile::tempdir().unwrap();
    let (mut app, result) = dolphin_install_fixture(temp.path(), CheatActivationReadiness::Enabled);
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.adapter = CheatEmulatorAdapter::RetroArch;
    workflow.routing.decision = route_cheat_install(&CheatRouteRequest {
        platform: Some("MegaDrive".to_string()),
        ..Default::default()
    });
    let report = cheat_install_loadability(
        app.cheat_workflow.as_ref().unwrap(),
        &app.emulator_readiness,
        &result,
        not_running,
    )
    .unwrap();
    assert_eq!(report.retroarch_core, None);
    assert_eq!(report.state, CheatLoadabilityState::AmbiguousProfile);
    assert!(
        report
            .issues
            .contains(&CheatLoadabilityIssue::RetroArchCoreUnknown)
    );
}

#[test]
fn beginner_result_separates_file_install_from_loadability() {
    let temp = tempfile::tempdir().unwrap();
    let (app, result) = dolphin_install_fixture(temp.path(), CheatActivationReadiness::Enabled);
    let report = cheat_install_loadability(
        app.cheat_workflow.as_ref().unwrap(),
        &app.emulator_readiness,
        &result,
        not_running,
    )
    .unwrap();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_beginner_install_result(ui, &result, Some(&report));
        });
    });
    for expected in [
        "Files installed",
        "Selected emulator:",
        "Dolphin",
        "Yes, verified from its settings",
        "Undo installation",
    ] {
        assert!(rendered_text_contains(&output, expected), "{expected}");
    }
    assert!(!rendered_text_contains(&output, "executed"));
}
