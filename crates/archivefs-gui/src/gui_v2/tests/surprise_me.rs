//! Surprise Me: deterministic picks from a disposable in-memory catalogue.
use super::*;
use crate::gui_v2::tests::{archive, fixture, frame, text};
use archivefs_core::PersistedArchive;
use std::collections::BTreeSet;
use std::sync::Arc;

const SEED: u64 = 0x5EED_2026_1102;

fn recording(id: i64) -> PersistedArchive {
    let mut row = archive(id, "Manic Miner", Some("ZX Spectrum"));
    row.relative_path = "Manic Miner.rzx".into();
    row.absolute_path = "/fixture/Manic Miner.rzx".into();
    row.archive_kind = "preservation_recording".into();
    row
}

/// 1 and 7 are ready to try. 2 is recorded missing, 3 has missing health,
/// 4 is a recording, 5 has no known system, 6 is ready to try until a
/// finished check says Not ready.
fn library() -> Library {
    let mut recorded_missing = archive(2, "Bravo", Some("SNES"));
    recorded_missing.last_verified_missing_at = Some("2026-11-01".into());
    let mut missing_health = archive(3, "Charlie", Some("SNES"));
    missing_health.last_known_health = "missing".into();
    Library::new(vec![
        archive(1, "Alpha", Some("SNES")),
        recorded_missing,
        missing_health,
        recording(4),
        archive(5, "Delta", None),
        archive(6, "Echo", Some("SNES")),
        archive(7, "Foxtrot", Some("Mega Drive")),
    ])
}

fn not_ready_six(library: &Library) -> ReadinessCache {
    let mut cache = ReadinessCache::default();
    cache.record(
        library.game(6).unwrap(),
        Some(CheckedReadiness::NotReady("Emulator not found")),
    );
    cache
}

fn ids(library: &Library, indices: &[usize]) -> BTreeSet<i64> {
    indices
        .iter()
        .map(|index| library.games[*index].archive.id)
        .collect()
}

fn filter(platform: Option<&str>, ready_to_try: bool) -> SurpriseFilter {
    SurpriseFilter {
        platform: platform.map(str::to_string),
        ready_to_try,
    }
}

#[test]
fn the_same_seed_gives_the_same_numbers_and_stays_in_range() {
    let mut first = SurpriseRng::seeded(SEED);
    let mut second = SurpriseRng::seeded(SEED);
    for _ in 0..100 {
        assert_eq!(first.next_u64(), second.next_u64());
    }
    let mut rng = SurpriseRng::seeded(SEED);
    let mut seen = BTreeSet::new();
    for _ in 0..1000 {
        let value = rng.below(5);
        assert!(value < 5);
        seen.insert(value);
    }
    assert_eq!(seen.len(), 5, "every value in range comes up");
}

#[test]
fn ready_to_try_leaves_out_missing_files_recordings_unknown_systems_and_not_ready() {
    let library = library();
    let cache = not_ready_six(&library);
    let pool = |platform, ready| {
        ids(
            &library,
            &candidates(&library, &filter(platform, ready), &cache),
        )
    };

    assert_eq!(pool(None, true), BTreeSet::from([1, 7]));
    assert_eq!(pool(Some("SNES"), true), BTreeSet::from([1]));
    assert_eq!(pool(Some("SNES"), false), BTreeSet::from([1, 2, 3, 6]));
    assert_eq!(pool(None, false), BTreeSet::from([1, 2, 3, 4, 5, 6, 7]));
    assert!(pool(Some("Neo Geo"), false).is_empty());

    // Without the finished Not ready answer, 6 is offered again.
    let fresh = ReadinessCache::default();
    assert_eq!(
        ids(
            &library,
            &candidates(&library, &filter(Some("SNES"), true), &fresh)
        ),
        BTreeSet::from([1, 6])
    );
}

#[test]
fn ready_to_try_never_picks_a_missing_file_over_many_rolls() {
    let library = library();
    let cache = not_ready_six(&library);
    let mut state = SurpriseState::with_seed(SEED);
    let mut seen = BTreeSet::new();
    for _ in 0..500 {
        let id = state
            .roll(&library, &cache)
            .expect("two games are ready to try");
        assert!(id == 1 || id == 7, "picked {id}");
        seen.insert(id);
    }
    assert_eq!(seen, BTreeSet::from([1, 7]));
}

#[test]
fn roll_again_changes_the_game_whenever_another_exists() {
    let library = library();
    let cache = ReadinessCache::default();
    let mut state = SurpriseState::with_seed(SEED);
    state.filter = filter(None, false);
    let mut previous = state.roll(&library, &cache).unwrap();
    for _ in 0..200 {
        let next = state.roll(&library, &cache).unwrap();
        assert_ne!(next, previous);
        previous = next;
    }

    // One candidate: Roll Again keeps showing it rather than nothing.
    let mega_drive = library.game(7).unwrap().platform.clone();
    state.filter = filter(Some(&mega_drive), true);
    assert_eq!(state.roll(&library, &cache), Some(7));
    assert_eq!(state.roll(&library, &cache), Some(7));
}

#[test]
fn a_seed_replays_the_same_picks() {
    let library = library();
    let cache = ReadinessCache::default();
    let picks = |seed| {
        let mut state = SurpriseState::with_seed(seed);
        state.filter = filter(None, false);
        (0..20)
            .map(|_| state.roll(&library, &cache).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(picks(SEED), picks(SEED));
    assert_ne!(picks(SEED), picks(SEED + 1));
}

#[test]
fn an_empty_result_is_reported_once_and_does_not_reroll_every_frame() {
    let library = Library::new(vec![{
        let mut row = archive(2, "Bravo", Some("SNES"));
        row.last_verified_missing_at = Some("2026-11-01".into());
        row
    }]);
    let cache = ReadinessCache::default();
    let mut state = SurpriseState::with_seed(SEED);
    assert_eq!(state.roll(&library, &cache), None);
    assert!(state.is_empty());
    state.refresh(&library, &cache);
    assert!(state.is_empty() && state.current.is_none());

    // Turning the filter off finds the game.
    state.filter.ready_to_try = false;
    state.filter_changed(&library, &cache);
    assert_eq!(state.current, Some(2));
    assert!(!state.is_empty());

    let empty = Library::default();
    let mut state = SurpriseState::with_seed(SEED);
    assert_eq!(state.roll(&empty, &cache), None);
}

#[test]
fn a_pick_that_leaves_the_catalogue_or_fails_its_check_is_replaced() {
    let library = library();
    let mut cache = ReadinessCache::default();
    let mut state = SurpriseState::with_seed(SEED);
    state.filter = filter(Some("SNES"), true);
    state.current = Some(6);
    assert!(state.current_game(&library, &cache).is_some());

    // A finished Not ready answer arrives for the game on show.
    cache.record(
        library.game(6).unwrap(),
        Some(CheckedReadiness::NotReady("Emulator not found")),
    );
    assert!(state.current_game(&library, &cache).is_none());
    state.refresh(&library, &cache);
    assert_eq!(state.current, Some(1));

    // A reload drops the game on show.
    let reloaded = Library::new(vec![archive(6, "Echo", Some("SNES"))]);
    let fresh = ReadinessCache::default();
    state.refresh(&reloaded, &fresh);
    assert_eq!(state.current, Some(6));
}

fn app(context: &egui::Context, library: Library) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app.library = Arc::new(library);
    app.loaded = true;
    app.router.current = Route::BrowsePlay;
    app.browse_play.surprise = SurpriseState::with_seed(SEED);
    app
}

#[test]
fn play_hands_the_pick_to_browse_and_play_and_starts_nothing() {
    let context = egui::Context::default();
    let mut app = app(&context, library());
    app.open_surprise_me();
    let id = app.browse_play.surprise.current.expect("a pick");
    assert!(id == 1 || id == 6 || id == 7);

    // A Play for a game no longer on show is ignored.
    let other = if id == 1 { 7 } else { 1 };
    app.apply_surprise_action(SurpriseAction::Play(other));
    assert_eq!(app.router.current, Route::BrowsePlay);
    assert!(app.browse_play.surprise.open);

    app.apply_surprise_action(SurpriseAction::Play(id));
    assert_eq!(app.router.current, Route::BrowsePlayGame(id));
    assert!(!app.browse_play.surprise.open);
    // Launch planning and starting live in the native workflows; Surprise Me
    // never creates them, so nothing was checked or started from here.
    assert!(
        app.native_workflows.is_none(),
        "Surprise Me never starts a game itself"
    );
}

#[test]
fn the_window_shows_the_pick_with_play_and_roll_again() {
    let context = egui::Context::default();
    let mut app = app(&context, library());
    let size = [1280.0, 800.0];
    frame(&context, &mut app, size);
    assert!(
        text(&frame(&context, &mut app, size))
            .iter()
            .any(|line| line == SURPRISE_BUTTON)
    );

    app.open_surprise_me();
    let id = app.browse_play.surprise.current.unwrap();
    let title = app.library.game(id).unwrap().title.clone();
    frame(&context, &mut app, size);
    let shown = text(&frame(&context, &mut app, size));
    for expected in [
        title.as_str(),
        "Play",
        "Roll again",
        PLAY_NOTE,
        READY_TO_TRY_LABEL,
        LAST_RECORDED_NOTE,
    ] {
        assert!(
            shown.iter().any(|line| line == expected),
            "missing {expected:?} in {shown:?}"
        );
    }
}

#[test]
fn the_window_explains_an_empty_library_and_an_empty_filter() {
    let context = egui::Context::default();
    let size = [1280.0, 800.0];

    let mut app = app(&context, Library::default());
    app.open_surprise_me();
    frame(&context, &mut app, size);
    let shown = text(&frame(&context, &mut app, size));
    assert!(shown.iter().any(|line| line == EMPTY_LIBRARY), "{shown:?}");

    let mut app = app_missing_only(&context);
    app.open_surprise_me();
    frame(&context, &mut app, size);
    let shown = text(&frame(&context, &mut app, size));
    assert!(shown.iter().any(|line| line == NO_MATCH), "{shown:?}");
}

fn app_missing_only(context: &egui::Context) -> App {
    let mut row = archive(2, "Bravo", Some("SNES"));
    row.last_verified_missing_at = Some("2026-11-01".into());
    app(context, Library::new(vec![row]))
}

const SURPRISE_BUTTON: &str = super::super::browse_play::SURPRISE_ME;

use crate::gui_v2::launch_readiness_summary::{CompactReadiness, ReadinessPresentationState};

/// A finished compact readiness answer, as the Library's card records it.
fn finished(tone: ReadinessPresentationState, badge: &'static str) -> Option<CheckedReadiness> {
    CheckedReadiness::from_compact(&CompactReadiness {
        badge,
        tone,
        spinner: false,
        reason: format!("{badge} (fixture)"),
        headline: badge.to_string(),
        play_enabled: false,
        fix: None,
        pending: false,
        more_findings: 0,
    })
}

#[test]
fn a_stale_record_stays_potentially_launchable_until_a_check_says_otherwise() {
    // The saved row says nothing is wrong; the file may have gone since the
    // last scan. Nothing touches the disk, so it stays a candidate.
    let library = Library::new(vec![
        archive(1, "Alpha", Some("SNES")),
        archive(6, "Echo", Some("SNES")),
    ]);
    let mut cache = ReadinessCache::default();
    let ready = filter(Some("SNES"), true);
    assert_eq!(
        ids(&library, &candidates(&library, &ready, &cache)),
        BTreeSet::from([1, 6])
    );

    // A Not ready answer taken against an older version of the file no longer
    // applies once a rescan records different file facts.
    cache.record(
        library.game(6).unwrap(),
        finished(
            ReadinessPresentationState::SourceUnavailable,
            "Game file unavailable",
        ),
    );
    assert_eq!(
        ids(&library, &candidates(&library, &ready, &cache)),
        BTreeSet::from([1])
    );
    let mut rescanned = archive(6, "Echo", Some("SNES"));
    rescanned.size_bytes = Some(8);
    let rescanned = Library::new(vec![archive(1, "Alpha", Some("SNES")), rescanned]);
    assert_eq!(
        ids(&rescanned, &candidates(&rescanned, &ready, &cache)),
        BTreeSet::from([1, 6])
    );
}

#[test]
fn a_failed_check_after_play_offers_roll_again_and_leaves_that_game_out() {
    use ReadinessPresentationState as State;
    for (tone, badge) in [
        // The file was deleted, or its mount is not available.
        (State::SourceUnavailable, "Game file unavailable"),
        (State::MediaUnreadable, "Game file unreadable"),
        (State::NeedsEmulator, "Emulator needed"),
    ] {
        let context = egui::Context::default();
        let mut app = app(&context, library());
        app.browse_play.surprise.filter = filter(Some("SNES"), true);
        app.open_surprise_me();
        let id = app.browse_play.surprise.current.unwrap();
        app.apply_surprise_action(SurpriseAction::Play(id));
        assert_eq!(app.router.current, Route::BrowsePlayGame(id));
        let game = app.library.game(id).unwrap().clone();

        // Still checking: no answer yet, so nothing is offered.
        assert_eq!(
            app.browse_play
                .surprise
                .retry_offer(&game, &app.browse_play.readiness),
            None
        );
        let shown = crate::shared_apply_warnings::test_support::render(|ui| {
            app.surprise_retry_offer(ui, &game)
        });
        assert!(!shown.contains(RETRY_NOTE));

        // The existing readiness check finishes Not ready.
        app.browse_play
            .readiness
            .record(&game, finished(tone, badge));
        assert_eq!(
            app.browse_play
                .surprise
                .retry_offer(&game, &app.browse_play.readiness),
            Some(badge)
        );
        let shown = crate::shared_apply_warnings::test_support::render(|ui| {
            app.surprise_retry_offer(ui, &game)
        });
        assert!(
            shown.contains(RETRY_NOTE) && shown.contains("Roll again"),
            "{shown}"
        );

        // Roll Again picks another game, never the one that failed.
        app.open_surprise_me();
        assert!(app.browse_play.surprise.open);
        let next = app.browse_play.surprise.current.unwrap();
        assert_ne!(next, id, "{badge}");
        assert_eq!(
            app.browse_play
                .surprise
                .retry_offer(&game, &app.browse_play.readiness),
            None
        );
        assert!(app.native_workflows.is_none());
    }
}

#[test]
fn a_ready_answer_or_a_game_not_from_surprise_me_gets_no_offer() {
    let context = egui::Context::default();
    let mut app = app(&context, library());
    app.open_surprise_me();
    let id = app.browse_play.surprise.current.unwrap();
    app.apply_surprise_action(SurpriseAction::Play(id));
    let game = app.library.game(id).unwrap().clone();
    app.browse_play
        .readiness
        .record(&game, Some(CheckedReadiness::Ready));
    assert_eq!(
        app.browse_play
            .surprise
            .retry_offer(&game, &app.browse_play.readiness),
        None
    );

    // A game picked by hand that fails its check is not a surprise pick.
    let other = app
        .library
        .game(if id == 1 { 7 } else { 1 })
        .unwrap()
        .clone();
    app.browse_play.readiness.record(
        &other,
        finished(
            ReadinessPresentationState::SourceUnavailable,
            "Game file unavailable",
        ),
    );
    assert_eq!(
        app.browse_play
            .surprise
            .retry_offer(&other, &app.browse_play.readiness),
        None
    );
}
