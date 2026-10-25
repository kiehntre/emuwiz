//! Blue usability repairs from the October 20 hands-on smoke (D3-D7),
//! rendered through the real `App::show`. Presentation and navigation only:
//! nothing here may start work, change files or launch an emulator.
use super::*;
use crate::gui_v2::simple_shell::simple_breadcrumb_labels;

const DESKTOP: [f32; 2] = [1440.0, 900.0];

fn blue_app(context: &egui::Context, games: Vec<PersistedArchive>) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app.library = Arc::new(Library::new(games));
    app.loaded = true;
    app
}

fn shown(context: &egui::Context, app: &mut App, size: [f32; 2]) -> Vec<String> {
    frame(context, app, size);
    text(&frame(context, app, size))
}

fn input(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            events,
            ..Default::default()
        },
        |context| app.show(context),
    )
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

fn click(context: &egui::Context, app: &mut App, size: [f32; 2], point: egui::Pos2) {
    for pressed in [true, false] {
        input(
            context,
            app,
            size,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    frame(context, app, size);
}

fn top_of(output: &egui::FullOutput, label: &str) -> Option<f32> {
    text_bounds(output, label).first().map(|rect| rect.min.y)
}

// ------------------------------------------------------------- D7 trail

#[test]
fn simple_breadcrumbs_start_with_the_highlighted_sidebar_row() {
    let crumbs = |route: &Route, title: Option<&str>| simple_breadcrumb_labels(route, title);
    assert_eq!(
        crumbs(&Route::Section(Section::Setup), None),
        ["Setup", "Setup & Doctor"]
    );
    assert_eq!(
        crumbs(
            &Route::Task {
                section: Section::Launch,
                game: 7
            },
            Some("Pac-Man")
        ),
        ["Library", "Launch", "Pac-Man"]
    );
    assert_eq!(
        crumbs(&Route::ReviewIdentity(7), Some("Pac-Man")),
        ["Library", "Pac-Man", "Review identity"]
    );
    // A page whose own name is not a family parent keeps its trail.
    assert_eq!(
        crumbs(&Route::BrowsePlayGame(7), Some("Pac-Man")),
        ["Browse & Play", "Pac-Man"]
    );
    // Every simple trail starts with the row the sidebar highlights.
    for route in [
        Route::Section(Section::Setup),
        Route::Section(Section::Romm),
        Route::Section(Section::Dat),
        Route::Section(Section::History),
        Route::Section(Section::Converter),
        Route::Section(Section::Emulators),
        Route::Game(7),
        Route::MameWorkflow,
    ] {
        let labels = crumbs(&route, Some("Pac-Man"));
        let destination = crate::gui_v2::simple_shell::destination_for(&route)
            .expect("highlighted row")
            .label();
        assert_eq!(labels[0], destination, "{route:?}: {labels:?}");
    }
    // The Classic trail is unchanged.
    assert_eq!(
        crate::gui_v2::routes::breadcrumb_labels(&Route::Section(Section::Setup), None)[0],
        "Advanced / Diagnostics"
    );
}

#[test]
fn the_setup_page_trail_reads_setup_in_the_blue_view() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, Vec::new());
    app.router.go(Route::Section(Section::Setup));
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(!strings.iter().any(|s| s == "Advanced / Diagnostics"));
    assert!(strings.iter().any(|s| s == "Setup & Doctor"));
}

// ------------------------------------------------------- D7 Home tiles

#[test]
fn home_tiles_show_every_explanation_whole_at_small_window_sizes() {
    for size in [[1440.0, 900.0], [1280.0, 800.0]] {
        let context = egui::Context::default();
        let mut app = blue_app(&context, Vec::new());
        app.router.current = Route::Home;
        frame(&context, &mut app, size);
        let output = frame(&context, &mut app, size);
        for tile in &crate::gui_v2::home_blue::HOME_TILES {
            let galley = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == tile.explanation => {
                        Some((text.pos, text.galley.clone()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{size:?}: {} not shown whole", tile.title));
            assert!(!galley.1.elided, "{size:?}: {} elided", tile.title);
            // Painted inside the window, not cut off by the tile's clip.
            assert!(galley.0.y + galley.1.size().y < size[1] * 2.0);
        }
    }
}

// ------------------------------------------------------------ D4 MAME

#[test]
fn a_restored_mame_route_never_paints_the_generic_organise_hub() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, Vec::new());
    // Restored from preferences: `go` never ran, the view is the default.
    app.router.current = Route::MameWorkflow;
    app.organisation.view = crate::gui_v2::organisation::OrganisationView::Landing;
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(
        strings.iter().any(|s| s == "Fix my MAME library"),
        "{strings:?}"
    );
    assert!(
        !strings
            .iter()
            .any(|s| s == "Choose what you want to organise")
    );
    assert!(!strings.iter().any(|s| s == "Preview Playing Library"));
    // Back reads MAME and leads to the MAME hub, not Organise.
    assert!(strings.iter().any(|s| s == "‹ MAME"));
    assert!(!strings.iter().any(|s| s == "‹ Organisation"));
    let output = frame(&context, &mut app, DESKTOP);
    let back = text_bounds(&output, "‹ MAME")[0].center();
    click(&context, &mut app, DESKTOP, back);
    assert_eq!(app.router.current, Route::Section(Section::Mame));
}

#[test]
fn the_organise_hub_keeps_its_organisation_back_label() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, Vec::new());
    app.router.current = Route::Section(Section::Build);
    app.organisation.view = crate::gui_v2::organisation::OrganisationView::MameNormalizer;
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(strings.iter().any(|s| s == "‹ Organisation"));
    assert!(!strings.iter().any(|s| s == "‹ MAME"));
}

// ------------------------------------------------------- D5 PageDown

/// The DAT page's always-visible introduction, and a card that starts below
/// the fold of a 600-pixel window. Text outside the viewport is not painted,
/// so the card is only found once the page has scrolled.
const DAT_PAGE_INTRO: &str =
    "Manage the trusted game identification data EmuWiz uses to check releases.";
const DAT_PAGE_BELOW_FOLD: &str = "Saved catalogue inventory";

/// A long page outside Check (DATs & Verification, the smoke's D5 page) pages
/// with PageDown and PageUp, even while a button has keyboard focus.
#[test]
fn page_down_and_page_up_scroll_ordinary_pages() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, Vec::new());
    app.router.current = Route::Section(Section::Dat);
    let size = [1024.0, 600.0];
    frame(&context, &mut app, size);
    let start = frame(&context, &mut app, size);
    let before = top_of(&start, DAT_PAGE_INTRO).expect("DAT page content");
    assert_eq!(
        top_of(&start, DAT_PAGE_BELOW_FOLD),
        None,
        "starts below the fold"
    );
    input(&context, &mut app, size, vec![key(egui::Key::PageDown)]);
    let paged = frame(&context, &mut app, size);
    assert_ne!(
        top_of(&paged, DAT_PAGE_INTRO),
        Some(before),
        "PageDown scrolled"
    );
    assert!(
        top_of(&paged, DAT_PAGE_BELOW_FOLD).is_some(),
        "PageDown revealed content below the fold"
    );
    input(&context, &mut app, size, vec![key(egui::Key::PageUp)]);
    let back = frame(&context, &mut app, size);
    assert_eq!(
        top_of(&back, DAT_PAGE_INTRO),
        Some(before),
        "PageUp returned"
    );
    assert_eq!(top_of(&back, DAT_PAGE_BELOW_FOLD), None);
    // Button focus (keyboard users) must not disable paging.
    input(&context, &mut app, size, vec![key(egui::Key::Tab)]);
    assert!(!context.text_edit_focused());
    input(&context, &mut app, size, vec![key(egui::Key::PageDown)]);
    let focused_paged = frame(&context, &mut app, size);
    assert_ne!(top_of(&focused_paged, DAT_PAGE_INTRO), Some(before));
    assert!(top_of(&focused_paged, DAT_PAGE_BELOW_FOLD).is_some());
}

/// A focused text field keeps PageUp/PageDown for itself.
#[test]
fn page_keys_are_left_to_a_focused_text_field() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, Vec::new());
    app.router.current = Route::Section(Section::Dat);
    let size = [1024.0, 600.0];
    frame(&context, &mut app, size);
    let start = frame(&context, &mut app, size);
    let before = top_of(&start, DAT_PAGE_INTRO).expect("DAT page content");
    let search = text_bounds(&start, "Search your games or systems…")[0].center();
    click(&context, &mut app, size, search);
    assert!(context.text_edit_focused(), "the header search is focused");
    input(&context, &mut app, size, vec![key(egui::Key::PageDown)]);
    let after = frame(&context, &mut app, size);
    assert_eq!(top_of(&after, DAT_PAGE_INTRO), Some(before));
    assert_eq!(
        top_of(&after, DAT_PAGE_BELOW_FOLD),
        None,
        "the page did not scroll"
    );
}

// ------------------------------------------------- D3 Review identity

/// Play on an unverified game leads on to the existing Review identity route
/// instead of a dead end; other readiness states keep their own actions.
/// (The full Play journey on a scanned profile is exercised under Xvfb.)
#[test]
fn only_unresolved_identity_offers_review_identity_on_launch() {
    use crate::gui_v2::native_workflows::offers_review_identity;
    use crate::launch_readiness_page::LaunchReadinessInput;
    assert!(offers_review_identity(
        &LaunchReadinessInput::IdentityUnknown
    ));
    assert!(offers_review_identity(
        &LaunchReadinessInput::IdentityConflicting
    ));
    assert!(!offers_review_identity(
        &LaunchReadinessInput::EvidenceNotLoaded
    ));
    assert!(!offers_review_identity(
        &LaunchReadinessInput::RetroArchNotScanned
    ));
}
