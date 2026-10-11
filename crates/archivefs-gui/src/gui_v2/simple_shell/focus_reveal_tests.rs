use super::*;

const CLIP: egui::Rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 100.0));
const BELOW: egui::Rect =
    egui::Rect::from_min_max(egui::pos2(20.0, 300.0), egui::pos2(180.0, 344.0));
const INSIDE: egui::Rect =
    egui::Rect::from_min_max(egui::pos2(20.0, 20.0), egui::pos2(180.0, 64.0));

fn key() -> egui::Id {
    egui::Id::new("focus_reveal_regression")
}

fn target() -> egui::Id {
    key().with("target")
}

fn pending(context: &egui::Context) -> Option<(egui::Id, u8, u8)> {
    context
        .data(|data| data.get_temp::<Option<(egui::Id, u8, u8)>>(key().with("pending_reveal")))?
}

fn held_request(context: &egui::Context) {
    context.data_mut(|data| {
        data.insert_temp(key().with("last_focus"), Some(target()));
        data.insert_temp(key().with("pending_reveal"), Some((target(), 0_u8, 0_u8)));
    });
}

/// Render a registered control with an independently specified content and
/// clip rectangle. Focus is held explicitly so pointer cancellation cannot
/// pass merely because a gutter click happened to clear focus first.
fn pass(
    context: &egui::Context,
    id: egui::Id,
    rect: egui::Rect,
    registered: bool,
    owned: bool,
    in_content: bool,
    events: Vec<egui::Event>,
) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 400.0),
            )),
            events,
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |parent| {
                let mut ui = parent.new_child(egui::UiBuilder::new().max_rect(CLIP));
                ui.set_clip_rect(CLIP);
                if registered {
                    let _ = ui.interact(rect, id, egui::Sense::click());
                }
                if in_content {
                    ui.expand_to_include_rect(rect);
                }
                context.memory_mut(|memory| memory.request_focus(id));
                let controls = [id];
                reveal_new_keyboard_focus(&mut ui, key(), if owned { &controls } else { &[] });
            });
        },
    );
}

fn wheel() -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 80.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    }
}

fn button(pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: egui::pos2(190.0, 90.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn wheel_cancels_pending_reveal_with_unchanged_focus() {
    let context = egui::Context::default();
    held_request(&context);
    pass(&context, target(), BELOW, true, true, true, vec![wheel()]);
    assert_eq!(context.memory(|memory| memory.focused()), Some(target()));
    assert_eq!(
        pending(&context),
        None,
        "manual wheel must win over pending reveal"
    );
    for _ in 0..12 {
        pass(&context, target(), BELOW, true, true, true, vec![]);
        assert_eq!(
            pending(&context),
            None,
            "held focus must not restart old reveal"
        );
    }
}

#[test]
fn pointer_press_and_release_cancel_pending_even_if_focus_stays() {
    for pressed in [true, false] {
        let context = egui::Context::default();
        if !pressed {
            pass(
                &context,
                target(),
                BELOW,
                true,
                true,
                true,
                vec![button(true)],
            );
        }
        held_request(&context);
        pass(
            &context,
            target(),
            BELOW,
            true,
            true,
            true,
            vec![button(pressed)],
        );
        assert_eq!(context.memory(|memory| memory.focused()), Some(target()));
        assert_eq!(
            pending(&context),
            None,
            "pointer pressed={pressed} must cancel"
        );
    }
}

#[test]
fn held_pointer_drag_does_not_start_a_new_keyboard_reveal() {
    let context = egui::Context::default();
    pass(
        &context,
        target(),
        BELOW,
        true,
        true,
        true,
        vec![button(true)],
    );
    let next = key().with("next");
    pass(
        &context,
        next,
        BELOW,
        true,
        true,
        true,
        vec![egui::Event::PointerMoved(egui::pos2(190.0, 75.0))],
    );
    assert!(context.input(|input| input.pointer.any_down()));
    assert_eq!(
        pending(&context),
        None,
        "a held scrollbar drag is manual input"
    );
}

#[test]
fn new_keyboard_focus_after_manual_scroll_starts_fresh_reveal() {
    let context = egui::Context::default();
    held_request(&context);
    pass(&context, target(), BELOW, true, true, true, vec![wheel()]);
    assert_eq!(pending(&context), None);
    let next = key().with("next");
    pass(&context, next, BELOW, true, true, true, vec![]);
    assert_eq!(pending(&context), Some((next, 1, 0)));
}

#[test]
fn unowned_missing_and_outside_content_requests_expire() {
    for (registered, owned, in_content) in [
        (true, false, true),
        (false, true, true),
        (true, true, false),
    ] {
        let context = egui::Context::default();
        held_request(&context);
        pass(
            &context,
            target(),
            BELOW,
            registered,
            owned,
            in_content,
            vec![],
        );
        assert_eq!(
            pending(&context),
            None,
            "registered={registered} owned={owned} in_content={in_content}"
        );
    }
}

#[test]
fn reveal_lifetime_is_bounded_when_geometry_alternates_inside_and_outside() {
    let context = egui::Context::default();
    held_request(&context);
    for index in 0..REVEAL_PASSES {
        let rect = if index % 2 == 0 { BELOW } else { INSIDE };
        pass(&context, target(), rect, true, true, true, vec![]);
    }
    assert_eq!(
        pending(&context),
        None,
        "all observations must count toward the bound"
    );
    for _ in 0..12 {
        pass(&context, target(), BELOW, true, true, true, vec![]);
        assert_eq!(pending(&context), None);
    }
}

#[test]
fn stable_visible_focus_finishes_after_three_observations() {
    let context = egui::Context::default();
    held_request(&context);
    for _ in 0..REVEAL_STABLE_PASSES {
        pass(&context, target(), INSIDE, true, true, true, vec![]);
    }
    assert_eq!(pending(&context), None);
}
