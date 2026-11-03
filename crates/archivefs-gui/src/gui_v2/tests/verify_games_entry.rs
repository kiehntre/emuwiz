//! The Library's "Verify Games" entry point and Game Details' per-game
//! Verify / Re-check action. Navigation only: both open the existing read-only
//! verification pages, which own catalogue choice, Preview and confirmation.
//! Nothing here reads a game file, a catalogue or the real library.
use super::*;
use crate::gui_v2::browse_play::VERIFY_GAMES;
use crate::gui_v2::game_dat_check::{self, MISSING_CATALOGUE_HELP, Stage, Target};

const DESKTOP: [f32; 2] = [1672.0, 941.0];

fn blue_app(context: &egui::Context, games: Vec<PersistedArchive>) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app.library = Arc::new(Library::new(games));
    app.loaded = true;
    app
}

fn games() -> Vec<PersistedArchive> {
    vec![
        archive(1, "Alpha", Some("SNES")),
        archive(2, "Bravo", Some("SNES")),
        archive(3, "Charlie", Some("Mega Drive")),
    ]
}

fn click_text(context: &egui::Context, app: &mut App, size: [f32; 2], label: &str) {
    context.style_mut(|style| style.animation_time = 0.0);
    frame(context, app, size);
    let layout = frame(context, app, size);
    let point = text_bounds(&layout, label)
        .into_iter()
        .find(|rect| rect.min.y > 64.0)
        .unwrap_or_else(|| panic!("{label} not on screen: {:?}", text(&layout)))
        .center();
    for pressed in [true, false] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                events: vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |context| app.show(context),
        );
    }
    frame(context, app, size);
}

fn shown(context: &egui::Context, app: &mut App, size: [f32; 2]) -> Vec<String> {
    frame(context, app, size);
    text(&frame(context, app, size))
}

#[test]
fn library_verify_games_keeps_the_chosen_system() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::BrowsePlay;
    app.filter.select_platform("SNES".into());
    click_text(&context, &mut app, DESKTOP, VERIFY_GAMES);
    assert_eq!(app.router.current, Route::Section(Section::Check));
    assert_eq!(app.check_platform.as_deref(), Some("SNES"));
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(
        strings.iter().any(|line| line == "SNES games"),
        "{strings:?}"
    );
    // The existing tally, from saved results only: nothing is checked yet,
    // so nothing is called verified.
    assert!(
        strings.iter().any(|line| line == "0 verified"),
        "{strings:?}"
    );
    assert!(
        app.verification_job.is_none() && app.verification.is_none(),
        "opening Check Games starts nothing"
    );
}

#[test]
fn library_verify_games_without_a_system_opens_the_platform_choice() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::BrowsePlay;
    // A system chosen earlier on Check Games must not leak into "All systems".
    app.check_platform = Some("Mega Drive".into());
    click_text(&context, &mut app, DESKTOP, VERIFY_GAMES);
    assert_eq!(app.router.current, Route::Section(Section::Check));
    assert_eq!(app.check_platform, None);
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(
        strings.iter().any(|line| line == "Choose a platform"),
        "{strings:?}"
    );
    assert!(strings.iter().any(|line| line == "Check SNES"));
    assert!(app.verification_job.is_none());
}

#[test]
fn game_details_verify_this_game_opens_the_one_game_check() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::Game(1);
    click_text(&context, &mut app, [1280.0, 1200.0], "Verify this game");
    assert_eq!(app.router.current, Route::GameCheck(1));
}

#[test]
fn a_saved_result_is_offered_as_a_re_check_and_never_shown_as_verified() {
    use super::super::game_verification_guidance::Action;
    let mut no_match = Library::new(games()).game(1).unwrap().clone();
    no_match.dat_state = Some(archivefs_core::LibraryDatState::NoMatch);
    assert_eq!(Action::one_game_check(&no_match), Action::Recheck);
    assert_eq!(
        Action::one_game_check(&no_match).label(),
        "Re-check this game"
    );
    assert_eq!(
        Action::one_game_check(&no_match).route(&no_match),
        Route::GameCheck(1)
    );
    let unchecked = Library::new(games()).game(2).unwrap().clone();
    assert_eq!(
        Action::one_game_check(&unchecked).label(),
        "Verify this game"
    );
}

#[test]
fn a_game_without_a_known_system_goes_to_check_games_first() {
    use super::super::game_verification_guidance::Action;
    let library = Library::new(vec![archive(9, "Mystery", None)]);
    let game = library.game(9).unwrap();
    assert_eq!(
        Action::one_game_check(game).route(game),
        Route::Section(Section::Check)
    );
}

#[test]
fn a_missing_catalogue_is_explained_and_links_dat_management() {
    let library = Library::new(games());
    let game = library.game(1).unwrap();
    let mut state = game_dat_check::State::default();
    state.target = Some(Target::of(game));
    state.stage = Stage::Choose;
    let mut action = None;
    let painted = crate::shared_apply_warnings::test_support::render(|ui| {
        action = game_dat_check::show(ui, &mut state, 1);
    });
    for wanted in [
        "No ready catalogue covers SNES. Set one up first; nothing has been checked.",
        MISSING_CATALOGUE_HELP,
        "Set up verification",
    ] {
        assert!(painted.contains(wanted), "{wanted}: {painted}");
    }
    assert!(!painted.contains("Verified"), "{painted}");
    assert!(matches!(action, None));
}
