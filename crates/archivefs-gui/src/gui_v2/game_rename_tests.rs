//! Blue rename card state: preview, confirm, refusal; disposable library only.
use super::State;
use std::path::{Path, PathBuf};

fn library(temp: &Path) -> (PathBuf, PathBuf, i64) {
    let root = temp.join("games");
    std::fs::create_dir_all(root.join("nes")).unwrap();
    let game = root.join("nes").join("Old.nes");
    std::fs::write(&game, b"NES\x1a synthetic").unwrap();
    let database = temp.join("library.sqlite3");
    let mut db = archivefs_core::Database::open_or_create(&database).unwrap();
    db.register_source_folders(std::slice::from_ref(&root))
        .unwrap();
    let folder = db.list_source_folders().unwrap()[0].id;
    let archive = archivefs_core::Archive::from_path_in_root(&game, &root).unwrap();
    let id = db
        .upsert_archive(folder, &root, &archive)
        .unwrap()
        .archive_id;
    db.set_manual_platform(id, "nes").unwrap();
    (database, game, id)
}

fn state(temp: &Path, database: PathBuf) -> State {
    State {
        paths: Some((database, temp.join("journal"))),
        saves: Some(Default::default()),
        ..State::default()
    }
}

#[test]
fn preview_then_confirm_renames_and_keeps_the_game() {
    let temp = tempfile::tempdir().unwrap();
    let (database, game, id) = library(temp.path());
    let mut card = state(temp.path(), database.clone());
    card.typed = "New Game (USA)".into();
    assert!(!card.confirm(), "nothing happens without a preview");
    assert!(game.exists());
    card.preview_now(&game);
    assert!(
        matches!(card.preview, Some(Ok(_))),
        "{:?}",
        card.preview.as_ref().map(|r| r.as_ref().err())
    );
    assert!(card.confirm());
    let renamed = game.with_file_name("New Game (USA).nes");
    assert!(renamed.exists() && !game.exists());
    let db = archivefs_core::Database::open_read_only(&database).unwrap();
    assert_eq!(
        db.find_archive_id_by_absolute_path(&renamed).unwrap(),
        Some(id)
    );
    assert!(matches!(&card.outcome, Some(Ok(message)) if message.contains("Undo is in History")));
}

#[test]
fn a_clash_is_shown_and_nothing_changes() {
    let temp = tempfile::tempdir().unwrap();
    let (database, game, _) = library(temp.path());
    std::fs::write(game.with_file_name("Taken.nes"), b"other").unwrap();
    let mut card = state(temp.path(), database);
    card.typed = "Taken".into();
    card.preview_now(&game);
    assert!(matches!(card.preview, Some(Err(_))));
    assert!(!card.confirm());
    assert!(game.exists());
    assert_eq!(
        std::fs::read(game.with_file_name("Taken.nes")).unwrap(),
        b"other"
    );
}

/// After a rename, the one-game re-check's answer reaches Activity and the
/// card, and a saved result reloads the library so Game Details shows it.
#[test]
fn a_finished_recheck_is_reported_and_reloads_the_library() {
    use archivefs_core::dat::reaudit::{GameRecheck, ReauditPlan, ReauditReport};
    let context = eframe::egui::Context::default();
    let mut app = super::super::tests::fixture(&context);
    app.game_rename.open_for = Some(7);
    let job = app.activity.queue(
        "Re-checking the renamed game",
        super::super::Route::Game(7),
        false,
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    app.game_rename.recheck = Some(super::Recheck {
        job,
        archive_id: Some(7),
        receiver,
    });
    app.poll_rename_recheck();
    assert!(app.game_rename.recheck.is_some(), "still running");
    // Another rename meanwhile: its re-check waits, it is not dropped.
    app.start_rename_recheck(super::RecheckTarget::Id(8));
    assert_eq!(app.game_rename.queued.len(), 1);
    sender
        .send(Ok(GameRecheck {
            plan: ReauditPlan {
                saved_rows: 1,
                ..Default::default()
            },
            report: Some(ReauditReport {
                planned: 1,
                verified: 1,
                saved: 1,
                ..Default::default()
            }),
        }))
        .unwrap();
    app.poll_rename_recheck();
    // The waiting request was taken up once the first finished.
    assert!(app.game_rename.queued.is_empty());
    assert!(app.game_rename.recheck.is_none());
    let line = app.game_rename.recheck_line.clone().unwrap();
    assert!(line.contains("still verified"), "{line}");
    assert_eq!(app.activity.jobs[&job].summary, line);
    assert!(app.game_rename.reload_pending);

    // A failed re-check never claims verification and saves nothing.
    let job = app.activity.queue(
        "Re-checking the renamed game",
        super::super::Route::Game(7),
        false,
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    app.game_rename.reload_pending = false;
    app.game_rename.recheck = Some(super::Recheck {
        job,
        archive_id: Some(7),
        receiver,
    });
    sender.send(Err("database is busy".into())).unwrap();
    app.poll_rename_recheck();
    let line = app.game_rename.recheck_line.clone().unwrap();
    assert!(
        line.contains("not verified until you re-check it"),
        "{line}"
    );
    assert!(!app.game_rename.reload_pending);
}

/// The library's games, as the Game Details page holds them.
fn games(database: &Path) -> Vec<super::super::library::Game> {
    archivefs_core::Database::open_read_only(database)
        .unwrap()
        .load_archives()
        .unwrap()
        .into_iter()
        .map(super::super::library::Game::from_archive)
        .collect()
}

fn found(stem: &str) -> Option<super::Suggestion> {
    Some(super::Suggestion {
        stem: stem.into(),
        catalogue: "no-intro-nes".into(),
    })
}

/// The card opened for `game` with its lookup still running; the returned
/// sender answers it.
fn opened_waiting(
    card: &mut State,
    game: &super::super::library::Game,
) -> std::sync::mpsc::Sender<Option<super::Suggestion>> {
    card.open(game);
    let (sender, receiver) = std::sync::mpsc::channel();
    card.suggestion = Some(receiver);
    sender
}

#[test]
fn the_verified_name_fills_an_untouched_field_and_is_labelled() {
    let temp = tempfile::tempdir().unwrap();
    let (database, _game, _id) = library(temp.path());
    let mut card = state(temp.path(), database.clone());
    let game = &games(&database)[0];
    let answer = opened_waiting(&mut card, game);
    assert_eq!(card.typed, "Old");
    card.poll_suggestion();
    assert_eq!(card.typed, "Old", "nothing changes while the lookup runs");
    answer.send(found("Zelda (USA)")).unwrap();
    card.poll_suggestion();
    assert_eq!(card.typed, "Zelda (USA)");
    assert_eq!(card.suggested_from.as_deref(), Some("no-intro-nes"));
    // The file's own extension stays: the preview names Zelda (USA).nes.
    card.preview_now(&game.archive.absolute_path);
    match &card.preview {
        Some(Ok(preview)) => assert_eq!(
            preview.destination.file_name().unwrap().to_string_lossy(),
            "Zelda (USA).nes"
        ),
        other => panic!("{:?}", other.as_ref().map(|r| r.as_ref().err())),
    }
}

#[test]
fn a_name_typed_while_the_lookup_runs_is_never_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let (database, _game, _id) = library(temp.path());
    let mut card = state(temp.path(), database.clone());
    let answer = opened_waiting(&mut card, &games(&database)[0]);
    card.typed = "My Own Name".into();
    answer.send(found("Zelda (USA)")).unwrap();
    card.poll_suggestion();
    assert_eq!(card.typed, "My Own Name");
    assert_eq!(card.suggested_from, None);
    assert!(
        card.suggestion.is_none(),
        "the answer was taken and dropped"
    );
}

#[test]
fn an_answer_for_a_game_no_longer_open_is_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let (database, game_path, _id) = library(temp.path());
    // A second game in the same library.
    let other = game_path.with_file_name("Second.nes");
    std::fs::write(&other, b"NES\x1a second").unwrap();
    {
        let mut db = archivefs_core::Database::open_or_create(&database).unwrap();
        let root = temp.path().join("games");
        let folder = db.list_source_folders().unwrap()[0].id;
        let archive = archivefs_core::Archive::from_path_in_root(&other, &root).unwrap();
        db.upsert_archive(folder, &root, &archive).unwrap();
    }
    let all = games(&database);
    let first = all
        .iter()
        .find(|g| g.archive.absolute_path == game_path)
        .unwrap();
    let second = all
        .iter()
        .find(|g| g.archive.absolute_path == other)
        .unwrap();
    let mut card = state(temp.path(), database.clone());
    let first_answer = opened_waiting(&mut card, first);
    card.open(second);
    // The first game's lookup finishes after the switch.
    let _ = first_answer.send(found("First Game (USA)"));
    card.poll_suggestion();
    assert_eq!(card.typed, "Second");
    assert_eq!(card.suggested_from, None);
    assert!(card.is_open_for(second.archive.id));
}

#[test]
fn no_trustworthy_name_leaves_the_current_one() {
    let temp = tempfile::tempdir().unwrap();
    let (database, _game, _id) = library(temp.path());
    let mut card = state(temp.path(), database.clone());
    let answer = opened_waiting(&mut card, &games(&database)[0]);
    answer.send(None).unwrap();
    card.poll_suggestion();
    assert_eq!(card.typed, "Old");
    assert_eq!(card.suggested_from, None);
    // A lookup whose worker went away also leaves it.
    let answer = opened_waiting(&mut card, &games(&database)[0]);
    drop(answer);
    card.poll_suggestion();
    assert_eq!(card.typed, "Old");
    assert!(card.suggestion.is_none());
}

/// Owner request 2026-11-26 (Check Games C4): the rename field starts with
/// the verified catalogue name, keeping the file's own extension.
mod verified_name_prefill {
    use super::super::{Suggestion, suggested_stem};
    use archivefs_core::VerifiedDatName as V;
    use std::path::Path;

    fn usable(rom: &str, game: &str, member: bool) -> V {
        V::Usable {
            rom_name: rom.into(),
            game_name: game.into(),
            dat_source_id: "no-intro-gbc".into(),
            through_archive_member: member,
        }
    }

    fn stem(dat: V, source: &str) -> Option<String> {
        suggested_stem(dat, Path::new(source)).map(|s| s.stem)
    }

    #[test]
    fn a_loose_file_takes_the_catalogue_file_name() {
        assert_eq!(
            suggested_stem(
                usable(
                    "Ecco the Dolphin (USA).gbc",
                    "Ecco the Dolphin (USA)",
                    false
                ),
                Path::new("/g/ecco.GBC"),
            ),
            Some(Suggestion {
                stem: "Ecco the Dolphin (USA)".into(),
                catalogue: "no-intro-gbc".into(),
            })
        );
    }

    #[test]
    fn archives_and_other_extensions_follow_bulk_rename_and_keep_their_name() {
        // A ZIP holding one exactly verified game takes the game's name
        // (owner zip rule 2026-11-26), in bulk Rename and here alike.
        assert_eq!(
            stem(
                usable("Ecco Jr. (USA).gbc", "Ecco Jr. (USA)", true),
                "/g/ecco.zip"
            ),
            Some("Ecco Jr. (USA)".into())
        );
        // Other containers keep their name.
        assert_eq!(
            stem(
                usable("Ecco Jr. (USA).gbc", "Ecco Jr. (USA)", true),
                "/g/ecco.7z"
            ),
            None
        );
        // A loose file whose catalogue entry has another extension.
        assert_eq!(
            stem(
                usable("Game (Europe).bin", "Game (Europe)", false),
                "/g/game.md"
            ),
            None
        );
        // Agreement with bulk Rename, case by case.
        for (dat, source) in [
            (
                usable(
                    "Ecco the Dolphin (USA).gbc",
                    "Ecco the Dolphin (USA)",
                    false,
                ),
                "/g/e.gbc",
            ),
            (
                usable("Ecco Jr. (USA).gbc", "Ecco Jr. (USA)", true),
                "/g/e.zip",
            ),
            (
                usable("Ecco Jr. (USA).gbc", "Ecco Jr. (USA)", true),
                "/g/e.7z",
            ),
            (
                usable("Game (Europe).bin", "Game (Europe)", false),
                "/g/game.md",
            ),
        ] {
            let bulk =
                crate::rom_organisation_page::choose_name(dat.clone(), None, Path::new(source))
                    .name
                    .map(|name| {
                        Path::new(&name)
                            .file_stem()
                            .unwrap()
                            .to_string_lossy()
                            .into_owned()
                    });
            assert_eq!(stem(dat, source), bulk, "{source}");
        }
    }

    #[test]
    fn no_verified_name_or_an_unsafe_one_leaves_the_current_name() {
        assert_eq!(stem(V::None, "/g/x.gbc"), None);
        assert_eq!(stem(V::NotUsable("stale".into()), "/g/x.gbc"), None);
        // What core returns when catalogues disagree or name several
        // entries (core test `the_rule_refuses_ambiguity_disagreement_and_missing_names`).
        assert_eq!(
            stem(
                V::NotUsable("the catalogues disagree on this game's name".into()),
                "/g/x.gbc"
            ),
            None
        );
        assert_eq!(
            stem(
                V::NotUsable("a catalogue check found more than one possible match".into()),
                "/g/x.gbc"
            ),
            None
        );
        assert_eq!(stem(usable("a\\b.gbc", "a\\b", false), "/g/x.gbc"), None);
        assert_eq!(
            stem(usable("ctl\u{1b}.gbc", "ctl\u{1b}", false), "/g/x.gbc"),
            None
        );
        assert_eq!(stem(usable(" .gbc", " ", false), "/g/x.gbc"), None);
        assert_eq!(stem(usable("a/b.gbc", "a/b", false), "/g/x.gbc"), None);
        assert_eq!(
            stem(usable(".hidden.gbc", ".hidden", false), "/g/x.gbc"),
            None
        );
    }
}
