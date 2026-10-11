//! Slice 2B layout matrix: a populated library rendered through the real
//! `App::show` in Blue and Classic, at every saved scale and window size. The
//! probe measures what is actually painted - clipped text, overlapping
//! controls and reachable actions - rather than asserting on private layout.
use super::*;

const SCALES: [f32; 4] = [0.8, 1.0, 1.3, 1.6];
const WINDOWS: [(f32, f32); 3] = [(1024.0, 640.0), (1280.0, 720.0), (1920.0, 1080.0)];
const LONG_TITLE: &str = "The Extraordinarily Long Adventures of a Game With A Title That Keeps \
    Going Well Beyond Any Reasonable Width: Special Collector's Edition Remastered (Europe) (Rev 2)";

fn populated(context: &egui::Context, blue: bool, scale: f32) -> App {
    let mut app = fixture(context);
    let mut games = vec![archive(1, LONG_TITLE, Some("Sony PlayStation 2"))];
    let systems = [
        "SNES",
        "Mega Drive",
        "Dreamcast",
        "Arcade",
        "Game Boy Advance",
    ];
    for index in 2..=60_i64 {
        games.push(archive(
            index,
            &format!("Fixture Game {index:02}"),
            Some(systems[index as usize % systems.len()]),
        ));
    }
    app.library = Arc::new(Library::new(games));
    app.simple.enabled = blue;
    app.simple.set_ui_scale(scale);
    app
}

fn points(window: (f32, f32), scale: f32) -> [f32; 2] {
    [window.0 / scale, window.1 / scale]
}

/// Text shapes painted wider than the region they are clipped to, or off the
/// screen: the "horizontal clipping" the slice is about.
fn clipped_text(output: &egui::FullOutput, size: [f32; 2]) -> Vec<String> {
    fn gather(shape: &egui::Shape, clip: egui::Rect, size: [f32; 2], found: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => {
                let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                let limit = clip.intersect(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                ));
                if rect.width() > 1.0
                    && (rect.max.x > limit.max.x + 1.0 || rect.min.x < limit.min.x - 1.0)
                {
                    found.push(format!("{:?}", text.galley.text()));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    gather(shape, clip, size, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        gather(&clipped.shape, clipped.clip_rect, size, &mut found);
    }
    found
}

fn settled(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    for _ in 0..3 {
        frame(context, app, size);
    }
    frame(context, app, size)
}

fn visible(rect: egui::Rect, size: [f32; 2]) -> bool {
    rect.min.x >= -0.5
        && rect.min.y >= -0.5
        && rect.max.x <= size[0] + 0.5
        && rect.max.y <= size[1] + 0.5
}

fn role_size(context: &egui::Context, role: crate::ui::theme::TextRole) -> f32 {
    context.style().text_styles[&role.text_style()].size
}

#[test]
fn typography_roles_are_ordered_in_blue_and_classic_keeps_its_sizes() {
    use crate::ui::theme::{self, TextRole};
    for blue in [true, false] {
        let context = egui::Context::default();
        let mut app = populated(&context, blue, 1.0);
        app.router.current = Route::Home;
        settled(&context, &mut app, [1280.0, 720.0]);
        let page = role_size(&context, TextRole::PageTitle);
        let section = role_size(&context, TextRole::SectionTitle);
        let card = role_size(&context, TextRole::CardTitle);
        let body = role_size(&context, TextRole::Body);
        assert_eq!(
            page,
            context.style().text_styles[&egui::TextStyle::Heading].size
        );
        assert!(page > section && section > body, "{page} {section} {body}");
        if blue {
            assert!(
                section > card && card > body,
                "{page} {section} {card} {body}"
            );
        } else {
            // Classic is unchanged at 100%: headings 27, section titles 19.
            assert_eq!(
                (page, section, card),
                (
                    theme::PAGE_TITLE_SIZE,
                    theme::SECTION_TITLE_SIZE,
                    theme::PAGE_TITLE_SIZE
                )
            );
        }
    }
}

#[test]
fn slice2b_matrix_has_no_clipping_overlap_or_hidden_play() {
    let pages: [(&str, Route); 5] = [
        ("home", Route::Home),
        ("platforms", Route::Section(Section::Platforms)),
        ("library", Route::BrowsePlayGame(1)),
        ("game", Route::Game(1)),
        ("settings", Route::Section(Section::Settings)),
    ];
    let mut failures = Vec::new();
    for blue in [true, false] {
        for scale in SCALES {
            for window in WINDOWS {
                let size = points(window, scale);
                for (name, route) in &pages {
                    let context = egui::Context::default();
                    let mut app = populated(&context, blue, scale);
                    app.router.current = route.clone();
                    let output = settled(&context, &mut app, size);
                    let cell = format!("blue={blue} {scale} {window:?} {name}");
                    let clipped = clipped_text(&output, size);
                    if !clipped.is_empty() {
                        failures.push(format!("{cell}: clipped {clipped:?}"));
                    }
                    if *name == "library" {
                        let search = text_bounds(&output, "Search by title or system");
                        let grid = text_bounds(&output, "Grid");
                        let list = text_bounds(&output, "List");
                        // Search shares a row with the switch only when it fits;
                        // either way their rectangles never intersect.
                        for switch in grid.iter().chain(list.iter()) {
                            for field in &search {
                                if field.intersects(*switch) {
                                    failures.push(format!("{cell}: search overlaps {switch:?}"));
                                }
                            }
                        }
                        let primary = scale == 1.0 && window == (1280.0, 720.0);
                        if primary
                            && !text_bounds(&output, "Play")
                                .iter()
                                .any(|r| visible(*r, size))
                        {
                            failures.push(format!("{cell}: Play not visible"));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
