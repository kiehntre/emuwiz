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
fn unowned_and_missing_requests_expire() {
    for (registered, owned, in_content) in [(true, false, true), (false, true, true)] {
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

/// The Library shape: a page scroller whose content holds a nested, virtualised
/// shelf below the visible viewport. The first Shift+Tab with nothing focused
/// lands on the last rendered card. On the first pass that card lies outside the
/// page content rectangle, because the shelf has not scrolled to it yet; the
/// reveal must survive that and bring the card into the page viewport.
#[test]
fn nested_shelf_card_outside_content_on_first_pass_is_still_revealed() {
    use std::cell::RefCell;
    const ROW: f32 = 246.0;
    let context = egui::Context::default();
    // (outer viewport, last rendered card id, rect of every card this pass)
    let seen: RefCell<(egui::Rect, Option<egui::Id>, Vec<(egui::Id, egui::Rect)>)> =
        RefCell::new((egui::Rect::NOTHING, None, Vec::new()));
    let frame = |events: Vec<egui::Event>, shift: bool| {
        seen.borrow_mut().2.clear();
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                modifiers: egui::Modifiers {
                    shift,
                    ..Default::default()
                },
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    ui.allocate_ui(egui::vec2(380.0, 160.0), |ui| {
                        show_focus_reveal_scroll(
                            ui,
                            "nested_shelf_regression",
                            [false, false],
                            |ui| {
                                seen.borrow_mut().0 = ui.clip_rect();
                                // Toolbar, chips and summary above the shelf.
                                ui.allocate_space(egui::vec2(100.0, 200.0));
                                egui::ScrollArea::vertical()
                                    .id_salt("nested_shelf")
                                    .max_height(70.0)
                                    .animated(false)
                                    .show_rows(ui, ROW, 6, |ui, range| {
                                        for _ in range {
                                            let (rect, response) = ui.allocate_exact_size(
                                                egui::vec2(160.0, ROW),
                                                egui::Sense::click(),
                                            );
                                            // The production card scrolls its own shelf.
                                            if response.gained_focus() {
                                                ui.scroll_to_rect(response.rect, None);
                                            }
                                            let mut seen = seen.borrow_mut();
                                            seen.1 = Some(response.id);
                                            seen.2.push((response.id, rect));
                                        }
                                    });
                            },
                        );
                    });
                });
            },
        );
    };
    for _ in 0..4 {
        frame(Vec::new(), false);
    }
    assert_eq!(context.memory(|memory| memory.focused()), None);
    let key_events = [true, false]
        .into_iter()
        .map(|pressed| egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: Some(egui::Key::Tab),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers {
                shift: true,
                ..Default::default()
            },
        })
        .collect();
    frame(key_events, true);
    let target = context
        .memory(|memory| memory.focused())
        .expect("Shift+Tab focused a card");
    for _ in 0..30 {
        frame(Vec::new(), false);
    }
    let (viewport, _, cards) = &*seen.borrow();
    let rect = cards
        .iter()
        .find(|(id, _)| *id == target)
        .map(|(_, rect)| *rect)
        .expect("focused card laid out");
    let shown = rect.intersect(*viewport);
    assert!(
        shown.height() >= 20.0,
        "focused card {rect:?} was not brought into the page viewport {viewport:?}"
    );
}

fn waits(context: &egui::Context) -> u8 {
    context
        .data(|data| data.get_temp::<u8>(key().with("reveal_waits")))
        .unwrap_or(0)
}

#[test]
fn outside_content_request_waits_on_its_own_bounded_budget() {
    let context = egui::Context::default();
    held_request(&context);
    // Outside the content rectangle, as a card in a nested shelf is until the
    // shelf scrolls. Waiting keeps the request alive without spending a scroll
    // attempt, and ends deterministically on the eighth waiting observation.
    for observation in 1..REVEAL_WAIT_PASSES {
        pass(&context, target(), BELOW, true, true, false, vec![]);
        assert_eq!(
            pending(&context),
            Some((target(), 0, 0)),
            "waiting observation {observation} must not spend a scroll attempt"
        );
        assert_eq!(waits(&context), observation);
    }
    pass(&context, target(), BELOW, true, true, false, vec![]);
    assert_eq!(
        pending(&context),
        None,
        "the eighth waiting observation ends it"
    );
    assert_eq!(waits(&context), 0);
    for _ in 0..12 {
        pass(&context, target(), BELOW, true, true, false, vec![]);
        assert_eq!(pending(&context), None, "held focus must not restart it");
    }
}

#[test]
fn waiting_leaves_the_full_scroll_attempt_budget() {
    let context = egui::Context::default();
    held_request(&context);
    for _ in 0..(REVEAL_WAIT_PASSES - 1) {
        pass(&context, target(), BELOW, true, true, false, vec![]);
    }
    assert_eq!(pending(&context), Some((target(), 0, 0)));
    // The shelf has now moved: the target is inside the page content but not
    // yet visible, so each observation is a real scroll attempt.
    for attempt in 1..REVEAL_PASSES {
        pass(&context, target(), BELOW, true, true, true, vec![]);
        assert_eq!(
            pending(&context),
            Some((target(), attempt, 0)),
            "scroll attempt {attempt} after seven waits"
        );
    }
    pass(&context, target(), BELOW, true, true, true, vec![]);
    assert_eq!(pending(&context), None, "the eighth scroll attempt ends it");
}

#[test]
fn manual_input_cancels_a_waiting_request_and_resets_its_budget() {
    let context = egui::Context::default();
    held_request(&context);
    for _ in 0..3 {
        pass(&context, target(), BELOW, true, true, false, vec![]);
    }
    assert_eq!(waits(&context), 3);
    pass(&context, target(), BELOW, true, true, false, vec![wheel()]);
    assert_eq!(pending(&context), None);
    assert_eq!(waits(&context), 0);
    for _ in 0..12 {
        pass(&context, target(), BELOW, true, true, false, vec![]);
        assert_eq!(pending(&context), None, "held focus must not restart it");
    }
}
