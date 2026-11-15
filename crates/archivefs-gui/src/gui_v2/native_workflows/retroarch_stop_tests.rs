//! Actual Blue launch-page clicks while fresh launch/readiness UI is disabled.
use super::*;
use crate::launch_readiness_page::RetroArchLaunchDisplay;

fn frame(
    ctx: &egui::Context,
    workflows: &mut NativeWorkflows,
    path: &Path,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024., 600.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                workflows.show_launch(ui, 1, path, None, &mut Activity::default());
            });
        },
    )
}

fn visible_center(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    fn visit(shape: &egui::Shape, clip: egui::Rect, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == label => {
                let rect = egui::Rect::from_min_size(t.pos, t.galley.size());
                clip.contains_rect(rect).then_some(rect.center())
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| visit(shape, clip, label)),
            _ => None,
        }
    }
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 600.));
    output
        .shapes
        .iter()
        .find_map(|s| visit(&s.shape, s.clip_rect.intersect(viewport), label))
        .unwrap_or_else(|| panic!("{label} not visible at 1024x600"))
}

fn click(
    ctx: &egui::Context,
    workflows: &mut NativeWorkflows,
    path: &Path,
    pos: egui::Pos2,
) -> egui::FullOutput {
    let mut output = frame(ctx, workflows, path, vec![]);
    for pressed in [true, false] {
        output = frame(
            ctx,
            workflows,
            path,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    output
}

#[test]
fn retroarch_stop_blue_clicks_work_while_the_launch_panel_is_disabled() {
    let root = tempfile::tempdir().unwrap();
    let other = root.path().join("other.bin");
    std::fs::write(&other, b"another game").unwrap();
    let context = egui::Context::default();
    let mut workflows = NativeWorkflows::new(context.clone());
    let request = crate::launch_readiness_page::retroarch_test_request("/library/Game.bin");
    workflows
        .app
        .launch_retroarch
        .track_running_for_test(&request);
    frame(&context, &mut workflows, &other, vec![]);
    let output = frame(&context, &mut workflows, &other, vec![]);

    // Stop is drawn but does nothing until the confirmation is ticked.
    let stop = visible_center(&output, "Stop RetroArch");
    click(&context, &mut workflows, &other, stop);
    workflows.app.launch_retroarch.poll();
    assert!(workflows.app.launch_retroarch.is_active());

    let output = frame(&context, &mut workflows, &other, vec![]);
    let confirmation = visible_center(&output, "I confirm: ask RetroArch to close Game.bin");
    click(&context, &mut workflows, &other, confirmation);
    let output = frame(&context, &mut workflows, &other, vec![]);
    let stop = visible_center(&output, "Stop RetroArch");
    click(&context, &mut workflows, &other, stop);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while workflows.app.launch_retroarch.is_active() {
        workflows.app.launch_retroarch.poll();
        assert!(
            std::time::Instant::now() < deadline,
            "confirmed Stop did not reach the tracked launch"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(matches!(
        workflows.app.launch_retroarch.display_for(&request),
        RetroArchLaunchDisplay::Exited {
            stop_outcome: Some(_),
            ..
        }
    ));
    assert_eq!(std::fs::read(&other).unwrap(), b"another game");
}
