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
            let _ = show_cheat_route_panel(ui, app.cheat_workflow.as_mut().unwrap());
        });
    });
    for expected in [
        "DuckStation",
        "Native adapter available",
        "Clear my emulator choice",
    ] {
        assert!(rendered_text_contains(&output, expected), "{expected}");
    }
}

#[test]
fn amiga_whdload_route_shows_safe_trainer_editor_gate() {
    let mut app = app_with_game("/roms/game.adf", "Amiga");
    app.cheat_emulator_selections.insert(
        PathBuf::from("/roms/game.adf"),
        CheatRouteTarget::standalone("amiga_whdload"),
    );
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/game.adf")));
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheat_route_panel(ui, app.cheat_workflow.as_mut().unwrap());
        });
    });
    for expected in [
        "WHDLoad Trainer / Custom Options",
        "These options come from the installed WHDLoad slave.",
        "Exact installed slave/config binding required",
        "Controls and Apply remain blocked.",
    ] {
        assert!(rendered_text_contains(&output, expected), "{expected}");
    }
}

/// A discovery as the backend returns it for a slave declaring
/// `C1:X:Infinite lives:0;C1:X:Infinite energy:1;C1:X:In-game keys:3;`, plus an
/// opaque and a malformed declaration.
fn shared_slot_discovery(
    state: archivefs_core::patch_manager::WhdloadTrainerDiscoveryState,
) -> archivefs_core::patch_manager::WhdloadTrainerDiscovery {
    use archivefs_core::patch_manager::{
        TrainerOption, TrainerOptionKind, TrainerOptionReadiness, TrainerOptionSource,
        WhdloadTrainerDiscovery, WhdloadTrainerDiscoveryState, WhdloadTrainerIdentity,
    };
    let slave = PathBuf::from("/whdload/Game/Game.slave");
    let option = |name: &str, key: &str, kind, readiness| TrainerOption {
        name: name.into(),
        description: format!("{name} description"),
        source: TrainerOptionSource {
            slave_path: slave.clone(),
            slave_sha256: "a".repeat(64),
            verified_game_identity: "amiga:fixture".into(),
            package_version: None,
        },
        custom_slot: key.split(':').next().unwrap().into(),
        selection_key: key.into(),
        kind,
        allowed_values: Vec::new(),
        current_value: None,
        current_raw_value: None,
        default_value: None,
        bit_range: None,
        provenance: "fixture".into(),
        readiness,
    };
    let ready = state == WhdloadTrainerDiscoveryState::Ready;
    WhdloadTrainerDiscovery {
        state,
        detail: "fixture detail".into(),
        slave_path: Some(slave.clone()),
        slave_sha256: Some("a".repeat(64)),
        slave_runtime_version: Some(17),
        declarations: Vec::new(),
        options: vec![
            option(
                "Infinite lives",
                "CUSTOM1:X:0",
                TrainerOptionKind::Boolean,
                TrainerOptionReadiness::Ready,
            ),
            option(
                "Infinite energy",
                "CUSTOM1:X:1",
                TrainerOptionKind::Boolean,
                TrainerOptionReadiness::Ready,
            ),
            option(
                "In-game keys",
                "CUSTOM1:X:3",
                TrainerOptionKind::Boolean,
                TrainerOptionReadiness::Ready,
            ),
            option(
                "Raw field",
                "CUSTOM5",
                TrainerOptionKind::Opaque,
                TrainerOptionReadiness::Unsupported,
            ),
            option(
                "Broken bits",
                "CUSTOM4",
                TrainerOptionKind::Boolean,
                TrainerOptionReadiness::InvalidValue,
            ),
        ],
        identity: ready.then(|| WhdloadTrainerIdentity {
            slave_path: slave,
            expected_slave_sha256: "a".repeat(64),
            verified_game_identity: "amiga:fixture".into(),
            package_version: None,
        }),
        candidates: Vec::new(),
    }
}

fn whdload_app() -> ArchiveFsApp {
    let mut app = app_with_game("/roms/game.adf", "Amiga");
    app.cheat_emulator_selections.insert(
        PathBuf::from("/roms/game.adf"),
        CheatRouteTarget::standalone("amiga_whdload"),
    );
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/game.adf")));
    app
}

/// Renders the route panel; `click` presses the n-th "Enabled" checkbox.
fn render_route(app: &mut ArchiveFsApp, click: Option<usize>) -> egui::FullOutput {
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 4000.0));
    let mut run = |events: Vec<egui::Event>, app: &mut ArchiveFsApp| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let _ = show_cheat_route_panel(ui, app.cheat_workflow.as_mut().unwrap());
                    });
                });
            },
        )
    };
    let output = run(Vec::new(), app);
    let Some(index) = click else {
        return output;
    };
    let point = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Enabled" => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .nth(index)
        .expect("an Enabled checkbox");
    for pressed in [true, false] {
        run(
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            app,
        );
    }
    run(Vec::new(), app)
}

#[test]
fn shared_slot_trainer_bits_toggle_independently_by_selection_key() {
    use archivefs_core::patch_manager::{TrainerOptionValue, WhdloadTrainerDiscoveryState};
    let mut app = whdload_app();
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .whdload_editor
        .discovery = Some(shared_slot_discovery(WhdloadTrainerDiscoveryState::Ready));
    // Enable lives and energy, then disable lives again.
    render_route(&mut app, Some(0));
    render_route(&mut app, Some(1));
    let output = render_route(&mut app, Some(0));
    let selections = &app
        .cheat_workflow
        .as_ref()
        .unwrap()
        .whdload_editor
        .selections;
    let value = |key: &str| {
        selections
            .iter()
            .find(|selection| selection.custom_slot == key)
            .map(|selection| selection.value.clone())
    };
    // Keyed by selection_key, never by the bare shared slot.
    assert!(
        selections
            .iter()
            .all(|selection| selection.custom_slot != "CUSTOM1")
    );
    assert_eq!(
        value("CUSTOM1:X:0"),
        Some(None),
        "lives removed: only its bit"
    );
    assert_eq!(
        value("CUSTOM1:X:1"),
        Some(Some(TrainerOptionValue::Boolean(true))),
        "energy kept"
    );
    assert_eq!(value("CUSTOM1:X:3"), None, "keys untouched");
    // Opaque and malformed stay visible with their reasons and never record.
    for expected in [
        "Raw field",
        "This custom field uses unsupported syntax, so it can be shown but will not apply.",
        "Broken bits",
        "This declaration is malformed, so EmuWiz cannot tell which bits it owns. It will not apply.",
    ] {
        assert!(rendered_text_contains(&output, expected), "{expected}");
    }
    assert!(selections.iter().all(|selection| {
        selection.custom_slot != "CUSTOM5" && selection.custom_slot != "CUSTOM4"
    }));
    // Never claimed active at launch; Preview waits for an option file.
    assert!(rendered_text_contains(&output, archivefs_gui_launch_note()));
    assert!(rendered_text_contains(
        &output,
        crate::WHDLOAD_NO_OPTION_FILE
    ));
}

fn archivefs_gui_launch_note() -> &'static str {
    crate::WHDLOAD_LAUNCH_NOTE
}

#[test]
fn trainer_preview_follows_the_backend_gates() {
    use crate::{WHDLOAD_NO_OPTION_FILE, WHDLOAD_NOTHING_CHANGEABLE, whdload_preview_blocker};
    use archivefs_core::patch_manager::{TrainerOptionReadiness, WhdloadTrainerDiscoveryState};
    let config = PathBuf::from("/data/whdload/Game.options");
    let ready = shared_slot_discovery(WhdloadTrainerDiscoveryState::Ready);
    assert_eq!(whdload_preview_blocker(&ready, Some(&config)), None);
    assert_eq!(
        whdload_preview_blocker(&ready, None),
        Some(WHDLOAD_NO_OPTION_FILE)
    );
    let mut nothing = ready.clone();
    for option in &mut nothing.options {
        option.readiness = TrainerOptionReadiness::Unsupported;
    }
    assert_eq!(
        whdload_preview_blocker(&nothing, Some(&config)),
        Some(WHDLOAD_NOTHING_CHANGEABLE)
    );
    for state in [
        WhdloadTrainerDiscoveryState::IdentityUnverified,
        WhdloadTrainerDiscoveryState::SlaveInsideHdf,
        WhdloadTrainerDiscoveryState::AmbiguousSlaves,
    ] {
        let blocked = shared_slot_discovery(state);
        assert_eq!(
            whdload_preview_blocker(&blocked, Some(&config)),
            Some(state.plain_label()),
            "{state:?}"
        );
        // Every declaration is still listed, read-only.
        let mut app = whdload_app();
        app.cheat_workflow
            .as_mut()
            .unwrap()
            .whdload_editor
            .discovery = Some(blocked);
        let output = render_route(&mut app, None);
        assert!(rendered_text_contains(&output, state.plain_label()));
        assert!(rendered_text_contains(&output, "In-game keys"));
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

#[test]
fn whdload_runtime_status_distinguishes_support_from_saved_choices() {
    use archivefs_core::patch_manager::AmigaEmulatorKind;
    for emulator in [AmigaEmulatorKind::FsUae, AmigaEmulatorKind::Amiberry] {
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                crate::cheats_mods::whdload_trainer_flow::show_launch_status(ui, Some(emulator));
            });
        });
        for expected in [
            "trainer launch unsupported",
            "EmuWiz saved CUSTOM options passed into launch: none.",
            "Enable trainers at launch",
        ] {
            assert!(rendered_text_contains(&output, expected), "{expected}");
        }
    }
}
