use super::*;

fn request(platform: &str) -> CheatRouteRequest {
    CheatRouteRequest {
        platform: Some(platform.to_string()),
        ..CheatRouteRequest::default()
    }
}

fn routed(decision: &CheatRouteDecision) -> &CheatRoute {
    decision
        .route()
        .unwrap_or_else(|| panic!("expected a routed decision, got {decision:?}"))
}

#[test]
fn ps3_with_rpcs3_selected_routes_to_rpcs3_native_support() {
    let mut req = request("PS3");
    req.selected = Some(CheatRouteTarget::standalone("rpcs3"));
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(route.target, CheatRouteTarget::standalone("rpcs3"));
    assert_eq!(route.basis, CheatRouteBasis::ExplicitSelection);
    assert_eq!(route.apply_support, CheatApplySupport::Supported);
    assert_eq!(decision.applicable_target(), Some(&route.target));
}

#[test]
fn ps3_never_falls_through_to_retroarch() {
    let mut req = request("PS3");
    req.retroarch_installed = Some(true);
    req.retroarch_cores = vec!["mednafen_psx_hw".into()];
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(route.target, CheatRouteTarget::standalone("rpcs3"));
    assert_eq!(route.basis, CheatRouteBasis::PlatformFallback);
    assert!(
        route
            .alternatives
            .iter()
            .all(|target| !matches!(target, CheatRouteTarget::RetroArch { .. }))
    );

    // Explicitly choosing RetroArch for PS3 is refused, not honoured.
    req.selected = Some(CheatRouteTarget::retroarch(None));
    let refused = route_cheat_install(&req);
    assert!(matches!(
        refused,
        CheatRouteDecision::Refused {
            refusal: CheatRouteRefusal::RetroArchNotACheatRouteForPlatform,
            ..
        }
    ));
    assert_eq!(refused.choices(), &[CheatRouteTarget::standalone("rpcs3")]);
}

#[test]
fn ps1_duckstation_selected_routes_to_duckstation() {
    let mut req = request("PSX");
    req.selected = Some(CheatRouteTarget::standalone("duckstation"));
    req.retroarch_installed = Some(true);
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(route.target, CheatRouteTarget::standalone("duckstation"));
    assert_eq!(route.apply_support, CheatApplySupport::Supported);
    assert!(
        route
            .alternatives
            .contains(&CheatRouteTarget::retroarch(None))
    );
}

#[test]
fn ps1_retroarch_selected_routes_to_retroarch_with_its_core() {
    let mut req = request("PSX");
    req.selected = Some(CheatRouteTarget::retroarch(Some("mednafen_psx_hw")));
    req.installed_standalone = vec!["duckstation".into()];
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(
        route.target,
        CheatRouteTarget::retroarch(Some("mednafen_psx_hw"))
    );
    assert_eq!(route.apply_support, CheatApplySupport::Supported);
    assert_eq!(decision.applicable_target(), Some(&route.target));
}

#[test]
fn psp_ppsspp_selected_routes_to_ppsspp() {
    let mut req = request("PSP");
    req.selected = Some(CheatRouteTarget::standalone("ppsspp"));
    let route = route_cheat_install(&req);
    assert_eq!(
        routed(&route).target,
        CheatRouteTarget::standalone("ppsspp")
    );
    assert_eq!(routed(&route).native_format, "PPSSPP CWCheat .ini");
    assert_eq!(routed(&route).apply_support, CheatApplySupport::Supported);
}

#[test]
fn gba_mgba_selected_routes_to_native_cheats() {
    let mut req = request("GBA");
    req.selected = Some(CheatRouteTarget::standalone("mgba"));
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(route.apply_support, CheatApplySupport::Supported);
    assert_eq!(route.native_format, "mGBA .cheats");
}

#[test]
fn arcade_mame_selected_routes_to_native_xml() {
    let mut req = request("Arcade");
    req.selected = Some(CheatRouteTarget::standalone("mame"));
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(route.apply_support, CheatApplySupport::Supported);
    assert_eq!(route.native_format, "MAME cheat XML");
}

#[test]
fn psp_retroarch_selected_routes_to_retroarch() {
    let mut req = request("PSP");
    req.selected = Some(CheatRouteTarget::retroarch(None));
    req.retroarch_cores = vec!["ppsspp".into()];
    let decision = route_cheat_install(&req);
    // The single installed PSP core is adopted; RetroArch alone is not.
    assert_eq!(
        routed(&decision).target,
        CheatRouteTarget::retroarch(Some("ppsspp"))
    );
}

#[test]
fn dreamcast_flycast_standalone_selected_routes_to_standalone() {
    let mut req = request("Dreamcast");
    req.selected = Some(CheatRouteTarget::standalone("flycast"));
    req.retroarch_cores = vec!["flycast".into()];
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(route.target, CheatRouteTarget::standalone("flycast"));
    assert_eq!(route.apply_support, CheatApplySupport::Supported);
}

#[test]
fn dreamcast_retroarch_flycast_core_selected_routes_to_retroarch() {
    let mut req = request("Dreamcast");
    req.selected = Some(CheatRouteTarget::retroarch(Some("flycast")));
    req.installed_standalone = vec!["flycast".into()];
    let route = route_cheat_install(&req);
    assert_eq!(
        routed(&route).target,
        CheatRouteTarget::retroarch(Some("flycast"))
    );
    assert!(
        routed(&route)
            .alternatives
            .contains(&CheatRouteTarget::standalone("flycast"))
    );
}

#[test]
fn selected_emulator_for_another_platform_is_refused_with_alternatives() {
    let mut req = request("PS2");
    req.selected = Some(CheatRouteTarget::standalone("duckstation"));
    let decision = route_cheat_install(&req);
    match &decision {
        CheatRouteDecision::Refused {
            refusal,
            alternatives,
            ..
        } => {
            assert_eq!(
                *refusal,
                CheatRouteRefusal::SelectedEmulatorDoesNotServePlatform
            );
            assert_eq!(alternatives, &vec![CheatRouteTarget::standalone("pcsx2")]);
        }
        other => panic!("expected refusal, got {other:?}"),
    }
    assert!(decision.applicable_target().is_none());
}

#[test]
fn selected_emulator_without_cheat_support_does_not_offer_apply() {
    let mut req = request("Nintendo 3DS");
    req.selected = Some(CheatRouteTarget::standalone("azahar"));
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(route.apply_support, CheatApplySupport::Unsupported);
    assert!(decision.applicable_target().is_none());
    assert!(
        decision
            .headline()
            .contains("not supported by your selected emulator")
    );
}

#[test]
fn three_ds_never_invents_a_retroarch_route() {
    let mut req = request("Nintendo 3DS");
    req.retroarch_installed = Some(true);
    let decision = route_cheat_install(&req);
    assert_eq!(
        routed(&decision).target,
        CheatRouteTarget::standalone("azahar")
    );
    assert!(routed(&decision).alternatives.is_empty());
}

#[test]
fn installed_native_standalone_plus_retroarch_prefers_native_apply_route() {
    let mut req = request("PSX");
    req.installed_standalone = vec!["duckstation".into()];
    req.retroarch_installed = Some(true);
    req.retroarch_cores = vec!["pcsx_rearmed".into()];
    let decision = route_cheat_install(&req);
    let route = routed(&decision);
    assert_eq!(route.target, CheatRouteTarget::standalone("duckstation"));
    assert_eq!(route.apply_support, CheatApplySupport::Supported);
    assert_eq!(decision.applicable_target(), Some(&route.target));
}

#[test]
fn two_configured_defaults_for_one_platform_are_ambiguous() {
    let mut req = request("PSX");
    req.configured_defaults = vec![
        CheatRouteTarget::standalone("duckstation"),
        CheatRouteTarget::retroarch(Some("pcsx_rearmed")),
        // Serves another platform: ignored.
        CheatRouteTarget::standalone("dolphin"),
    ];
    assert!(matches!(
        route_cheat_install(&req),
        CheatRouteDecision::Ambiguous { .. }
    ));
}

#[test]
fn configured_default_beats_platform_fallback() {
    let mut req = request("PSX");
    req.configured_defaults = vec![CheatRouteTarget::standalone("duckstation")];
    req.retroarch_installed = Some(true);
    let decision = route_cheat_install(&req);
    assert_eq!(routed(&decision).basis, CheatRouteBasis::ConfiguredDefault);
    assert_eq!(
        routed(&decision).target,
        CheatRouteTarget::standalone("duckstation")
    );
}

#[test]
fn explicit_selection_beats_configured_default() {
    let mut req = request("PSX");
    req.configured_defaults = vec![CheatRouteTarget::standalone("duckstation")];
    req.selected = Some(CheatRouteTarget::retroarch(Some("pcsx_rearmed")));
    let decision = route_cheat_install(&req);
    assert_eq!(routed(&decision).basis, CheatRouteBasis::ExplicitSelection);
}

#[test]
fn native_standalone_fallback_beats_retroarch_with_several_cores() {
    let mut req = request("PSX");
    req.retroarch_installed = Some(true);
    req.retroarch_cores = vec!["pcsx_rearmed".into(), "mednafen_psx_hw".into()];
    let decision = route_cheat_install(&req);
    assert_eq!(
        routed(&decision).target,
        CheatRouteTarget::standalone("duckstation")
    );
    assert_eq!(routed(&decision).basis, CheatRouteBasis::PlatformFallback);
}

#[test]
fn standalone_owned_platforms_keep_their_writable_emulator() {
    for (platform, adapter) in [
        ("PS2", "pcsx2"),
        ("GameCube", "dolphin"),
        ("Wii", "dolphin"),
        ("Xbox 360", "xenia"),
    ] {
        let mut req = request(platform);
        req.retroarch_installed = Some(true);
        let decision = route_cheat_install(&req);
        let route = routed(&decision);
        assert_eq!(
            route.target,
            CheatRouteTarget::standalone(adapter),
            "{platform}"
        );
        assert!(route.can_apply());
    }
}

#[test]
fn retroarch_only_platform_still_falls_back_to_retroarch() {
    let decision = route_cheat_install(&request("MegaDrive"));
    assert_eq!(routed(&decision).target, CheatRouteTarget::retroarch(None));
}

#[test]
fn unknown_or_missing_platform_has_no_route() {
    assert_eq!(
        route_cheat_install(&CheatRouteRequest::default()),
        CheatRouteDecision::NoRoute { platform_id: None }
    );
    assert!(matches!(
        route_cheat_install(&request("Unknown")),
        CheatRouteDecision::NoRoute { .. }
    ));
}

#[test]
fn routing_is_deterministic_for_identical_input() {
    let mut req = request("PSX");
    req.installed_standalone = vec!["duckstation".into()];
    req.retroarch_cores = vec!["pcsx_rearmed".into(), "mednafen_psx_hw".into()];
    let first = route_cheat_install(&req);
    for _ in 0..8 {
        assert_eq!(route_cheat_install(&req), first);
    }
    req.retroarch_cores.reverse();
    assert_eq!(route_cheat_install(&req), first);
}
