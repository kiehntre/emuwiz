//! One-game DAT check and the Game Details verification panel, on the real
//! verified-game fixture (production scan, audit and save; disposable profile).
use super::super::game_verification_guidance::{
    Action, Guidance, dat_verified_now, explain, file_state, freshness, saved_evidence,
};
use super::super::library::Game;
use super::super::routes::Route;
use super::super::verified_game_fixture::{RomState, VerifiedGame};
use super::*;
use archivefs_core::Database;
use archivefs_core::dat::library_identity_summary::DatProvenanceFreshness;
use archivefs_core::dat::sources::{DatSourceRegistry, load_dat_sources_config_from};
use std::time::{Duration, Instant};

/// The profile's catalogues, with the fixture DAT assigned to the game's
/// system as a person does when registering it.
fn inputs(f: &VerifiedGame) -> Inputs {
    let mut config = load_dat_sources_config_from(f.profile.join("dat_sources.toml")).unwrap();
    let platform = f
        .game()
        .and_then(|game| archivefs_core::canonical_platform_for_alias(&game.platform))
        .expect("fixture game has a known system")
        .to_string();
    for entry in config.sources.iter_mut().flatten() {
        entry.platform = Some(platform.clone());
    }
    let (mut local_registry, problems) = DatSourceRegistry::from_config(&config);
    assert!(problems.is_empty(), "{problems:?}");
    // Validated through the production validator, as "Validate" does.
    let ids: Vec<String> = local_registry
        .entries()
        .iter()
        .map(|e| e.id.clone())
        .collect();
    for id in ids {
        let entry = local_registry.get_mut(&id).unwrap();
        entry.health = archivefs_core::dat::sources::validation::validate_dat_source(
            entry,
            Default::default(),
        )
        .0
        .to_health(&entry.path, entry.kind);
    }
    Inputs {
        database: f.database.clone(),
        inventory: CatalogueInventorySnapshot {
            local_registry,
            managed_sources: Default::default(),
            managed_root: f.profile.join("managed-dats"),
        },
        trusted: TrustedRoots::none(),
    }
}

/// The verification panel's decision exactly as its worker makes it.
fn panel(f: &VerifiedGame, game: &Game) -> Guidance {
    let database = Database::open_read_only(&f.database).unwrap();
    let (checks, stale) = saved_evidence(game, &database).unwrap();
    let (metadata, unchanged) = file_state(game);
    let catalogues = list_installed_catalogues(inputs(f).inventory.inputs());
    explain(
        game,
        metadata.is_some(),
        unchanged,
        checks,
        freshness(game, stale),
        Ok(&catalogues),
        None,
    )
}

/// The Region note exactly as Game Details builds it for a saved DAT region,
/// given the panel's answer (or `None` before it has one).
fn region_note(game: &Game, panel: Option<&Guidance>) -> String {
    let rows = super::super::game_detail_facts::facts(
        game,
        super::super::game_detail_facts::Sources {
            dat_region: Some(("World", "Fixture No-Intro SNES")),
            dat_current: dat_verified_now(game, panel),
            region_checked: true,
            ..Default::default()
        },
    );
    let row = rows.iter().find(|row| row.label == "Region").unwrap();
    row.note.clone().unwrap()
}

/// Panel and Region note must agree about current DAT verification once the
/// panel has answered, and nothing may say Verified before it has.
fn assert_agree(f: &VerifiedGame, game: &Game) -> bool {
    let verified = assert_agree_after_panel(f, game);
    let early = region_note(game, None);
    assert!(!early.starts_with("From the verified match"), "{early}");
    verified
}

/// Agreement once the panel has answered (the state a person reads).
fn assert_agree_after_panel(f: &VerifiedGame, game: &Game) -> bool {
    let guidance = panel(f, game);
    let verified = guidance.reason == "Verified";
    let note = region_note(game, Some(&guidance));
    let current = note.starts_with("From the verified match");
    assert_eq!(verified, current, "panel {guidance:?} vs region {note:?}");
    if !current {
        assert!(note.ends_with("Not confirmed as current."), "{note}");
    }
    verified
}

fn game(f: &VerifiedGame) -> Game {
    f.game().expect("fixture game in library")
}

/// Removes the two proof fields from every saved row, as rows saved before
/// they were required look. Verdicts and hashes stay.
fn strip_proof(f: &VerifiedGame) {
    let conn = rusqlite::Connection::open(&f.database).unwrap();
    let rows: Vec<(i64, String, Vec<u8>)> = conn
        .prepare("SELECT archive_id, dat_source_id, facts_json FROM library_dat_identities")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!rows.is_empty());
    for (id, source, bytes) in rows {
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("audit_file");
        object["source"]
            .as_object_mut()
            .unwrap()
            .remove("catalogue_sha256");
        conn.execute(
            "UPDATE library_dat_identities SET facts_json = ?1 WHERE archive_id = ?2 AND dat_source_id = ?3",
            rusqlite::params![serde_json::to_vec(&value).unwrap(), id, source],
        )
        .unwrap();
    }
}

fn dat_rows(f: &VerifiedGame) -> Vec<(i64, Vec<u8>)> {
    let conn = rusqlite::Connection::open(&f.database).unwrap();
    conn.prepare("SELECT archive_id, facts_json FROM library_dat_identities ORDER BY archive_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn wait(state: &mut State, done: impl Fn(&Stage) -> bool) {
    let start = Instant::now();
    loop {
        state.poll();
        if done(&state.stage) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "check did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Opens the page for `game` and waits for catalogue choice.
fn opened(f: &VerifiedGame, game: &Game) -> State {
    let mut state = State::default();
    let loaded = inputs(f);
    state.open(Target::of(game), move || Ok(loaded));
    wait(&mut state, |stage| !matches!(stage, Stage::Loading));
    assert!(
        matches!(state.stage, Stage::Choose),
        "catalogues failed to load"
    );
    assert_eq!(state.catalogues.len(), 1);
    assert!(
        state.choice.is_some(),
        "the only ready catalogue is preselected"
    );
    state
}

/// Preview, confirm and wait for the finding.
fn check(f: &VerifiedGame, game: &Game) -> (State, Finding) {
    let mut state = opened(f, game);
    state.preview();
    wait(&mut state, |stage| !matches!(stage, Stage::Previewing));
    let Stage::Ready(preview) = &state.stage else {
        panic!("preview failed");
    };
    assert_eq!(
        preview.eligible_paths().collect::<Vec<_>>(),
        vec![game.archive.absolute_path.as_path()]
    );
    state.confirm();
    wait(&mut state, |stage| {
        matches!(stage, Stage::Finished(_) | Stage::Failed(_))
    });
    let Stage::Finished(finished) = &state.stage else {
        let Stage::Failed(error) = &state.stage else {
            unreachable!()
        };
        panic!("check failed: {error}");
    };
    let finding = finished.finding.clone();
    (state, finding)
}

#[test]
fn fresh_current_proof_is_verified_in_the_panel_and_the_region_note() {
    let f = VerifiedGame::new();
    let game = game(&f);
    assert!(game.has_current_dat_verification());
    let guidance = panel(&f, &game);
    assert_eq!(guidance.reason, "Verified", "{guidance:?}");
    assert_eq!(guidance.action, Action::Review);
    assert!(assert_agree(&f, &game));
}

#[test]
fn legacy_rows_need_a_recheck_and_a_one_game_check_restores_verified() {
    let f = VerifiedGame::new();
    strip_proof(&f);
    let legacy = game(&f);
    assert!(!legacy.has_current_dat_verification());
    let guidance = panel(&f, &legacy);
    assert_ne!(guidance.reason, "Verified");
    assert_eq!(guidance.action, Action::Recheck, "{guidance:?}");
    assert_eq!(guidance.action.label(), "Re-check this game");
    assert_eq!(
        guidance.action.route(&legacy),
        Route::GameCheck(legacy.archive.id)
    );
    assert!(!assert_agree(&f, &legacy));

    let (mut state, finding) = check(&f, &legacy);
    assert_eq!(finding, Finding::Verified);
    assert!(state.take_saved(), "the library is told to reload");
    assert!(!state.take_saved());
    let fresh = game(&f);
    assert!(fresh.has_current_dat_verification());
    assert_eq!(panel(&f, &fresh).reason, "Verified");
    assert!(assert_agree(&f, &fresh));
}

#[test]
fn a_rechecked_file_newer_than_its_scan_row_is_verified_not_changed() {
    let f = VerifiedGame::new();
    strip_proof(&f);
    // Same bytes rewritten: the scan's mtime baseline no longer matches.
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(
        &f.rom,
        super::super::verified_game_fixture::rom_bytes(RomState::Normal),
    )
    .unwrap();
    let before = game(&f);
    assert!(!file_state(&before).1, "scan baseline is stale");
    assert_eq!(panel(&f, &before).reason, "File evidence stale or changed");
    assert!(!assert_agree(&f, &before));
    let (_, finding) = check(&f, &before);
    assert_eq!(finding, Finding::Verified);
    let after = game(&f);
    assert!(after.has_current_dat_verification());
    assert!(assert_agree(&f, &after));
}

#[test]
fn a_mismatched_file_is_never_verified_in_either_place() {
    let f = VerifiedGame::built(RomState::Mismatched);
    let before = game(&f);
    assert!(!assert_agree(&f, &before));
    let (_, finding) = check(&f, &before);
    assert_eq!(finding, Finding::NoMatch);
    let after = game(&f);
    assert!(!after.has_current_dat_verification());
    assert_eq!(panel(&f, &after).reason, "No catalogue match");
    assert!(!assert_agree(&f, &after));
}

#[test]
fn a_changed_file_after_a_verified_check_loses_verified_in_both_places() {
    let f = VerifiedGame::new();
    f.change_rom();
    let game = game(&f);
    assert!(!game.has_current_dat_verification());
    assert!(!assert_agree(&f, &game));
}

#[test]
fn a_missing_file_fails_without_checking_or_saving_anything() {
    let f = VerifiedGame::new();
    strip_proof(&f);
    let game = game(&f);
    f.remove_rom();
    let saved = dat_rows(&f);
    let mut state = opened(&f, &game);
    state.preview();
    wait(&mut state, |stage| !matches!(stage, Stage::Previewing));
    let Stage::Failed(error) = &state.stage else {
        panic!("a missing file must not reach confirmation");
    };
    assert!(error.contains("Unavailable file"), "{error}");
    assert!(error.contains("Nothing was checked"), "{error}");
    assert!(!state.take_saved());
    assert_eq!(dat_rows(&f), saved);
    assert_ne!(panel(&f, &game).reason, "Verified");
}

#[test]
fn cancelling_before_confirmation_returns_to_the_choice_without_reading() {
    let f = VerifiedGame::new();
    strip_proof(&f);
    let game = game(&f);
    let saved = dat_rows(&f);
    let mut state = opened(&f, &game);
    state.preview();
    wait(&mut state, |stage| !matches!(stage, Stage::Previewing));
    assert!(matches!(state.stage, Stage::Ready(_)));
    state.cancel();
    assert!(matches!(state.stage, Stage::Choose));
    assert!(!state.busy());
    assert_eq!(dat_rows(&f), saved);
}

#[test]
fn only_setup_still_opens_the_platform_check() {
    let f = VerifiedGame::new();
    let game = game(&f);
    for action in [Action::Audit, Action::Recheck] {
        assert_eq!(action.route(&game), Route::GameCheck(game.archive.id));
    }
    assert!(matches!(
        Action::SelectDat.route(&game),
        Route::PlatformCheck(_)
    ));
}

#[test]
fn opening_another_game_keeps_a_pending_reload_and_rebinds() {
    let f = VerifiedGame::new();
    strip_proof(&f);
    let game = game(&f);
    let (mut state, finding) = check(&f, &game);
    assert_eq!(finding, Finding::Verified);
    // The App had a library load running, so the flag was not taken yet.
    let mut other = Target::of(&game);
    other.archive_id += 1000;
    other.title = "Another game".into();
    state.open(other.clone(), || Err("not needed".into()));
    assert_eq!(state.target.as_ref(), Some(&other));
    assert!(
        state.take_saved(),
        "a finished save still reloads the library"
    );
}

#[test]
fn opening_another_game_during_preview_rebinds_without_saving() {
    let f = VerifiedGame::new();
    let game = game(&f);
    let saved = dat_rows(&f);
    let mut state = opened(&f, &game);
    state.preview();
    let mut other = Target::of(&game);
    other.archive_id += 1000;
    state.open(other.clone(), || Err("Catalogues unavailable".into()));
    assert_eq!(state.target.as_ref(), Some(&other));
    wait(&mut state, |stage| !matches!(stage, Stage::Loading));
    assert!(matches!(state.stage, Stage::Failed(_)));
    assert!(!state.take_saved());
    assert_eq!(dat_rows(&f), saved);
}

#[test]
fn cancelling_a_running_check_saves_nothing() {
    let f = VerifiedGame::new();
    strip_proof(&f);
    let game = game(&f);
    let saved = dat_rows(&f);
    let mut state = opened(&f, &game);
    state.preview();
    wait(&mut state, |stage| !matches!(stage, Stage::Previewing));
    assert!(matches!(state.stage, Stage::Ready(_)));
    state.confirm();
    assert!(matches!(state.stage, Stage::Running(_)));
    // A running check keeps its own game.
    let mut other = Target::of(&game);
    other.archive_id += 1000;
    state.open(other, || Err("must not load".into()));
    assert_eq!(state.target.as_ref().unwrap().archive_id, game.archive.id);
    state.cancel();
    wait(&mut state, |stage| {
        matches!(stage, Stage::Finished(_) | Stage::Failed(_))
    });
    let Stage::Failed(error) = &state.stage else {
        panic!("a cancelled check must not finish");
    };
    // Cancel lands either before input validation ("Cancelled. No
    // verification results saved.") or inside the audit ("the audit was
    // cancelled"); both are real cancellations. Any other failure still fails.
    assert!(error.to_lowercase().contains("cancelled"), "{error}");
    assert!(!state.take_saved());
    assert_eq!(dat_rows(&f), saved);
    assert!(!game_of(&f).has_current_dat_verification());
}

fn game_of(f: &VerifiedGame) -> Game {
    game(f)
}

#[test]
fn a_stale_second_catalogue_makes_the_region_note_follow_the_panel() {
    let f = VerifiedGame::new();
    let game = game(&f);
    assert!(game.has_current_dat_verification());
    // A second catalogue's saved row for the same game, marked stale.
    let mut database = Database::open_or_create(&f.database).unwrap();
    let mut other = database
        .library_dat_identities_for_item(game.archive.id)
        .unwrap()
        .remove(0);
    other.source.source_id = "older-catalogue".into();
    other.source.source_revision = Some("old".into());
    database
        .persist_library_dat_identity(game.archive.id, &other)
        .unwrap();
    database
        .mark_library_dat_identity_stale_for_source_revision("older-catalogue", Some("new"))
        .unwrap();
    drop(database);
    let game = game_of(&f);
    // The Library skips a catalogue that is not configured, so its decision
    // stays current; the panel conservatively reports the stale row. The
    // Region note follows the panel instead of contradicting it.
    assert!(game.has_current_dat_verification());
    assert_eq!(panel(&f, &game).reason, "File evidence stale or changed");
    assert!(!assert_agree_after_panel(&f, &game));
    // The game's own file changing still wins over that current decision.
    f.change_rom();
    let changed = panel(&f, &game);
    assert_eq!(
        changed.reason, "File evidence stale or changed",
        "{changed:?}"
    );
}

#[test]
fn a_changed_catalogue_is_never_current_in_either_place() {
    let f = VerifiedGame::new();
    assert!(game(&f).has_current_dat_verification());
    // Same catalogue path, different bytes: its SHA-256 no longer matches the
    // one the saved match recorded.
    let mut dat = std::fs::read_to_string(&f.dat).unwrap();
    dat.push_str("\n<!-- edited -->\n");
    std::fs::write(&f.dat, dat).unwrap();
    let game = game(&f);
    assert!(!game.has_current_dat_verification());
    assert_ne!(panel(&f, &game).reason, "Verified");
    assert!(!assert_agree(&f, &game));
}

/// The Library loaded current proof, then the file changed: the panel
/// re-reads the saved rows and must not say Verified.
#[test]
fn a_file_changed_after_library_load_is_not_verified() {
    let f = VerifiedGame::new();
    let game = game(&f);
    assert!(game.has_current_dat_verification());
    f.change_rom();
    assert_ne!(panel(&f, &game).reason, "Verified");
    assert!(!assert_agree(&f, &game));
}

/// Same bytes, size and modification time, but a new file-system object
/// (as after a remount or a copy back): not proof of the same file, so the
/// panel must not say Verified while core reports it uncertain.
#[test]
fn a_new_file_object_after_library_load_is_not_verified() {
    let f = VerifiedGame::new();
    let game = game(&f);
    assert!(game.has_current_dat_verification());
    let path = &game.archive.absolute_path;
    let bytes = std::fs::read(path).unwrap();
    let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
    let replacement = path.with_extension("tmp");
    std::fs::write(&replacement, &bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    std::fs::rename(&replacement, path).unwrap();
    assert_ne!(panel(&f, &game).reason, "Verified");
    let database = Database::open_read_only(&f.database).unwrap();
    for check in database
        .library_dat_identities_for_item(game.archive.id)
        .unwrap()
    {
        let summary = database
            .library_dat_identity_summary_for_item(
                game.archive.id,
                &check.source.source_id,
                None,
                None,
                true,
            )
            .unwrap();
        assert_ne!(
            summary.map(|summary| summary.provenance_freshness),
            Some(DatProvenanceFreshness::Current)
        );
    }
}

// --- Game Details: one Verify / Re-check action, the card states the result --
//
// Each case renders the real Game Details page for the fixture game, with the
// verification card holding the answer its worker computes from this fixture
// (`panel`, `explain`): real database, real DAT, real scan. The answer is
// handed in rather than left to the worker only because the worker reads the
// default (real) profile.

mod one_action_in_game_details {
    use super::*;
    use crate::gui_v2::App;
    use crate::gui_v2::library::Library;
    use crate::gui_v2::routes::Section;
    use crate::gui_v2::tests::{fixture, frame, text, text_bounds};
    use eframe::egui;
    use std::sync::Arc;

    const SIZE: [f32; 2] = [1280.0, 2400.0];

    fn details(context: &egui::Context, library: Library, id: i64, card: Guidance) -> App {
        let mut app = fixture(context);
        app.simple.enabled = true;
        app.library = Arc::new(library);
        app.loaded = true;
        app.router.current = Route::Game(id);
        app.review
            .verification_guidance
            .set_for_test(id, None, Some(card));
        app
    }

    fn shown(context: &egui::Context, app: &mut App) -> Vec<String> {
        frame(context, app, SIZE);
        text(&frame(context, app, SIZE))
    }

    fn count(strings: &[String], wanted: &str) -> usize {
        strings.iter().filter(|line| *line == wanted).count()
    }

    fn click(context: &egui::Context, app: &mut App, label: &str) {
        context.style_mut(|style| style.animation_time = 0.0);
        let layout = frame(context, app, SIZE);
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
                        egui::vec2(SIZE[0], SIZE[1]),
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
    }

    /// Registered and scanned, never checked: a ready catalogue, no result.
    fn scanned() -> VerifiedGame {
        let f = VerifiedGame::unregistered(RomState::Normal);
        f.register_source();
        f.scan();
        f
    }

    /// The card's answer when no catalogue at all is installed.
    fn panel_without_catalogues(game: &Game) -> Guidance {
        let (metadata, unchanged) = file_state(game);
        explain(
            game,
            metadata.is_some(),
            unchanged,
            0,
            freshness(game, false),
            Ok(&[]),
            None,
        )
    }

    #[test]
    fn a_ready_catalogue_offers_one_verify_this_game_which_opens_the_one_game_check() {
        let f = scanned();
        let game = game(&f);
        let card = panel(&f, &game);
        assert_eq!(card.action, Action::Audit, "{card:?}");
        let context = egui::Context::default();
        let mut app = details(&context, f.library(), game.archive.id, card);
        let strings = shown(&context, &mut app);
        assert_eq!(count(&strings, "Verify this game"), 1, "{strings:?}");
        assert_eq!(count(&strings, "Re-check this game"), 0);
        assert!(
            strings
                .iter()
                .any(|line| line == "To check it, use Verify this game under More for this game."),
            "{strings:?}"
        );
        assert!(
            strings
                .iter()
                .any(|line| line == "Verification: Not checked yet")
        );
        assert!(!strings.iter().any(|line| line == "Verification: Verified"));
        // Bound already, so painting the check page reads no real inventory.
        app.game_dat_check.target = Some(Target::of(&game));
        click(&context, &mut app, "Verify this game");
        assert_eq!(app.router.current, Route::GameCheck(game.archive.id));
    }

    #[test]
    fn a_previously_checked_game_offers_one_re_check_and_keeps_its_saved_result() {
        let f = VerifiedGame::new();
        let game = game(&f);
        let card = panel(&f, &game);
        assert_eq!(card.reason, "Verified", "{card:?}");
        let context = egui::Context::default();
        let mut app = details(&context, f.library(), game.archive.id, card);
        let strings = shown(&context, &mut app);
        assert_eq!(count(&strings, "Re-check this game"), 1, "{strings:?}");
        assert_eq!(count(&strings, "Verify this game"), 0);
        assert!(strings.iter().any(|line| line == "Verification: Verified"));
        // The card's own action is the saved result, not another check.
        assert!(
            strings
                .iter()
                .any(|line| line == "View verification result")
        );

        // A legacy saved row: the card asks for a re-check and points at the
        // one button instead of repeating it.
        strip_proof(&f);
        let legacy = super::game(&f);
        let card = panel(&f, &legacy);
        assert_eq!(card.action, Action::Recheck, "{card:?}");
        let mut app = details(&context, f.library(), legacy.archive.id, card);
        let strings = shown(&context, &mut app);
        assert_eq!(count(&strings, "Re-check this game"), 1, "{strings:?}");
        assert!(!strings.iter().any(|line| line == "Verification: Verified"));
        assert!(strings.iter().any(|line| line
            == "To check it, use Re-check this game under More for this game."));
        app.game_dat_check.target = Some(Target::of(&legacy));
        click(&context, &mut app, "Re-check this game");
        assert_eq!(app.router.current, Route::GameCheck(legacy.archive.id));
    }

    #[test]
    fn a_saved_result_and_the_system_tally_are_two_differently_named_links() {
        let f = VerifiedGame::new();
        let game = game(&f);
        let card = panel(&f, &game);
        assert_eq!(card.action, Action::Review, "{card:?}");
        let id = game.archive.id;
        let context = egui::Context::default();
        let open = |context: &egui::Context| {
            let mut app = details(context, f.library(), id, card.clone());
            // What the detail loader reports for this fixture: one saved
            // check of the same, unchanged file.
            app.detail = Some(crate::gui_v2::library::Detail {
                game: id,
                file_present: true,
                unchanged: true,
                saved_checks: 1,
                ..Default::default()
            });
            app
        };
        let mut app = open(&context);
        let strings = shown(&context, &mut app);
        let summary = crate::gui_v2::pages::system_check_summary_label("SNES");
        assert_eq!(summary, "View SNES check summary");
        assert_eq!(
            count(&strings, "View verification result"),
            1,
            "{strings:?}"
        );
        assert_eq!(count(&strings, &summary), 1, "{strings:?}");

        // The card's link opens this game's own saved result...
        click(&context, &mut app, "View verification result");
        assert_eq!(app.router.current, Route::ReviewIdentity(id));

        // ...and the other opens the system-wide Check Games tally, on
        // this game's system and marked as opened from it.
        let mut app = open(&context);
        app.check_platform = Some("Mega Drive".into());
        shown(&context, &mut app);
        click(&context, &mut app, &summary);
        assert_eq!(
            app.router.current,
            Route::Task {
                section: crate::gui_v2::routes::Section::Check,
                game: id,
            }
        );
        assert_eq!(app.check_platform.as_deref(), Some("SNES"));
    }

    #[test]
    fn the_system_tally_link_is_named_even_without_a_system() {
        assert_eq!(
            crate::gui_v2::pages::system_check_summary_label(""),
            "View Check Games summary"
        );
    }

    #[test]
    fn a_missing_catalogue_keeps_set_up_verification_beside_the_one_check() {
        let f = scanned();
        let game = game(&f);
        let card = panel_without_catalogues(&game);
        assert_eq!(card.action, Action::SelectDat, "{card:?}");
        let context = egui::Context::default();
        let mut app = details(&context, f.library(), game.archive.id, card);
        let strings = shown(&context, &mut app);
        assert_eq!(count(&strings, "Set up verification"), 1, "{strings:?}");
        assert_eq!(count(&strings, "Verify this game"), 1, "{strings:?}");
        assert!(
            strings
                .iter()
                .any(|line| line == "Verification: Needs setup")
        );
        assert!(!strings.iter().any(|line| line == "Verification: Verified"));
        click(&context, &mut app, "Set up verification");
        assert_eq!(app.router.current, Route::PlatformCheck("SNES".into()));
    }

    #[test]
    fn game_details_routes_for_an_unknown_system_clear_a_stale_system_first() {
        let f = scanned();
        let mut archive = game(&f).archive;
        archive.platform = None;
        archive.platform_source = None;
        let library = Library::new(vec![archive]);
        let game = library.games[0].clone();
        assert!(archivefs_core::canonical_platform_for_alias(&game.platform).is_none());
        // The real catalogue is installed for SNES; this game's system is unknown.
        let (metadata, unchanged) = file_state(&game);
        let catalogues = list_installed_catalogues(inputs(&f).inventory.inputs());
        let card = explain(
            &game,
            metadata.is_some(),
            unchanged,
            0,
            freshness(&game, false),
            Ok(&catalogues),
            None,
        );
        assert_eq!(card.action, Action::SelectDat, "{card:?}");
        let context = egui::Context::default();
        let id = game.archive.id;
        for label in ["Set up verification", "Verify this game"] {
            let mut app = details(&context, library.clone(), id, card.clone());
            let strings = shown(&context, &mut app);
            assert_eq!(count(&strings, label), 1, "{label}: {strings:?}");
            app.check_platform = Some("SNES".into());
            app.verification = Some(crate::gui_v2::backend::VerificationResult {
                platform: "SNES".into(),
                matched: 0,
                attention: 0,
                unknown: 0,
                missing: 0,
                total: 0,
                statuses: Default::default(),
            });
            click(&context, &mut app, label);
            assert_eq!(
                app.router.current,
                Route::Section(Section::Check),
                "{label}"
            );
            assert_eq!(app.check_platform, None, "{label}");
            assert!(app.verification.is_none(), "{label}");
            assert!(app.verification_job.is_none(), "{label}");
        }
    }
}

/// A catalogue with no saved system whose header names this system is
/// suggested for an explicit trial: never preselected, never saved here.
#[test]
fn an_unassigned_catalogue_with_a_matching_header_is_suggested_not_preselected() {
    let f = VerifiedGame::new();
    let game = game(&f);
    let platform = Target::of(&game).platform;
    let loaded = inputs(&f);
    let mut rows = list_installed_catalogues(loaded.inventory.inputs());
    assert!(!rows.is_empty());
    rows[0].platform = EvidenceValue::Unknown;
    let id = local_source_id(&rows[0]).unwrap().to_string();
    let system = archivefs_core::canonical_platform_for_alias(&platform).unwrap();
    let matches = vec![(id.clone(), system)];
    let suggested = suggested_catalogues(&rows, &matches, &platform);
    assert_eq!(suggested.len(), 1);
    assert!(ready_catalogues(rows.clone(), &platform).is_empty());
    // No header match, or another system's game: nothing is suggested.
    assert!(suggested_catalogues(&rows, &[], &platform).is_empty());
    assert!(suggested_catalogues(&rows, &matches, "PSX").is_empty());
    // A saved system is never re-suggested.
    let mut assigned = rows.clone();
    assigned[0].platform = EvidenceValue::Assigned(platform.clone());
    assert!(suggested_catalogues(&assigned, &matches, &platform).is_empty());

    let mut state = State {
        target: Some(Target::of(&game)),
        stage: Stage::Choose,
        suggested,
        ..Default::default()
    };
    let painted = crate::shared_apply_warnings::test_support::render(|ui| {
        let _ = show(ui, &mut state, game.archive.id);
    });
    for wanted in [
        "Suggested — needs your confirmation",
        &suggested_reason(&platform),
        CHECK_THIS_GAME,
        NOT_NEEDED_TO_PLAY,
    ] {
        assert!(painted.contains(wanted), "{wanted}: {painted}");
    }
    assert!(state.choice.is_none(), "a suggestion is never preselected");
    assert!(
        !painted.contains(&use_for_system_label(&platform)),
        "{painted}"
    );
    assert!(!painted.contains("Verified"), "{painted}");
}

/// End to end on the real engine: the fixture's catalogue with no saved
/// system is tried for this game only, verified, then offered for "from now
/// on". The saved setup is unchanged until Yes.
#[test]
fn a_suggested_catalogue_trial_verifies_then_offers_to_remember() {
    let f = VerifiedGame::new();
    let game = game(&f);
    let target = Target::of(&game);
    let config = f.profile.join("dat_sources.toml");
    let before = std::fs::read(&config).unwrap();
    // The fixture registry exactly as saved: validated, no system set.
    let mut loaded = inputs(&f);
    for entry in loaded.inventory.local_registry.entries().to_vec() {
        loaded
            .inventory
            .local_registry
            .get_mut(&entry.id)
            .unwrap()
            .set_platform_by_person(None);
    }
    let matches = header_matches(&loaded);
    let rows = list_installed_catalogues(loaded.inventory.inputs());
    let suggested = suggested_catalogues(&rows, &matches, &target.platform);
    // "Nintendo - Super Nintendo Entertainment System" is recognised by the
    // existing rule, so the trial is offered.
    assert_eq!(suggested.len(), 1, "{matches:?}");
    let mut state = State {
        target: Some(target.clone()),
        stage: Stage::Choose,
        inputs: Some(loaded),
        choice: Some(suggested[0].reference.clone()),
        suggested,
        ..Default::default()
    };
    state.preview();
    wait(&mut state, |stage| !matches!(stage, Stage::Previewing));
    assert!(
        matches!(state.stage, Stage::Ready(_)),
        "header re-proved by core"
    );
    state.confirm();
    wait(&mut state, |stage| {
        matches!(stage, Stage::Finished(_) | Stage::Failed(_))
    });
    let Stage::Finished(finished) = &state.stage else {
        panic!("trial failed");
    };
    assert_eq!(finished.finding, Finding::Verified);
    assert!(state.assignable.is_some(), "Verified offers from now on");
    assert_eq!(
        std::fs::read(&config).unwrap(),
        before,
        "setup unchanged before Yes"
    );
}

/// The fixture's confirmed catalogue still takes the full check path, and a
/// catalogue that already has a saved system is never offered again.
#[test]
fn the_confirmed_catalogue_checks_this_game_without_reassigning_it() {
    let f = VerifiedGame::new();
    let game = game(&f);
    let (state, finding) = check(&f, &game);
    assert_eq!(finding, Finding::Verified);
    // The fixture's catalogue already has a saved system, so the existing
    // rule offers nothing to remember.
    assert!(state.assignable.is_none());
}

/// "From now on" follows only a Verified result. No match, more than one
/// match and not-checked results never offer it.
#[test]
fn only_a_verified_result_offers_to_remember_the_catalogue() {
    let candidate = Some(("dat".to_string(), "SNES".to_string(), "Fixture".to_string()));
    assert_eq!(
        assignable_after(&Finding::Verified, candidate.clone()),
        candidate
    );
    for finding in [
        Finding::NoMatch,
        Finding::Ambiguous,
        Finding::NotChecked("unreadable".into()),
    ] {
        assert_eq!(
            assignable_after(&finding, candidate.clone()),
            None,
            "{finding:?}"
        );
    }
    assert_eq!(assignable_after(&Finding::Verified, None), None);
}

/// The existing rule: a local catalogue the preview proved for this system,
/// with no system saved yet. A saved or different system is never offered.
#[test]
fn only_an_unassigned_preview_proven_catalogue_is_a_candidate() {
    use archivefs_core::dat::sources::PlatformProvenance;
    let f = VerifiedGame::new();
    let loaded = inputs(&f);
    let mut row = list_installed_catalogues(loaded.inventory.inputs()).remove(0);
    row.platform_origin = PlatformProvenance::NotSet;
    assert!(assignment_candidate(&row, "SNES", "SNES").is_some());
    assert!(assignment_candidate(&row, "NES", "SNES").is_none());
    row.platform_origin = PlatformProvenance::ChosenByPerson;
    assert!(assignment_candidate(&row, "SNES", "SNES").is_none());
}

/// A cancelled or failed check offers nothing and saves nothing.
#[test]
fn a_cancelled_check_never_offers_an_assignment() {
    let (sender, receiver) = mpsc::channel();
    let mut state = State {
        stage: Stage::Running("Reading…".into()),
        executing: Some(("dat".into(), "SNES".into(), "Fixture".into())),
        receiver: Some(receiver),
        ..Default::default()
    };
    sender
        .send(Message::Done(Err(super::super::activity::CANCELLED.into())))
        .unwrap();
    state.poll();
    assert!(matches!(state.stage, Stage::Failed(_)));
    assert!(state.assignable.is_none());
    assert!(state.executing.is_none());
}

/// Yes saves through the existing Check My Games assignment: the choice is
/// on disk, recorded as the person's, and survives a restart (reload).
#[test]
fn a_confirmed_assignment_persists_across_a_restart() {
    use archivefs_core::dat::sources::PlatformProvenance;
    let f = VerifiedGame::new();
    let config = f.profile.join("dat_sources.toml");
    let before = load_dat_sources_config_from(&config).unwrap();
    let entry = before.sources.iter().flatten().next().unwrap().clone();
    let provenance = |config: &archivefs_core::dat::sources::DatSourcesConfig| {
        let (registry, _) = DatSourceRegistry::from_config(config);
        let entry = registry.get(&entry.id).unwrap();
        (entry.platform.clone(), entry.platform_provenance())
    };
    assert_eq!(provenance(&before), (None, PlatformProvenance::NotSet));
    let mut page = crate::dat_sources_page::DatSourcesPageState::load(
        config.clone(),
        Vec::new(),
        TrustedRoots::none(),
    );
    page.apply(
        crate::dat_sources_page::DatSourcesPageAction::AssignPlatformNow {
            id: entry.id.clone(),
            platform: "SNES".into(),
        },
    );
    let result = page.take_platform_assignment().unwrap();
    assert!(result.is_ok(), "{result:?}");
    drop(page);
    let after = load_dat_sources_config_from(&config).unwrap();
    assert_eq!(
        provenance(&after),
        (Some("SNES".to_string()), PlatformProvenance::ChosenByPerson)
    );
}
