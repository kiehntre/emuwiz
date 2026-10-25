use super::*;
use crate::tests::InMemoryClipboard;
use archivefs_core::game_identity::*;
fn identity(game: &Path) -> GameIdentityReport {
    GameIdentityReport {
        archive_path: game.into(),
        platform: IdentityPlatform::PlayStation,
        format: IdentityImageFormat::Iso,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::PspDiscId,
            status: IdentityStatus::Verified,
            value: Some("ULUS10000".into()),
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
fn text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => {
            for s in v {
                text(s, out)
            }
        }
        _ => {}
    }
}
fn frame(
    state: &mut State,
    game: &Path,
    id: Option<&GameIdentityReport>,
    size: egui::Vec2,
) -> String {
    let context = egui::Context::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| show(ui, state, game, id));
            });
        },
    );
    let mut rendered = String::new();
    for s in output.shapes {
        text(&s.shape, &mut rendered)
    }
    rendered
}
#[test]
fn identity_and_explicit_profile_gate_install_and_are_honest_at_small_sizes() {
    let game = Path::new("/disposable/game.iso");
    for size in [egui::vec2(1024.0, 600.0), egui::vec2(800.0, 600.0)] {
        let mut state = State::default();
        let without = frame(&mut state, game, None, size);
        assert!(without.contains("Inspect PSP identity first"));
        assert!(!without.contains("Confirm and install"));
        let id = identity(game);
        let rendered = frame(&mut state, game, Some(&id), size);
        assert!(rendered.contains("ULUS10000"));
        assert!(rendered.contains("Choose supported PPSSPP AppImage"));
        assert!(rendered.contains("Installation does not enable cheats"));
        assert!(!rendered.contains("Confirm and install disabled cheats"));
        assert!(state.environment().is_err());
        assert!(state.preview.is_none());
    }
}
#[test]
fn changed_identity_revokes_confirmation_and_late_worker_results() {
    let game = Path::new("/disposable/game.iso");
    let mut state = State::default();
    let mut id = identity(game);
    frame(&mut state, game, Some(&id), egui::vec2(1024.0, 600.0));
    state.confirmed = true;
    state.undo_confirmed = true;
    let old = state.generation;
    let (tx, rx) = mpsc::channel();
    state.receiver = Some(rx);
    id.evidence[0].value = Some("ULES00001".into());
    frame(&mut state, game, Some(&id), egui::vec2(1024.0, 600.0));
    assert!(!state.confirmed && !state.undo_confirmed);
    assert!(state.generation > old);
    tx.send((old, Err("old worker failure".into()))).unwrap();
    state.poll();
    assert!(state.error.is_none());
    assert!(state.preview.is_none());
}
#[test]
fn uncertain_worker_exit_never_becomes_success_or_keeps_approval() {
    let mut state = State::default();
    let (tx, rx) = mpsc::channel();
    state.receiver = Some(rx);
    state.mutating = true;
    state.notice = Some("Operation running".into());
    state.confirmed = true;
    state.undo_confirmed = true;
    assert!(state.busy());
    drop(tx);
    state.poll();
    assert!(!state.busy());
    assert!(state.notice.is_none());
    assert!(!state.confirmed && !state.undo_confirmed);
    assert!(state.error.as_deref().unwrap().contains("inspect History"));
    assert!(state.preview.is_none());
}

#[test]
fn full_page_ppsspp_route_and_diagnostics_offer_local_preview() {
    let mut app = crate::tests::app_with_cheats_mods_context();
    let context = egui::Context::default();
    let temporary = tempfile::tempdir().unwrap();
    let game = temporary.path().join("game.iso");
    {
        let w = app.cheat_workflow.as_mut().unwrap();
        w.archive_path = game.clone();
        w.platform = Some("PSP".into());
        w.adapter = CheatEmulatorAdapter::Unsupported;
        w.routing.decision = pm::route_cheat_install(&pm::CheatRouteRequest {
            platform: Some("PSP".into()),
            selected: Some(pm::CheatRouteTarget::standalone("ppsspp")),
            ..Default::default()
        });
    }
    // The shared identity controller must admit this conditional native route.
    app.start_game_identity_inspection(context.clone());
    assert!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .identity_request
            .is_some()
    );
    let w = app.cheat_workflow.as_mut().unwrap();
    w.identity = CheatStepResource::Ready((w.identity_request.clone().unwrap(), identity(&game)));
    let mut clipboard = InMemoryClipboard::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 1600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_cheats_mods_page(
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
                    &mut clipboard,
                    &mut Default::default(),
                    &mut Default::default(),
                );
                // Exercise expanded diagnostics independently of the default collapse.
                show_cheats_mods_workflow_states(
                    ui,
                    app.cheat_workflow.as_ref(),
                    &app.emulator_readiness.retroarch_profiles,
                    &app.emulator_readiness.pcsx2_profiles,
                    &app.emulator_readiness.dolphin_profiles,
                );
            });
        },
    );
    let mut rendered = String::new();
    for shape in output.shapes {
        text(&shape.shape, &mut rendered);
    }
    assert!(
        rendered.contains("Choose supported PPSSPP AppImage"),
        "{rendered}"
    );
    assert!(rendered.contains("PPSSPP local CWCheat installation"));
    assert!(!rendered.contains("Unsupported platform"));
    assert!(!rendered.contains("GUI workflow does not apply it yet"));
}
