//! Test real page reachability without enlarging the viewport or accepting
//! text painted outside its clip rectangle.
use super::*;

#[test]
fn egui_id_shift_diagnostic_uses_an_unbounded_clip_in_debug_builds() {
    let context = egui::Context::default();
    let size = [1024.0, 600.0];
    let render = |skip| {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    ui.skip_ahead_auto_ids(skip);
                    ui.label("Synthetic loading/content transition");
                });
            },
        )
    };
    render(0);
    let diagnostic = render(1);
    assert_eq!(
        diagnostic
            .shapes
            .iter()
            .any(|shape| shape.clip_rect == egui::Rect::EVERYTHING),
        cfg!(debug_assertions)
    );
    context.style_mut(|style| style.debug.warn_if_rect_changes_id = false);
    let application = render(0);
    assert_viewport_is(&context, &application, size);
}

pub(super) fn wheel_gesture(context: &egui::Context, app: &mut App, size: [f32; 2], delta: f32) {
    // A complete touch gesture applies its delta directly. A lone Move with
    // a large delta is smoothed across later frames, including pointer clicks.
    for phase in [
        egui::TouchPhase::Start,
        egui::TouchPhase::Move,
        egui::TouchPhase::End,
    ] {
        frame_with(
            context,
            app,
            size,
            vec![
                egui::Event::PointerMoved(egui::pos2(size[0] - 100.0, size[1] - 100.0)),
                egui::Event::MouseWheel {
                    phase,
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(
                        0.0,
                        if phase == egui::TouchPhase::Move {
                            delta
                        } else {
                            0.0
                        },
                    ),
                    modifiers: Default::default(),
                },
            ],
        );
    }
}

pub(super) fn visible_text(output: &egui::FullOutput, wanted: &str) -> bool {
    fn contains(shape: &egui::Shape, clip: egui::Rect, wanted: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => {
                text.galley.text().contains(wanted)
                    && clip.contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, clip, wanted)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|shape| contains(&shape.shape, shape.clip_rect, wanted))
}

pub(super) fn scroll_to_text(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    wanted: &str,
) -> egui::FullOutput {
    // Check application clips as in release builds, not egui's unbounded
    // ID-shift diagnostic overlay (proved by the test above).
    context.style_mut(|style| style.debug.warn_if_rect_changes_id = false);
    frame(context, app, size);
    wheel_gesture(context, app, size, 10000.0);
    for _ in 0..40 {
        let output = frame(context, app, size);
        assert_viewport_is(context, &output, size);
        if visible_text(&output, wanted) {
            return output;
        }
        wheel_gesture(context, app, size, -120.0);
    }
    panic!(
        "{wanted} never became visible at {size:?}: {:?}",
        text(&frame(context, app, size))
    );
}
