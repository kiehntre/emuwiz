use super::*;
fn painted(output: &egui::FullOutput) -> String {
    fn text(s: &egui::Shape, result: &mut String) {
        match s {
            egui::Shape::Text(t) => {
                result.push_str(t.galley.text());
                result.push('\n');
            }
            egui::Shape::Vec(v) => {
                for s in v {
                    text(s, result)
                }
            }
            _ => (),
        }
    }
    let mut result = String::new();
    for s in &output.shapes {
        text(&s.shape, &mut result)
    }
    result
}
#[test]
fn opening_blue_is_read_only_and_explains_supported_loading_contract() {
    let ctx = egui::Context::default();
    let mut state = MelonDsPageState::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 1200.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show(ui, &mut state, Path::new("/disposable/game.nds"))
            });
        },
    );
    let text = painted(&output);
    for label in [
        "Standalone melonDS cheats",
        "1.1 Ubuntu x86-64",
        "usrcheat.dat",
        "in-game effectiveness",
        "installation History / Undo",
    ] {
        assert!(
            text.to_lowercase().contains(&label.to_lowercase()),
            "{label}: {text}"
        );
    }
    assert!(state.receiver.is_none());
    assert!(state.preview.is_none());
    assert!(state.result.is_none());
    assert!(!state.confirmed);
    assert!(!state.busy());
}
#[test]
fn interrupted_worker_reports_uncertainty_and_clears_confirmation() {
    let (tx, rx) = mpsc::channel();
    drop(tx);
    let mut state = MelonDsPageState {
        receiver: Some(rx),
        mutating: true,
        confirmed: true,
        ..Default::default()
    };
    state.poll();
    assert!(!state.busy());
    assert!(!state.confirmed);
    assert!(
        state
            .failure
            .unwrap()
            .contains("inspect History before retrying")
    );
}
#[test]
fn routed_support_requires_local_preview_and_does_not_claim_general_apply() {
    let target = CheatRouteTarget::standalone("melonds");
    assert_eq!(
        cheat_apply_support(&target),
        CheatApplySupport::LocalPreviewRequired
    );
    let decision = route_cheat_install(&CheatRouteRequest {
        platform: Some("NDS".into()),
        selected: Some(target),
        ..Default::default()
    });
    assert!(
        matches!(decision, CheatRouteDecision::Routed(_)),
        "{decision:?}"
    );
    if let CheatRouteDecision::Routed(route) = decision {
        assert!(!route.can_apply());
    }
}
