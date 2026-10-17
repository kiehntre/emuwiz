use super::super::organisation::OrganisationView;
use super::*;

#[test]
fn blue_repair_navigation_embeds_review_and_history_at_supported_sizes() {
    for size in [[1920.0, 1080.0], [1440.0, 900.0], [1024.0, 600.0]] {
        let ctx = egui::Context::default();
        // egui's debug-only ID-shift overlay paints with Rect::EVERYTHING
        // when async loading gives way to content. Check the application's
        // viewport, with the same diagnostic setting as a release build.
        ctx.style_mut(|style| style.debug.warn_if_rect_changes_id = false);
        let mut app = organisation_app(&ctx);
        frame(&ctx, &mut app, size);
        let shown = frame(&ctx, &mut app, size);
        let targets = text_bounds(&shown, "Repair Review");
        assert!(
            !targets.is_empty(),
            "Repair Review unavailable at {size:?}: {:?}",
            text(&shown)
        );
        let point = targets[0].center();
        click_at(&ctx, &mut app, size, point);
        assert_eq!(app.organisation.view, OrganisationView::RepairReview);
        frame(&ctx, &mut app, size);
        let output = frame(&ctx, &mut app, size);
        assert_viewport_is(&ctx, &output, size);
        let labels = text(&output).join("\n");
        assert!(labels.contains("Load repair plan"));
        assert!(!labels.contains("Open specialist interface"));
        assert!(app.native_workflows.is_some());
        // Other History tests write restart receipts to the process-wide
        // default journal directory. This navigation fixture needs its own
        // genuinely empty history, including the real asynchronous loader.
        let history_dir = tempfile::tempdir().unwrap();
        app.native_workflows
            .as_mut()
            .unwrap()
            .app
            .repair_history_page = Some(
            crate::repair_history_page::RepairHistoryPageState::load_background(
                history_dir.path().to_path_buf(),
            ),
        );
        // At 1024x600 the shared Organisation/Mr Wiz header leaves the
        // empty-state card below the fold. Prove it is reachable by ordinary
        // scrolling, then return to the navigation; never assume a taller canvas.
        let empty = scroll_until(&ctx, &mut app, size, "No repair plan loaded");
        assert_viewport_is(&ctx, &empty, size);
        scroll(&ctx, &mut app, size, 10000.0);
        let output = frame(&ctx, &mut app, size);
        let point = text_bounds(&output, "Repair History")[0].center();
        click_at(&ctx, &mut app, size, point);
        assert_eq!(app.organisation.view, OrganisationView::RepairHistory);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let output = frame(&ctx, &mut app, size);
            assert_viewport_is(&ctx, &output, size);
            let page = app
                .native_workflows
                .as_ref()
                .unwrap()
                .app
                .repair_history_page
                .as_ref()
                .unwrap();
            if !page.background_busy() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let empty = scroll_until(&ctx, &mut app, size, "No repair transactions found");
        assert_viewport_is(&ctx, &empty, size);
    }
}

fn scroll(ctx: &egui::Context, app: &mut App, size: [f32; 2], delta: f32) {
    frame_with(
        ctx,
        app,
        size,
        vec![
            egui::Event::PointerMoved(egui::pos2(size[0] - 100.0, size[1] - 120.0)),
            egui::Event::MouseWheel {
                phase: egui::TouchPhase::Move,
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, delta),
                modifiers: Default::default(),
            },
        ],
    );
    frame(ctx, app, size);
}

fn scroll_until(
    ctx: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    label: &str,
) -> egui::FullOutput {
    for _ in 0..10 {
        let output = frame(ctx, app, size);
        if organisation_label_is_visible(&output, label) {
            return output;
        }
        scroll(ctx, app, size, -150.0);
    }
    panic!("{label} never became reachable at {size:?}");
}

fn click_at(ctx: &egui::Context, app: &mut App, size: [f32; 2], point: egui::Pos2) {
    for pressed in [true, false] {
        frame_with(
            ctx,
            app,
            size,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
}

#[test]
fn blue_repair_review_keyboard_navigation_keeps_preview_read_only() {
    let size = [1024.0, 600.0];
    let ctx = egui::Context::default();
    let mut app = organisation_app(&ctx);
    app.organisation.view = OrganisationView::RepairReview;
    frame(&ctx, &mut app, size);
    let output = frame(&ctx, &mut app, size);
    let target = text_bounds(&output, "Repair History")[0];
    let mut focused = false;
    for _ in 0..60 {
        frame_with(
            &ctx,
            &mut app,
            size,
            vec![org_key_event(egui::Key::Tab, true)],
        );
        frame_with(
            &ctx,
            &mut app,
            size,
            vec![org_key_event(egui::Key::Tab, false)],
        );
        focused = ctx
            .memory(|m| m.focused())
            .and_then(|id| ctx.read_response(id))
            .is_some_and(|r| r.rect.contains_rect(target));
        if focused {
            break;
        }
    }
    assert!(focused);
    frame_with(
        &ctx,
        &mut app,
        size,
        vec![org_key_event(egui::Key::Enter, true)],
    );
    frame_with(
        &ctx,
        &mut app,
        size,
        vec![org_key_event(egui::Key::Enter, false)],
    );
    assert_eq!(app.organisation.view, OrganisationView::RepairHistory);
    let workflows = app.native_workflows.as_ref().unwrap();
    let review = workflows.app.repair_review_page.as_ref().unwrap();
    assert!(!review.is_apply_running());
    assert!(review.apply_result.is_none());
}
