//! One-game DAT check and the Game Details verification panel, on the real
//! verified-game fixture (production scan, audit and save; disposable profile).
use super::super::game_verification_guidance::{
    Action, Guidance, explain, file_state, freshness, saved_evidence,
};
use super::super::library::Game;
use super::super::routes::Route;
use super::super::verified_game_fixture::{RomState, VerifiedGame};
use super::*;
use archivefs_core::Database;
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

/// The Region note exactly as Game Details builds it for a saved DAT region.
fn region_note(game: &Game) -> String {
    let rows = super::super::game_detail_facts::facts(
        game,
        super::super::game_detail_facts::Sources {
            dat_region: Some(("World", "Fixture No-Intro SNES")),
            dat_current: game.has_current_dat_verification(),
            region_checked: true,
            ..Default::default()
        },
    );
    let row = rows.iter().find(|row| row.label == "Region").unwrap();
    row.note.clone().unwrap()
}

/// Panel and Region note must agree about current DAT verification.
fn assert_agree(f: &VerifiedGame, game: &Game) -> bool {
    let verified = panel(f, game).reason == "Verified";
    let note = region_note(game);
    let current = note.starts_with("From the verified match");
    assert_eq!(
        verified,
        current,
        "panel {:?} vs region {note:?}",
        panel(f, game)
    );
    if !current {
        assert!(note.ends_with("Needs a recheck."), "{note}");
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
