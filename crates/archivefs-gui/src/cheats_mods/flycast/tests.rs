use super::*;
use archivefs_core::game_identity::*;
use archivefs_core::launch::{
    CandidatePreference, FirmwareReadiness, LaunchCandidate, LaunchContentRef, LaunchPlanSummary,
};

fn identity(game: &Path) -> GameIdentityReport {
    GameIdentityReport {
        archive_path: game.into(),
        platform: IdentityPlatform::Dreamcast,
        format: IdentityImageFormat::Iso,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::DreamcastProductCode,
            status: IdentityStatus::Verified,
            value: Some("T-8109N".into()),
            confidence: IdentityConfidence::ExactBytes,
            provenance: IdentityProvenance {
                archive_path: game.into(),
                member_path: None,
                member_index: None,
                method: "synthetic".into(),
            },
            diagnostic: String::new(),
        }],
        warnings: vec![],
        bytes_read: 0,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    }
}
fn native(shift: u32) -> Vec<u8> {
    format!("cheats=1\ncheat0_desc=Health\ncheat0_cheat_type=1\ncheat0_address=256\ncheat0_memory_search_size={shift}\ncheat0_value=9\ncheat0_enable=true\n").into_bytes()
}
fn candidate(id: &str, preference: CandidatePreference) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "flycast",
            profile_id: id.into(),
            profile_path: Some(PathBuf::from(format!("/disposable/{id}"))),
        },
        content: LaunchContentRef {
            kind: None,
            container: None,
            resolved_path: Some("/disposable/game.gdi".into()),
            requires_mount: false,
            provenance: "synthetic".into(),
        },
        firmware: FirmwareReadiness::NotRequired,
        blockers: vec![],
        warnings: vec![],
        readiness: LaunchReadiness::Ready,
        preference,
    }
}
fn plan(candidates: Vec<LaunchCandidate>) -> LaunchPlan {
    LaunchPlan {
        platform_id: Some("Dreamcast".into()),
        game_key: Some("T-8109N".into()),
        summary: LaunchPlanSummary {
            candidates: candidates.len(),
            ready: candidates.len(),
            ready_with_warnings: 0,
            blocked: 0,
        },
        candidates,
        media_topology: None,
    }
}
#[test]
fn launch_sole_and_remembered_choices_match_blue_without_reselection() {
    let game = Path::new("/disposable/game.gdi");
    let sole = plan(vec![candidate("one", CandidatePreference::SoleEligible)]);
    let before = sole.clone();
    assert_eq!(selected_launch(&sole, game).unwrap().profile_id, "one");
    assert_eq!(sole, before);
    let remembered = plan(vec![
        candidate("one", CandidatePreference::Undetermined),
        candidate("two", CandidatePreference::Remembered),
    ]);
    assert_eq!(
        selected_launch(&remembered, game).unwrap().profile_id,
        "two"
    );
    assert!(selected_launch(&remembered, Path::new("/different.gdi")).is_err());
}
#[test]
fn ambiguous_blocked_other_emulator_and_mounted_choices_refuse() {
    let game = Path::new("/disposable/game.gdi");
    assert!(
        selected_launch(
            &plan(vec![
                candidate("one", CandidatePreference::Undetermined),
                candidate("two", CandidatePreference::Undetermined)
            ]),
            game
        )
        .is_err()
    );
    let mut c = candidate("one", CandidatePreference::Remembered);
    c.readiness = LaunchReadiness::Blocked;
    assert!(selected_launch(&plan(vec![c.clone()]), game).is_err());
    c.readiness = LaunchReadiness::Ready;
    c.target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "other".into(),
        profile_path: None,
    };
    assert!(selected_launch(&plan(vec![c.clone()]), game).is_err());
    c.target = candidate("one", CandidatePreference::Remembered).target;
    c.content.requires_mount = true;
    assert!(selected_launch(&plan(vec![c]), game).is_err());
}
#[test]
fn identity_requires_exact_path_complete_dreamcast_and_consistent_evidence() {
    let game = Path::new("/disposable/game.gdi");
    let mut id = identity(game);
    assert_eq!(verified_product(Some(&id), game).unwrap(), "T-8109N");
    assert!(verified_product(None, game).is_err());
    assert!(verified_product(Some(&id), Path::new("/different.gdi")).is_err());
    id.complete = false;
    assert!(verified_product(Some(&id), game).is_err());
    id.complete = true;
    id.platform = IdentityPlatform::PlayStation;
    assert!(verified_product(Some(&id), game).is_err());
    id.platform = IdentityPlatform::Dreamcast;
    for value in [
        None,
        Some(String::new()),
        Some("../game".into()),
        Some("OTHER".into()),
    ] {
        let mut extra = id.evidence[0].clone();
        extra.value = value;
        id.evidence.push(extra);
        assert!(verified_product(Some(&id), game).is_err());
        id.evidence.pop();
    }
    id.evidence[0].status = IdentityStatus::Candidate;
    assert!(verified_product(Some(&id), game).is_err());
}
#[test]
fn native_import_preview_has_no_profile_or_rom_writes_and_defaults_off() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("local.cht");
    let game = t.path().join("game.iso");
    let profile = t.path().join("emu.cfg");
    fs::write(&source, native(5)).unwrap();
    fs::write(&game, b"synthetic ROM").unwrap();
    fs::write(&profile, b"user config").unwrap();
    let mut s = FlycastPageState::default();
    s.accept(inspect_source(&source));
    assert_eq!(s.selected, vec![false]);
    assert_eq!(s.enabled, vec![false]);
    s.expected_product = "T-8109N".into();
    s.selected[0] = true;
    s.review(&Ok("T-8109N".into())).unwrap();
    assert!(s.reviewed);
    assert_eq!(fs::read(&source).unwrap(), native(5));
    assert_eq!(fs::read(&game).unwrap(), b"synthetic ROM");
    assert_eq!(fs::read(&profile).unwrap(), b"user config");
    assert_eq!(fs::read_dir(t.path()).unwrap().count(), 3);
}
#[test]
fn source_changed_missing_wrong_product_or_warnings_refuse_without_writes() {
    let t = tempfile::tempdir().unwrap();
    let source = t.path().join("local.cht");
    fs::write(&source, native(3)).unwrap();
    let mut s = FlycastPageState::default();
    s.accept(inspect_source(&source));
    s.selected[0] = true;
    s.expected_product = "OTHER".into();
    assert!(s.review(&Ok("T-8109N".into())).is_err());
    s.expected_product = "T-8109N".into();
    fs::write(&source, native(4)).unwrap();
    assert!(s.review(&Ok("T-8109N".into())).is_err());
    assert!(!s.reviewed);
    fs::remove_file(&source).unwrap();
    assert!(s.review(&Ok("T-8109N".into())).is_err());
    let mut duplicate = native(3);
    duplicate.extend_from_slice(b"cheat0_value=10\n");
    fs::write(&source, &duplicate).unwrap();
    s.accept(inspect_source(&source));
    s.selected[0] = true;
    assert!(s.review(&Ok("T-8109N".into())).is_err());
    assert_eq!(fs::read(&source).unwrap(), duplicate);
}
#[test]
fn native_widths_and_opaque_formats_are_distinct() {
    for shift in [3, 4, 5] {
        assert!(supported(
            &parse_flycast_cheat_file(&native(shift)).unwrap().entries[0]
        ));
    }
    for shift in [0, 1, 2, 6] {
        assert!(!supported(
            &parse_flycast_cheat_file(&native(shift)).unwrap().entries[0]
        ));
    }
    assert!(parse_flycast_cheat_file(&vec![b' '; FLYCAST_CHEAT_MAX_BYTES + 1]).is_err());
    assert!(parse_flycast_cheat_file(b"\xff").is_err());
    assert!(
        !parse_flycast_cheat_file(b"[RetroArch]\ncheat0_code=DEADBEEF\n")
            .unwrap()
            .warnings
            .is_empty()
    );
}
#[test]
fn changed_launch_or_identity_invalidates_review() {
    let mut s = FlycastPageState::default();
    let game = Path::new("/disposable/game.gdi");
    s.update_context(game, Some(&identity(game)));
    s.reviewed = true;
    s.launch = Some(Err("Launch choice changed".into()));
    s.update_context(game, Some(&identity(game)));
    assert!(!s.reviewed);
    s.reviewed = true;
    s.update_context(game, None);
    assert!(!s.reviewed);
    let mut id = identity(game);
    s.update_context(game, Some(&id));
    s.reviewed = true;
    id.warnings
        .push("Changed evidence with the same product code".into());
    s.update_context(game, Some(&id));
    assert!(!s.reviewed);
}
#[test]
fn empty_panel_has_no_implicit_io_or_apply_and_reports_blocker() {
    let ctx = egui::Context::default();
    let mut s = FlycastPageState::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024., 600.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show(ui, &mut s, Path::new("/disposable/game.iso"), None)
            });
        },
    );
    assert!(s.receiver.is_none());
    assert!(s.loaded.is_none());
    assert!(!s.reviewed);
    assert!(INSTALL_BLOCKER.contains("Apply is disabled"));
    assert_eq!(
        cheat_apply_support(&CheatRouteTarget::standalone("flycast")),
        CheatApplySupport::InventoryOnly
    );
}

#[test]
fn inventory_only_route_reaches_blue_panel_without_generic_apply() {
    let mut app = crate::tests::app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let target = CheatRouteTarget::standalone("flycast");
    let decision = route_cheat_install(&CheatRouteRequest {
        platform: Some("Dreamcast".into()),
        selected: Some(target.clone()),
        ..Default::default()
    });
    workflow.platform = Some("Dreamcast".into());
    workflow.routing = CheatRoutingState::for_decision(decision, Some(target));
    workflow.adapter = cheat_adapter_for_decision(&workflow.routing.decision);
    let request = GameIdentityRequest {
        archive_path: workflow.archive_path.clone(),
        platform: workflow.platform.clone(),
        adapter: workflow.adapter,
    };
    workflow.identity_request = Some(request.clone());
    workflow.identity = CheatStepResource::Ready((request, identity(&workflow.archive_path)));
    assert!(is_flycast_route(workflow));
    assert_eq!(workflow.adapter, CheatEmulatorAdapter::Unsupported);
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = crate::cheats_mods_preview::show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.emulator_readiness.retroarch_profiles,
                &app.emulator_readiness.pcsx2_profiles,
                &app.emulator_readiness.dolphin_profiles,
                &app.emulator_readiness.xenia_profiles,
                None,
                None,
                &app.history,
                false,
                &mut app.clipboard,
                &mut app.dolphin_texture_mod,
                &mut app.local_mod_package,
            );
        });
    });
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(
        workflow.flycast.context.is_some(),
        "dedicated Flycast panel rendered"
    );
    assert!(!workflow.flycast.reviewed);
    assert!(workflow.flycast.receiver.is_none());
}
