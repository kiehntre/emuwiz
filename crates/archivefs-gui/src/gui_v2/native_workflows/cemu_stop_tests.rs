//! Actual Blue launch-page clicks while fresh launch/readiness UI is disabled.
use super::*;
use archivefs_core::launch::{
    cemu_command::{CemuCommand, CemuCommandSelection},
    cemu_execution::spawn_cemu,
};

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
fn cemu_stop_blue_clicks_remain_enabled_with_blocked_readiness_and_selection_change() {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original.rpx");
    let other = root.path().join("other.rpx");
    std::fs::write(&original, b"synthetic unsupported RPX").unwrap();
    std::fs::write(&other, b"different unsupported RPX").unwrap();
    let context = egui::Context::default();
    let mut workflows = NativeWorkflows::new(context.clone());
    let process = spawn_cemu(CemuCommand {
        executable: "/bin/sleep".into(),
        arguments: vec!["10".into()],
        working_directory: None,
        selection: CemuCommandSelection {
            profile_id: "private-test-profile".into(),
            platform_id: "WiiU".into(),
            content_path: original.clone(),
            rpx_path: original.clone(),
            title_id: None,
        },
    })
    .unwrap();
    workflows
        .app
        .launch_standalone
        .track_test_cemu(original.clone(), process);
    frame(&context, &mut workflows, &other, vec![]);
    let output = frame(&context, &mut workflows, &other, vec![]);
    visible_center(&output, "Running");
    visible_center(
        &output,
        &format!("Launched content: {}", original.display()),
    );
    let stop = visible_center(&output, "Stop Cemu");
    // Stop is drawn but disabled until the explicit confirmation is checked.
    click(&context, &mut workflows, &other, stop);
    assert!(workflows.app.launch_standalone.is_active());
    let output = frame(&context, &mut workflows, &other, vec![]);
    let confirmation = visible_center(
        &output,
        "I confirm: request graceful Stop for this Cemu process",
    );
    click(&context, &mut workflows, &other, confirmation);
    let output = frame(&context, &mut workflows, &other, vec![]);
    let stop = visible_center(&output, "Stop Cemu");
    click(&context, &mut workflows, &other, stop);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while workflows.app.launch_standalone.is_active() {
        workflows.app.launch_standalone.poll();
        assert!(
            std::time::Instant::now() < deadline,
            "confirmed Stop did not reach the owned watcher"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let output = frame(&context, &mut workflows, &other, vec![]);
    visible_center(
        &output,
        "Exited · Cemu process terminated after the graceful Stop request.",
    );
    assert_eq!(
        std::fs::read(&original).unwrap(),
        b"synthetic unsupported RPX"
    );
    assert_eq!(std::fs::read(&other).unwrap(), b"different unsupported RPX");
}
