use super::*;
#[test]
fn defaults_have_no_implicit_selection_activation_confirmation_or_worker() {
    let s = Rpcs3PageState::default();
    assert!(s.selected.is_none());
    assert!(!s.enabled);
    assert!(!s.confirmed);
    assert!(!s.undo_confirmed);
    assert!(s.receiver.is_none());
    assert!(s.preview.is_none());
    assert!(!s.busy());
    assert!(environment().is_err());
}
#[test]
fn changed_launch_selection_invalidates_confirmation_and_inspection() {
    let mut s = Rpcs3PageState::default();
    s.update_context(None);
    s.confirmed = true;
    s.enabled = true;
    s.selected = Some(0);
    s.selection = Some(Err("Launch profile changed".into()));
    s.update_context(None);
    assert!(!s.confirmed);
    assert!(!s.enabled);
    assert!(s.selected.is_none());
}
#[test]
fn interrupted_worker_never_reports_success() {
    let (tx, rx) = mpsc::channel();
    let mut s = Rpcs3PageState {
        receiver: Some(rx),
        mutating: true,
        confirmed: true,
        ..Default::default()
    };
    drop(tx);
    s.poll();
    assert!(s.failure.as_deref().unwrap().contains("interrupted"));
    assert!(s.result.is_none());
    assert!(!s.confirmed);
    assert!(!s.busy());
}
#[test]
fn empty_panel_renders_without_io_or_activation() {
    let ctx = egui::Context::default();
    let mut s = Rpcs3PageState::default();
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
                show(ui, &mut s, Path::new("/disposable/game"), None)
            });
        },
    );
    assert!(s.receiver.is_none());
    assert!(!s.enabled);
    assert!(!s.confirmed);
}
#[test]
fn inventory_capability_requires_local_preview_and_reaches_blue_patch_panel() {
    let mut app = crate::tests::app_with_cheats_mods_context();
    let w = app.cheat_workflow.as_mut().unwrap();
    let target = CheatRouteTarget::standalone("rpcs3");
    let d = route_cheat_install(&CheatRouteRequest {
        platform: Some("PS3".into()),
        selected: Some(target.clone()),
        ..Default::default()
    });
    assert_eq!(
        d.route().unwrap().apply_support,
        CheatApplySupport::LocalPreviewRequired
    );
    assert!(!d.route().unwrap().can_apply());
    w.platform = Some("PS3".into());
    w.routing = CheatRoutingState::for_decision(d, Some(target));
    w.adapter = cheat_adapter_for_decision(&w.routing.decision);
    assert!(is_rpcs3_route(w));
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
    assert!(app.cheat_workflow.as_ref().unwrap().rpcs3.context.is_some());
}

fn launch_plan(
    preference: archivefs_core::launch::CandidatePreference,
) -> archivefs_core::launch::LaunchPlan {
    use archivefs_core::launch::*;
    LaunchPlan {
        platform_id: Some("PS3".into()),
        game_key: Some("BLUS12345".into()),
        summary: LaunchPlanSummary {
            candidates: 1,
            ready: 1,
            ready_with_warnings: 0,
            blocked: 0,
        },
        candidates: vec![LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "rpcs3",
                profile_id: "selected".into(),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: None,
                container: None,
                resolved_path: Some("/disposable/game".into()),
                requires_mount: false,
                provenance: "synthetic".into(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: vec![],
            warnings: vec![],
            readiness: LaunchReadiness::Ready,
            preference,
        }],
        media_topology: None,
    }
}
#[test]
fn blue_reuses_launch_selection_without_changing_auto_or_remembered_plan() {
    use archivefs_core::launch::CandidatePreference;
    for preference in [
        CandidatePreference::SoleEligible,
        CandidatePreference::Remembered,
    ] {
        let p = launch_plan(preference);
        let before = p.clone();
        assert_eq!(
            selected_launch_profile(&p, Path::new("/disposable/game")).unwrap(),
            "selected"
        );
        assert_eq!(p, before);
        assert!(selected_launch_profile(&p, Path::new("/different/game")).is_err());
    }
}
#[test]
fn blue_refuses_ambiguous_blocked_other_emulator_and_mounted_launch() {
    use archivefs_core::launch::*;
    let mut p = launch_plan(CandidatePreference::SoleEligible);
    p.candidates[0].content.requires_mount = true;
    assert!(selected_launch_profile(&p, Path::new("/disposable/game")).is_err());
    p.candidates[0].content.requires_mount = false;
    p.candidates[0].readiness = LaunchReadiness::Blocked;
    assert!(selected_launch_profile(&p, Path::new("/disposable/game")).is_err());
    p.candidates[0].readiness = LaunchReadiness::Ready;
    p.candidates[0].target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "other".into(),
        profile_path: None,
    };
    assert!(selected_launch_profile(&p, Path::new("/disposable/game")).is_err());
    let mut p = launch_plan(CandidatePreference::Undetermined);
    p.candidates.push(p.candidates[0].clone());
    assert!(selected_launch_profile(&p, Path::new("/disposable/game")).is_err());
}

#[test]
fn completed_undo_clears_selection_before_catalog_is_removed() {
    let (tx, rx) = mpsc::channel();
    let mut s = Rpcs3PageState {
        selected: Some(0),
        enabled: true,
        receiver: Some(rx),
        mutating: true,
        catalog: Some(Rpcs3ActivationCatalog {
            title_id: "BLUS12345".into(),
            app_version: "01.03".into(),
            entries: vec![],
        }),
        ..Default::default()
    };
    assert!(
        tx.send(Ok(Message::Undone(SharedRollbackResult {
            preview: SharedRollbackPreview {
                schema_version: 1,
                preview_id: "synthetic".into(),
                journal_path: SharedTransactionPath::from_path(Path::new("/disposable/history")),
                original_operation_id: "synthetic".into(),
                destination_root: SharedTransactionPath::from_path(Path::new(
                    "/disposable/profile"
                )),
                entries: vec![],
                available: true,
            },
            journal_path: None,
            status: SharedApplyStatus::Success,
        })))
        .is_ok()
    );
    s.poll();
    assert!(s.catalog.is_none());
    assert!(
        s.selected.is_none(),
        "a stale index must not enable Preview without a catalog"
    );
    assert!(!s.enabled);
    assert!(!s.confirmed);
}
