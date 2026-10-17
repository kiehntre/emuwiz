use super::super::{
    imagery::{EmptyArt, Imagery, empty_state},
    library::Library,
    tests::{archive, fixture},
};
use super::*;
use std::sync::Arc;

fn texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
    fn collect(s: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match s {
            egui::Shape::Text(t) => out.push((
                t.galley.text().into(),
                egui::Rect::from_min_size(t.pos, t.galley.size()),
            )),
            egui::Shape::Vec(v) => {
                for s in v {
                    collect(s, out);
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for s in &output.shapes {
        collect(&s.shape, &mut out);
    }
    out
}
fn raw(size: egui::Vec2, events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        events,
        ..Default::default()
    }
}
fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn blue_polish_long_recent_title_keeps_home_actions_compact_and_keyboard_reachable() {
    for width in [1024.0, 640.0] {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.simple.enabled = true;
        app.simple.sync_style(&context);
        let title = "The Long Adventures of a Game With Collector's Edition Extras ".repeat(10);
        app.library = Arc::new(Library::new(vec![archive(1, &title, Some("SNES"))]));
        app.imagery.note_opened(1);
        let size = egui::vec2(width, 600.0);
        let panel = std::cell::Cell::new(egui::Rect::NOTHING);
        let mut render = |events| {
            context.run(raw(size, events), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    panel.set(
                        ui.scope(|ui| app.home_welcome_panel(ui, true))
                            .response
                            .rect,
                    );
                });
            })
        };
        let initial = render(vec![]);
        capture(&context, &initial, &format!("home-{width}-0"));
        let output = render(vec![]);
        capture(&context, &output, &format!("home-{width}-1"));
        let labels = texts(&output);
        assert!(
            panel.get().height() <= 235.0,
            "Recent title consumes the compact Home: {:?}",
            panel.get()
        );
        for label in ["Open game", "Open Library"] {
            let rect = labels.iter().find(|(s, _)| s == label).unwrap().1;
            assert!(
                rect.right() <= width && rect.bottom() <= 235.0,
                "{label}: {rect:?}"
            );
        }
        // Only the two existing navigation actions can receive focus here.
        render(vec![key(egui::Key::Tab)]);
        render(vec![key(egui::Key::Enter)]);
        assert_eq!(app.router.current, Route::Game(1));
        assert!(
            app.activity
                .jobs
                .values()
                .all(|job| matches!(job.title.as_str(), "Preparing artwork" | "Loading artwork")),
            "Opening details must not launch, verify or mutate games"
        );
    }
}

#[test]
fn blue_polish_empty_state_uses_the_same_primary_control_as_other_blue_cards() {
    for size in [egui::vec2(1024.0, 600.0), egui::vec2(1440.0, 900.0)] {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.simple.enabled = true;
        app.simple.sync_style(&context);
        let mut imagery = Imagery::default();
        let mut clicked = false;
        let mut render = |events| {
            context.run(raw(size, events), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    clicked = empty_state(
                        ui,
                        &mut imagery,
                        EmptyArt::Glyph("console"),
                        "Nothing here yet",
                        "Choose a folder to browse your existing files.",
                        Some("Add game folder"),
                    );
                    action_button(ui, "Reference action", ActionStyle::Primary, true);
                });
            })
        };
        render(vec![]);
        let output = render(vec![]);
        // Shared Blue controls have identical corner radius and compact height;
        // compare their painted primary rectangles, not implementation constants.
        let mut buttons = vec![];
        fn collect(s: &egui::Shape, out: &mut Vec<(f32, egui::CornerRadius)>) {
            match s {
                egui::Shape::Rect(r) if r.fill == blue::PRIMARY_ACTION => {
                    out.push((r.rect.height(), r.corner_radius))
                }
                egui::Shape::Vec(v) => {
                    for s in v {
                        collect(s, out);
                    }
                }
                _ => {}
            }
        }
        for s in &output.shapes {
            collect(&s.shape, &mut buttons);
        }
        assert_eq!(buttons.len(), 2);
        assert_eq!(
            buttons[0], buttons[1],
            "Empty states should use the shared Blue primary button"
        );
        render(vec![key(egui::Key::Tab)]);
        render(vec![key(egui::Key::Enter)]);
        assert!(
            clicked,
            "The primary empty-state action must remain keyboard reachable"
        );
    }
}

// Optional evidence export uses egui's actual tessellated geometry and font
// atlas. It never opens a native window or reads application settings.
fn capture(context: &egui::Context, output: &egui::FullOutput, name: &str) {
    let Some(root) = std::env::var_os("EMUWIZ_POLISH_CAPTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).unwrap();
    let textures: Vec<_> = output
        .textures_delta
        .set
        .iter()
        .map(|(id, delta)| {
            let egui::ImageData::Color(image) = &delta.image;
            serde_json::json!({"id":format!("{id:?}"), "size":image.size,
            "pos":delta.pos, "pixels":image.pixels.iter().map(|p|p.to_array()).collect::<Vec<_>>()})
        })
        .collect();
    let meshes: Vec<_> = context.tessellate(output.shapes.clone(), output.pixels_per_point)
        .into_iter().filter_map(|p| {
            let egui::epaint::Primitive::Mesh(m) = p.primitive else { return None; };
            Some(serde_json::json!({"clip":[p.clip_rect.min.x,p.clip_rect.min.y,p.clip_rect.max.x,p.clip_rect.max.y],
                "texture":format!("{:?}",m.texture_id), "indices":m.indices,
                "vertices":m.vertices.iter().map(|v|serde_json::json!([v.pos.x,v.pos.y,v.uv.x,v.uv.y,v.color.to_array()])).collect::<Vec<_>>()}))
        }).collect();
    let file = root.join(format!("{name}.json"));
    std::fs::write(
        file,
        serde_json::to_vec(&serde_json::json!({"textures":textures,"meshes":meshes})).unwrap(),
    )
    .unwrap();
}
