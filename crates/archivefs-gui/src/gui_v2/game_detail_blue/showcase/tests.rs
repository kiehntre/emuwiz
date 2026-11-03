//! Artwork showcase on Game Details: synthetic data only, through the real
//! `App::show`. No picture is fetched; ready pictures are inserted as already
//! decoded textures, and every other state is the loader's own result.
use super::*;
use crate::gui_v2::{
    Route,
    artwork::Picture,
    library::{Detail, Library},
    media_sources::{Kind, MediaIndex, Source},
    tests::{archive, fixture, frame, text},
    thumbnail::Timings,
};
use archivefs_core::identity_source::model::ExternalIdentityRecord;
use std::sync::Arc;

// Tall, so the Screenshots section below the hero is on screen.
const DESKTOP: [f32; 2] = [1672.0, 2400.0];
const GAME: i64 = 7;
const BROKEN: &str = "The picture is broken, unsupported, or too large to decode safely.";

fn record(verification: &str) -> Arc<ExternalIdentityRecord> {
    Arc::new(
        serde_json::from_value(serde_json::json!({
            "provider":"romm", "server_id":"fixture", "provider_game_id":"7",
            "provider_path":"/game", "regions":[], "hashes":[],
            "metadata_provider_ids":[], "related_files":[], "sibling_game_ids":[],
            "imported_at_unix_seconds":1, "verification":verification,
            "conflicts":[], "evidence":[],
            "artwork":{"reference":"/cover.png", "small_reference":"/cover.png"}
        }))
        .unwrap(),
    )
}

fn local(name: &str) -> Source {
    Source::Local(format!("/fixture/media/{name}.png").into())
}

fn ready(context: &egui::Context) -> Picture {
    Picture::Ready {
        texture: context.load_texture(
            "showcase",
            egui::ColorImage::filled([8, 6], egui::Color32::GREEN),
            Default::default(),
        ),
        timings: Timings::default(),
    }
}

fn app(context: &egui::Context, blue: bool, index: MediaIndex) -> App {
    let mut app = fixture(context);
    app.simple.enabled = blue;
    app.library = Arc::new(Library::new(vec![archive(GAME, "Fixture", Some("PSX"))]));
    app.loaded = true;
    app.artwork.index = Some(Arc::new(index));
    app.router.current = Route::Game(GAME);
    app.detail = Some(Detail {
        game: GAME,
        file_present: true,
        unchanged: true,
        ..Detail::default()
    });
    app
}

fn put(app: &mut App, kind: Kind, picture: Picture) {
    let key = app.artwork.key(GAME, kind);
    app.artwork.pictures.insert(key, picture);
}

fn settled(context: &egui::Context, app: &mut App, size: [f32; 2]) -> Vec<String> {
    for _ in 0..3 {
        frame(context, app, size);
    }
    text(&frame(context, app, size))
}

fn has(strings: &[String], wanted: &str) -> bool {
    strings.iter().any(|value| value.contains(wanted))
}

/// Both Game Details layouts: Blue side by side, and the classic column.
fn layouts() -> [(bool, [f32; 2]); 2] {
    [(true, DESKTOP), (false, [1000.0, 4000.0])]
}

// --- pure classification ----------------------------------------------

#[test]
fn state_reports_each_stored_reference_honestly() {
    let context = egui::Context::default();
    let mut index = MediaIndex::default();
    index.covers.insert(1, local("one"));
    index.covers.insert(
        2,
        Source::Remote {
            record: record("stale"),
            kind: Kind::Cover,
        },
    );
    index.covers.insert(
        3,
        Source::Remote {
            record: record("strong_external"),
            kind: Kind::Cover,
        },
    );
    let shown = ready(&context);
    assert_eq!(
        state(None, 1, Kind::Cover, None, false),
        ArtworkState::Checking
    );
    assert_eq!(
        state(Some(&index), 9, Kind::Cover, None, false),
        ArtworkState::NoneStored
    );
    assert_eq!(
        state(Some(&index), 1, Kind::Cover, Some(&shown), false),
        ArtworkState::Shown {
            origin: Origin::LocalFile,
            stale: false
        }
    );
    assert_eq!(
        state(Some(&index), 2, Kind::Cover, Some(&shown), false),
        ArtworkState::Shown {
            origin: Origin::SavedRomm,
            stale: true
        }
    );
    assert_eq!(
        state(Some(&index), 3, Kind::Cover, Some(&shown), false),
        ArtworkState::Shown {
            origin: Origin::SavedRomm,
            stale: false
        }
    );
    assert_eq!(
        state(
            Some(&index),
            1,
            Kind::Cover,
            Some(&Picture::Failed(BROKEN.into())),
            false
        ),
        ArtworkState::Unreadable {
            origin: Origin::LocalFile,
            stale: false,
            reason: BROKEN.into()
        }
    );
    assert_eq!(
        state(Some(&index), 1, Kind::Cover, None, true),
        ArtworkState::Paused
    );
}

#[test]
fn captions_never_present_stale_or_broken_artwork_as_good() {
    let stale = caption(
        "cover",
        &ArtworkState::Shown {
            origin: Origin::SavedRomm,
            stale: true,
        },
    )
    .unwrap();
    assert_eq!(stale.tone, Tone::Warning);
    assert!(stale.headline.contains("stale"));
    // Still stale while it loads: the warning does not wait for the picture.
    let loading = caption(
        "cover",
        &ArtworkState::Loading {
            origin: Origin::SavedRomm,
            stale: true,
        },
    )
    .unwrap();
    assert_eq!(loading, stale);
    let broken = caption(
        "cover",
        &ArtworkState::Unreadable {
            origin: Origin::SavedRomm,
            stale: true,
            reason: BROKEN.into(),
        },
    )
    .unwrap();
    assert_eq!(broken.tone, Tone::Warning);
    let detail = broken.detail.unwrap();
    assert!(
        detail.contains(BROKEN) && detail.contains("stale"),
        "{detail}"
    );
    for state in [ArtworkState::Checking, ArtworkState::Paused] {
        assert_eq!(caption("cover", &state), None);
    }
    for state in [
        ArtworkState::NoneStored,
        ArtworkState::Shown {
            origin: Origin::LocalFile,
            stale: false,
        },
        ArtworkState::Shown {
            origin: Origin::SavedRomm,
            stale: false,
        },
    ] {
        let caption = caption("cover", &state).unwrap();
        assert_eq!(caption.tone, Tone::Plain);
        assert!(
            !caption.headline.to_lowercase().contains("verified"),
            "{caption:?}"
        );
    }
}

#[test]
fn preview_stays_four_by_three_and_bounded() {
    assert_eq!(preview_size(2000.0), egui::vec2(640.0, 480.0));
    assert_eq!(preview_size(400.0), egui::vec2(400.0, 300.0));
    assert_eq!(preview_size(50.0), egui::vec2(200.0, 150.0));
}

// --- Game Details through App::show --------------------------------------

#[test]
fn present_cover_and_screenshot_name_their_saved_source() {
    for (blue, size) in layouts() {
        let context = egui::Context::default();
        let mut index = MediaIndex::default();
        index.covers.insert(GAME, local("cover"));
        index.screenshots.insert(GAME, vec![local("shot")]);
        let mut app = app(&context, blue, index);
        put(&mut app, Kind::Cover, ready(&context));
        put(&mut app, Kind::Screenshot(0), ready(&context));
        let strings = settled(&context, &mut app, size);
        assert!(
            has(&strings, "From a saved image on this computer"),
            "blue={blue}: {strings:?}"
        );
        assert!(has(&strings, "Screenshot 1 of 1"), "blue={blue}");
        assert!(!has(&strings, "No cover saved"), "blue={blue}");
        assert!(!has(&strings, "can't be shown"), "blue={blue}");
    }
}

#[test]
fn missing_artwork_says_nothing_was_saved_and_draws_no_fake_cover() {
    for (blue, size) in layouts() {
        let context = egui::Context::default();
        let mut app = app(&context, blue, MediaIndex::default());
        let strings = settled(&context, &mut app, size);
        assert!(
            has(&strings, "No cover saved for this game"),
            "blue={blue}: {strings:?}"
        );
        assert!(
            has(&strings, "No screenshots found for this game."),
            "blue={blue}"
        );
        assert!(
            has(&strings, "No picture yet"),
            "blue={blue}: the placeholder tile stays"
        );
        let key = app.artwork.key(GAME, Kind::Cover);
        assert!(matches!(
            app.artwork.pictures.get(&key),
            Some(Picture::Missing)
        ));
    }
}

#[test]
fn stale_romm_artwork_keeps_its_warning_and_is_never_called_current() {
    for (blue, size) in layouts() {
        let context = egui::Context::default();
        let mut index = MediaIndex::default();
        index.covers.insert(
            GAME,
            Source::Remote {
                record: record("stale"),
                kind: Kind::Cover,
            },
        );
        let mut app = app(&context, blue, index);
        put(&mut app, Kind::Cover, ready(&context));
        let strings = settled(&context, &mut app, size);
        assert!(
            has(&strings, "Saved RomM artwork · record is stale"),
            "blue={blue}: {strings:?}"
        );
        assert!(
            has(&strings, "Saved artwork · stale record"),
            "blue={blue}: tile badge kept"
        );
        assert!(!has(&strings, "From the saved RomM record"), "blue={blue}");
    }
}

#[test]
fn malformed_or_vanished_saved_files_are_named_with_a_retry() {
    for reason in [BROKEN, "The picture is no longer available."] {
        for (blue, size) in layouts() {
            let context = egui::Context::default();
            let mut index = MediaIndex::default();
            index.covers.insert(GAME, local("cover"));
            index.screenshots.insert(GAME, vec![local("shot")]);
            let mut app = app(&context, blue, index);
            put(&mut app, Kind::Cover, Picture::Failed(reason.into()));
            put(
                &mut app,
                Kind::Screenshot(0),
                Picture::Failed(reason.into()),
            );
            let strings = settled(&context, &mut app, size);
            assert!(
                has(&strings, "The saved cover file can't be shown"),
                "blue={blue}: {strings:?}"
            );
            assert!(
                has(&strings, "The saved screenshot file can't be shown"),
                "blue={blue}"
            );
            assert!(
                has(&strings, reason),
                "blue={blue}: the loader's reason is shown"
            );
            assert!(has(&strings, "Retry picture"), "blue={blue}");
            assert!(has(&strings, "Retry screenshot 1"), "blue={blue}");
            assert!(
                !has(&strings, "No cover saved"),
                "blue={blue}: a broken file is not 'none'"
            );
        }
    }
}

#[test]
fn artwork_does_not_hide_a_missing_game_file_or_promote_a_stale_dat_match() {
    let context = egui::Context::default();
    let mut index = MediaIndex::default();
    index.covers.insert(GAME, local("cover"));
    let mut app = app(&context, true, index);
    put(&mut app, Kind::Cover, ready(&context));
    app.detail = Some(Detail {
        game: GAME,
        file_present: false,
        unchanged: false,
        dat_region: Some(("Europe".into(), "Redump PSX".into())),
        ..Detail::default()
    });
    let strings = settled(&context, &mut app, DESKTOP);
    assert!(has(&strings, "From a saved image on this computer"));
    assert!(has(&strings, "Not found"), "{strings:?}");
    assert!(has(&strings, "The file was moved or removed"));
    assert!(
        has(&strings, "Needs a recheck."),
        "a saved DAT match stays unconfirmed"
    );
    assert!(!has(&strings, "From the verified match"));
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

fn run(context: &egui::Context, app: &mut App, events: Vec<egui::Event>) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, DESKTOP.into())),
            events,
            ..Default::default()
        },
        |context| app.show(context),
    )
}

#[test]
fn screenshots_load_one_until_asked_then_step_by_keyboard() {
    let context = egui::Context::default();
    let mut index = MediaIndex::default();
    index
        .screenshots
        .insert(GAME, vec![local("a"), local("b"), local("c")]);
    let mut app = app(&context, true, index);
    for ordinal in 0..3 {
        put(&mut app, Kind::Screenshot(ordinal), ready(&context));
    }
    let strings = settled(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Screenshot 1 of 3"), "{strings:?}");
    assert!(has(&strings, "Show all screenshots (3)"));
    assert!(
        !has(&strings, "Next screenshot"),
        "thumbnails wait until asked for"
    );

    app.screenshots = true;
    let strings = settled(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Screenshot 3"), "{strings:?}");
    assert!(has(&strings, "Next screenshot"));

    // Tab reaches the Next button; Enter moves the preview on.
    let mut reached = false;
    for _ in 0..200 {
        let output = run(&context, &mut app, vec![key(egui::Key::Tab)]);
        if output.platform_output.events.iter().any(|event| {
            matches!(event, egui::output::OutputEvent::FocusGained(info)
                if info.label.as_deref() == Some("Next screenshot"))
        }) {
            reached = true;
            break;
        }
    }
    assert!(reached, "Next screenshot must be reachable with Tab");
    run(&context, &mut app, vec![key(egui::Key::Enter)]);
    let strings = settled(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Screenshot 2 of 3"), "{strings:?}");
}
