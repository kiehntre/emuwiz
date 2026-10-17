use super::*;

#[test]
fn gui_v2_save_vault_is_embedded_and_never_opens_a_classic_window() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.saves_states.inventory = Some(saves_inventory(Vec::new()));
    let first = saves_frame(&context, &mut app, [1280.0, 1200.0]);
    let point = text_bounds(&first, "Open PS2 Save Vault")
        .first()
        .expect("native action")
        .center();
    for pressed in [true, false] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 1200.0),
                )),
                events: vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    super::super::saves_states::show(&mut app, ui, None)
                });
            },
        );
    }
    assert!(app.saves_states.vault_open);
    assert!(app.native_workflows.is_some());
    let shown = text(&saves_frame(&context, &mut app, [1280.0, 1200.0]));
    assert!(shown.iter().any(|s| s.contains("Check PCSX2 Cards")));
    assert!(
        shown
            .iter()
            .any(|s| s.contains("Explicit confirmation required"))
    );
    assert!(
        !shown
            .iter()
            .any(|s| s.contains("Legacy / Advanced interface"))
    );
    assert!(!app.saves_states.vault_advanced);
}

#[test]
fn gui_v2_save_vault_keeps_inventory_evidence_when_returning_to_blue_inventory() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let mut record = state_record(PersistentStateType::MemoryCard, StateEmulator::Pcsx2);
    record.provenance = "synthetic configured profile".into();
    app.saves_states.inventory = Some(saves_inventory(vec![record.clone()]));
    app.saves_states.vault_open = true;
    saves_frame(&context, &mut app, [1280.0, 1200.0]);
    app.saves_states.vault_open = false;
    saves_frame(&context, &mut app, [1280.0, 1200.0]);
    let retained = &app.saves_states.inventory.as_ref().unwrap().records[0];
    assert_eq!(retained.path, record.path);
    assert_eq!(retained.provenance, record.provenance);
    assert_eq!(retained.selected_installation, record.selected_installation);
}
