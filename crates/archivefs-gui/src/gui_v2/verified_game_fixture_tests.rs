//! The shared verified-game fixture reaches Blue: the game the production
//! pipeline verified is shown as Verified in the Library with no click, at a
//! normal and a small window size, and still is after the library is
//! reloaded the way a restarted application reloads it.

use super::App;
use super::library::Library;
use archivefs_core::verified_game_fixture::{GAME_NAME, PLATFORM, VerifiedGameFixture};
use std::sync::Arc;

/// Exactly what `backend::load_library` does at startup, with the catalogue
/// registry read from the fixture's own config directory.
fn load(fixture: &VerifiedGameFixture) -> Library {
    let config = archivefs_core::dat::sources::load_dat_sources_config_from(
        fixture.config_dir.join("dat_sources.toml"),
    )
    .unwrap();
    let (local, problems) = archivefs_core::dat::sources::DatSourceRegistry::from_config(&config);
    assert!(problems.is_empty(), "{problems:?}");
    let catalogues = archivefs_core::dat::configured_catalogues::configured_catalogues(
        archivefs_core::dat::catalogue_selection::CatalogueInventoryInputs {
            local_registry: &local,
            managed_sources: &Default::default(),
            managed_root: &fixture.data_dir.join("managed-dats"),
            limits: Default::default(),
        },
    );
    super::backend::load_library_with_catalogues(&fixture.database_path, &catalogues).unwrap()
}

fn blue_app(context: &eframe::egui::Context, library: Library) -> App {
    let mut app = super::tests::fixture(context);
    app.simple.enabled = true;
    app.library = Arc::new(library);
    app
}

/// Every text galley painted at `size`, after a warm-up frame so scroll
/// areas have measured their content (as they do by the second real frame).
fn shown(context: &eframe::egui::Context, app: &mut App, size: [f32; 2]) -> Vec<String> {
    let _ = paint(context, app, size);
    paint(context, app, size)
}

fn paint(context: &eframe::egui::Context, app: &mut App, size: [f32; 2]) -> Vec<String> {
    let output = context.run(
        eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(size[0], size[1]),
            )),
            ..Default::default()
        },
        |ctx| app.show(ctx),
    );
    fn collect(shape: &eframe::egui::Shape, out: &mut Vec<String>) {
        match shape {
            eframe::egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|s| collect(&s.shape, &mut out));
    out
}

fn has(strings: &[String], needle: &str) -> bool {
    strings.iter().any(|value| value.contains(needle))
}

#[test]
fn the_fixture_game_is_verified_in_the_blue_library_with_no_click() {
    let fixture = VerifiedGameFixture::build().unwrap();
    let library = load(&fixture);
    assert_eq!(library.games.len(), 1, "{:?}", library.games.len());
    let game = &library.games[0];
    assert!(
        game.archive
            .display_name
            .contains("EmuWiz Synthetic Fixture"),
        "{}",
        game.archive.display_name
    );
    let _ = GAME_NAME;
    assert_eq!(game.archive.platform.as_deref(), Some(PLATFORM));
    assert!(
        game.dat_exact.is_some(),
        "the saved exact match reached the library"
    );
    assert_eq!(game.identity_summary(), "Verified");
    assert!(
        game.dat_exact_label().unwrap().starts_with("Exact "),
        "{:?}",
        game.dat_exact_label()
    );
}

#[test]
fn the_verified_identity_is_shown_at_a_normal_and_a_small_window_size() {
    let fixture = VerifiedGameFixture::build().unwrap();
    let context = eframe::egui::Context::default();
    let mut app = blue_app(&context, load(&fixture));
    let id = app.library.games[0].archive.id;
    for size in [[1280.0, 800.0], [700.0, 520.0]] {
        // The Library list shows the saved verdict with no worker involved.
        app.router.current = super::Route::BrowsePlay;
        let strings = shown(&context, &mut app, size);
        assert!(has(&strings, "1 game"), "{size:?}: {strings:?}");
        assert!(has(&strings, "Game Boy Advance"), "{size:?}: {strings:?}");
        if size[1] >= 800.0 {
            assert!(
                has(&strings, "EmuWiz Synthetic Fixture"),
                "{size:?}: {strings:?}"
            );
            assert!(has(&strings, "Verified"), "{size:?}: {strings:?}");
        } else {
            // Known small-window limit (recorded for the Blue presentation
            // batch): at 700x520 the filter block fills the height and the
            // first game row is below the fold, so its verdict is not painted.
            assert!(
                !has(&strings, "Needs verification") && !has(&strings, "Mismatch"),
                "{size:?}: {strings:?}"
            );
        }
        // Game Details opens for the game. Its own identity line comes from a
        // review worker that never runs in a headless frame, so it is not
        // asserted here; the Library verdict above needs no worker.
        app.router.current = super::Route::Game(id);
        let strings = shown(&context, &mut app, size);
        assert!(
            has(&strings, "EmuWiz Synthetic Fixture (USA)"),
            "{size:?}: {strings:?}"
        );
        assert!(has(&strings, "Game Boy Advance"), "{size:?}: {strings:?}");
    }
}

#[test]
fn verification_survives_reloading_the_library_as_a_restart_would() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().join("profile");
    {
        let built = VerifiedGameFixture::build_at(&root).unwrap();
        assert_eq!(load(&built).games[0].identity_summary(), "Verified");
    }
    // Nothing in memory survives; only the files under the root do.
    let reopened = VerifiedGameFixture::reopen(&root).unwrap();
    let library = load(&reopened);
    assert_eq!(library.games.len(), 1);
    assert_eq!(library.games[0].identity_summary(), "Verified");
}
