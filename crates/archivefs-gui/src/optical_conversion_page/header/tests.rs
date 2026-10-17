use super::*;

fn frame(
    ctx: &egui::Context,
    state: &mut OpticalConversionPageState,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> [egui::Response; 3] {
    crate::gui_v2::readable_style(ctx);
    ctx.style_mut(|style| {
        style.visuals.faint_bg_color = theme::blue::CARD_SURFACE;
        style.spacing.button_padding = egui::vec2(16.0, 11.0);
    });
    let modifiers = events
        .iter()
        .find_map(|event| match event {
            egui::Event::Key { modifiers, .. } => Some(*modifiers),
            _ => None,
        })
        .unwrap_or_default();
    let mut result = None;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            modifiers,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                result = Some(controls(ui, state));
            });
        },
    );
    result.unwrap()
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

fn click(pos: egui::Pos2) -> Vec<egui::Event> {
    let mut events = vec![egui::Event::PointerMoved(pos)];
    for pressed in [true, false] {
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    events
}

#[test]
fn native_buttons_have_real_disabled_states_and_visible_reasons() {
    let ctx = egui::Context::default();
    let mut state = OpticalConversionPageState::default();
    let buttons = frame(&ctx, &mut state, egui::vec2(800.0, 600.0), vec![]);
    assert!(buttons[0].enabled());
    assert!(!buttons[1].enabled());
    assert!(!buttons[2].enabled());
    frame(
        &ctx,
        &mut state,
        egui::vec2(800.0, 600.0),
        click(buttons[2].rect.center()),
    );
    assert!(!state.previewed);
    state.undo_incomplete = Some("synthetic incomplete Undo".into());
    let buttons = frame(&ctx, &mut state, egui::vec2(800.0, 600.0), vec![]);
    assert!(buttons.iter().all(|button| !button.enabled()));
    assert!(state.undo_incomplete.is_some());
}

#[test]
fn mouse_scan_and_keyboard_preview_preserve_source_and_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let cue = super::super::tests::source(dir.path());
    let before = std::fs::read(&cue).unwrap();
    let mut state = OpticalConversionPageState {
        source_root_draft: dir.path().display().to_string(),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let size = egui::vec2(800.0, 600.0);
    let buttons = frame(&ctx, &mut state, size, vec![]);
    frame(&ctx, &mut state, size, click(buttons[1].rect.center()));
    assert!(state.scanned);
    assert_eq!(state.candidates.len(), 1);
    state.selected = Some(0);
    let buttons = frame(&ctx, &mut state, size, vec![]);
    assert!(buttons[2].enabled());
    ctx.memory_mut(|memory| memory.request_focus(buttons[1].id));
    frame(
        &ctx,
        &mut state,
        size,
        key(egui::Key::Tab, egui::Modifiers::NONE),
    );
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(buttons[2].id));
    frame(
        &ctx,
        &mut state,
        size,
        key(egui::Key::Enter, egui::Modifiers::NONE),
    );
    assert!(state.previewed);
    assert!(!state.confirm);
    assert!(state.transaction.is_none());
    assert!(state.result.is_none());
    assert_eq!(std::fs::read(&cue).unwrap(), before);
    frame(
        &ctx,
        &mut state,
        size,
        key(egui::Key::Tab, egui::Modifiers::SHIFT),
    );
    // egui applies backwards focus navigation on the following frame.
    frame(&ctx, &mut state, size, vec![]);
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(buttons[1].id));
    frame(
        &ctx,
        &mut state,
        size,
        key(egui::Key::Space, egui::Modifiers::NONE),
    );
    assert!(state.scanned);
    assert!(
        !state.previewed,
        "Space activated Scan and invalidated the preview"
    );
    assert!(state.selected.is_none());
    assert!(!state.confirm);
    assert_eq!(std::fs::read(&cue).unwrap(), before);
}

#[test]
fn unsupported_selection_stays_disabled_and_failed_preview_is_truthful() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = OpticalConversionPageState::default();
    state.selected_context = Some(SelectedConversionContext {
        path: dir.path().join("unsupported.gdi"),
        platform: None,
    });
    let ctx = egui::Context::default();
    let size = egui::vec2(800.0, 600.0);
    assert!(!frame(&ctx, &mut state, size, vec![])[2].enabled());
    state.selected_context.as_mut().unwrap().path = dir.path().join("missing.cue");
    let buttons = frame(&ctx, &mut state, size, vec![]);
    frame(&ctx, &mut state, size, click(buttons[2].rect.center()));
    assert!(!state.previewed);
    assert!(
        state
            .hero_hint
            .as_deref()
            .unwrap()
            .starts_with("Preview unavailable")
    );
    assert!(state.candidates[0].error.is_some());
}

#[test]
fn header_and_actions_fit_target_viewports_without_overlapping_next_section() {
    for size in [
        egui::vec2(1920.0, 1080.0),
        egui::vec2(1440.0, 900.0),
        egui::vec2(1024.0, 600.0),
    ] {
        let ctx = egui::Context::default();
        crate::gui_v2::readable_style(&ctx);
        ctx.style_mut(|style| style.visuals.faint_bg_color = theme::blue::CARD_SURFACE);
        // Reserve the production rail and shell margins, including compact width.
        let width = size.x - if size.x <= 1024.0 { 90.0 } else { 300.0 };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_max_width(width);
                    let mut state = OpticalConversionPageState::default();
                    let header_top = ui.cursor().top();
                    assert!(show_disc_conversion_hero(ui, &mut state));
                    let header_bottom = ui.cursor().top();
                    let limit = if size.y == 600.0 {
                        220.0
                    } else {
                        size.y - 180.0
                    };
                    let header_height = header_bottom - header_top;
                    assert!(header_height < limit, "{size:?}: {header_height}");
                    assert!(ui.heading("Next section").rect.top() >= header_bottom);
                });
            },
        );
        let mut state = OpticalConversionPageState::default();
        let buttons = frame(&ctx, &mut state, egui::vec2(width, size.y), vec![]);
        for button in &buttons {
            assert!(button.rect.right() < width);
            assert!(button.rect.bottom() < size.y);
        }
        for pair in buttons.windows(2) {
            assert!(!pair[0].rect.intersects(pair[1].rect));
        }
    }
}
