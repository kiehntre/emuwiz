//! Job 41: real Blue layouts with synthetic data only.
use super::*;
use crate::ui::theme;

const LONG: &str = "The Extraordinarily Long Adventures: Collector's Edition (Europe) (Rev 2)";

fn populated(context: &egui::Context) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app.loaded = true;
    app.beginner_hints_enabled = false;
    app.library = Arc::new(Library::new(
        (1..=18)
            .map(|id| {
                archive(
                    id,
                    &format!("{id:02} {}", if id % 2 == 0 { LONG } else { "Short title" }),
                    Some("SNES"),
                )
            })
            .collect(),
    ));
    // Mix portrait and landscape artwork with missing slots. All are synthetic.
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    for (id, size, color) in [
        (1, [120, 160], egui::Color32::from_rgb(40, 95, 160)),
        (2, [160, 90], egui::Color32::from_rgb(75, 130, 100)),
    ] {
        let texture = context.load_texture(
            format!("synthetic-cover-{id}"),
            egui::ColorImage::filled(size, color),
            egui::TextureOptions::LINEAR,
        );
        app.artwork.pictures.insert(
            app.artwork.key(id, Kind::Cover),
            Picture::Ready {
                texture,
                timings: Default::default(),
            },
        );
    }
    app.tools.needs_attention.loaded = true;
    use archivefs_core::attention::*;
    let mut item = AttentionItem::new(
        "visual-fixture".into(),
        AttentionCategory::Dat,
        AttentionSeverity::Warning,
        "Saved game check needs review".into(),
        AttentionDestination::DatReview,
    );
    item.summary =
        "The saved check is out of date. Open the owning workflow to review the evidence.".into();
    item.affected = Some("/synthetic/games/A long game title/Collector Edition/game.rom".into());
    item.provenance = "Synthetic saved evidence for layout review".into();
    item.recoverability = "Review the saved result before choosing a recovery action".into();
    app.tools.needs_attention.snapshot.insert(item);
    app
}

#[test]
fn visual_polish_titles_stay_inside_their_column() {
    let context = egui::Context::default();
    let mut app = populated(&context);
    frame(&context, &mut app, [1920.0, 1080.0]);
    let _ = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(160.0, 100.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let response = super::super::title_fit::title_label(
                        ui,
                        "AnUnbrokenGameTitleThatMustStillFitItsCardWithoutClipping",
                        theme::TextRole::CardTitle,
                        2,
                    );
                    assert!(response.rect.width() <= 161.0, "{:?}", response.rect);
                },
            );
        });
    });
}

#[test]
fn visual_polish_card_metadata_has_one_baseline_per_row() {
    let context = egui::Context::default();
    let mut app = populated(&context);
    app.router.current = Route::BrowsePlay;
    for _ in 0..3 {
        frame(&context, &mut app, [1920.0, 1080.0]);
    }
    let output = frame(&context, &mut app, [1920.0, 1080.0]);
    let mut tops = Vec::new();
    for clipped in &output.shapes {
        if let egui::Shape::Text(text) = &clipped.shape
            && text.galley.text().contains("Platform recognised")
        {
            tops.push(text.pos.y);
        }
    }
    assert!(tops.len() >= 2, "metadata missing");
    assert!((tops[0] - tops[1]).abs() < 1.0, "uneven metadata: {tops:?}");
}

#[test]
fn visual_polish_game_detail_actions_have_equal_bounds() {
    let context = egui::Context::default();
    let mut app = populated(&context);
    app.router.current = Route::Game(2);
    for _ in 0..3 {
        frame(&context, &mut app, [1920.0, 1080.0]);
    }
    let output = frame(&context, &mut app, [1920.0, 1080.0]);
    let button = |label: &str| {
        let text = text_bounds(&output, label)[0];
        output
            .shapes
            .iter()
            .filter_map(|clipped| {
                if let egui::Shape::Rect(shape) = &clipped.shape {
                    let rect = shape.rect;
                    if rect.contains(text.center())
                        && rect.width() >= 100.0
                        && rect.height() >= 40.0
                    {
                        return Some(rect);
                    }
                }
                None
            })
            .min_by(|a, b| a.area().total_cmp(&b.area()))
            .expect("button background")
    };
    let verify = button("Verify");
    for label in [
        "Artwork, Manuals & Extras",
        "Mods & Cheats",
        "Saves & States",
        "Fix Problems",
        "Open Folder",
    ] {
        let rect = button(label);
        assert!(
            (rect.width() - verify.width()).abs() < 1.0
                && (rect.height() - verify.height()).abs() < 1.0,
            "{label}: {rect:?} vs {verify:?}"
        );
    }
    assert!((button("Artwork, Manuals & Extras").top() - verify.top()).abs() < 1.0);
}

#[test]
fn visual_polish_compact_selected_actions_stay_above_the_footer() {
    let context = egui::Context::default();
    let mut app = populated(&context);
    app.router.current = Route::BrowsePlayGame(2);
    for _ in 0..3 {
        frame(&context, &mut app, [1024.0, 600.0]);
    }
    let output = frame(&context, &mut app, [1024.0, 600.0]);
    for label in ["Play", "Game details"] {
        let bounds = text_bounds(&output, label);
        assert!(!bounds.is_empty(), "{label} is below the fold");
        assert!(bounds[0].bottom() < 548.0, "{label}: {:?}", bounds[0]);
    }
}

#[test]
fn visual_polish_compact_selection_keeps_other_games_reachable() {
    let context = egui::Context::default();
    let mut app = populated(&context);
    app.router.current = Route::BrowsePlayGame(2);
    context.style_mut(|style| style.debug.warn_if_rect_changes_id = false);
    // A coarse page-sized wheel jump can skip the title in a nested shelf.
    // Inspect fine wheel movement using the application's real clip rectangle.
    for _ in 0..100 {
        let output = frame(&context, &mut app, [1024.0, 600.0]);
        if visible_text(&output, "01 Short title") {
            return;
        }
        page_reachability::wheel_gesture(&context, &mut app, [1024.0, 600.0], -24.0);
    }
    panic!("the first shelf card is unreachable");
}

#[test]
fn visual_polish_health_content_has_a_readable_line_length() {
    let context = egui::Context::default();
    let mut app = populated(&context);
    let mut used = 0.0_f32;
    for _ in 0..3 {
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1920.0, 1080.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    super::super::needs_attention_page::show(ui, &mut app.tools.needs_attention);
                });
            },
        );
        for shape in &output.shapes {
            if let egui::Shape::Rect(rect) = &shape.shape
                && (rect.fill == theme::blue::CARD_SURFACE_TOP
                    || rect.fill == theme::blue::SELECTED_FILL)
            {
                used = used.max(rect.rect.right() - 8.0);
            }
        }
    }
    assert!(used > 700.0, "health cards missing");
    assert!(used <= 1401.0, "health panels stretched to {used}");
}

#[test]
fn visual_polish_layouts_render_at_both_requested_sizes() {
    for size in [[1920.0, 1080.0], [1024.0, 600.0]] {
        for route in [
            Route::BrowsePlay,
            Route::BrowsePlayGame(2),
            Route::Game(2),
            Route::Tool(super::super::routes::ToolPage::NeedsAttention),
        ] {
            let context = egui::Context::default();
            let mut app = populated(&context);
            app.router.current = route.clone();
            for _ in 0..3 {
                frame(&context, &mut app, size);
            }
            let output = frame(&context, &mut app, size);
            assert!(!output.shapes.is_empty(), "{route:?} at {size:?}");
            fn check_text(shape: &egui::Shape, clip: egui::Rect, route: &Route, size: [f32; 2]) {
                match shape {
                    egui::Shape::Text(text) => {
                        let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                        if bounds.intersects(clip) {
                            assert!(
                                bounds.left() >= clip.left() - 1.0
                                    && bounds.right() <= clip.right() + 1.0,
                                "clipped text {:?} at {route:?} / {size:?}: {bounds:?} in {clip:?}",
                                text.galley.text()
                            );
                        }
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            check_text(shape, clip, route, size);
                        }
                    }
                    _ => {}
                }
            }
            for clipped in &output.shapes {
                check_text(&clipped.shape, clipped.clip_rect, &route, size);
            }
        }
    }
}

/// Manual screenshot entrypoint. Run on a private Xvfb with private HOME/XDG.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "native visual fixture; requires a disposable graphical session"]
fn visual_polish_native_preview() {
    let route = match std::env::var("EMUWIZ_VISUAL_PAGE").as_deref() {
        Ok("details") => Route::Game(2),
        Ok("health") => Route::Tool(super::super::routes::ToolPage::NeedsAttention),
        Ok("selected") => Route::BrowsePlayGame(2),
        _ => Route::BrowsePlay,
    };
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1920.0, 1080.0]),
        ..Default::default()
    };
    use winit::platform::x11::EventLoopBuilderExtX11;
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    struct Preview(App);
    impl eframe::App for Preview {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            // Paint the fixture without polling the live backend or loading a catalogue.
            self.0.imagery.begin_frame(ui.ctx());
            self.0.show(ui.ctx());
        }
    }
    eframe::run_native(
        "EmuWiz visual fixture",
        options,
        Box::new(move |cc| {
            let mut app = populated(&cc.egui_ctx);
            app.router.current = route;
            Ok(Box::new(Preview(app)))
        }),
    )
    .unwrap();
}
