use std::fs;
use std::path::PathBuf;

use super::{CollectionReanchor, Database, GameCollectionKind, scan_and_persist};
use crate::Config;

struct Fixture {
    _temp: tempfile::TempDir,
    source: PathBuf,
    db_path: PathBuf,
    config: Config,
    database: Database,
}

fn fixture(files: &[&str]) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(&source).unwrap();
    for file in files {
        fs::write(source.join(file), file.as_bytes()).unwrap();
    }
    let db_path = temp.path().join("library.sqlite3");
    let config = Config {
        source_folders: vec![source.clone()],
        mount_root: temp.path().join("mount"),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
        configured_source_roots: None,
    };
    let mut database = Database::open_or_create(&db_path).unwrap();
    scan_and_persist(&mut database, &config, "test").unwrap();
    Fixture {
        _temp: temp,
        source,
        db_path,
        config,
        database,
    }
}

fn id_of(database: &Database, name: &str) -> i64 {
    database
        .load_archives()
        .unwrap()
        .iter()
        .find(|archive| archive.absolute_path.ends_with(name))
        .unwrap()
        .id
}

/// Each item and the row it points at: its own row, or else the row proposed
/// for it (see `proposed_rows` for which).
fn favourites(database: &Database) -> Vec<(i64, Option<i64>)> {
    database.load_game_collections().unwrap()[0]
        .items
        .iter()
        .map(|item| {
            (
                item.stored_archive_id,
                item.resolved_archive_id.or(item.proposed_archive_id),
            )
        })
        .collect()
}

/// Items whose row is only proposed: their own row is gone.
fn proposed_rows(database: &Database) -> Vec<(i64, Option<i64>, Option<i64>)> {
    database.load_game_collections().unwrap()[0]
        .items
        .iter()
        .map(|item| {
            (
                item.stored_archive_id,
                item.resolved_archive_id,
                item.proposed_archive_id,
            )
        })
        .collect()
}

#[test]
fn favourites_exists_from_the_start_and_cannot_be_renamed_or_deleted() {
    let mut f = fixture(&[]);
    let collections = f.database.load_game_collections().unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].kind, GameCollectionKind::Favourites);
    let id = f.database.favourites_collection_id().unwrap();
    assert!(
        f.database
            .rename_game_collection(id, "Favourites", "Other")
            .is_err()
    );
    assert!(f.database.delete_game_collection(id, "Favourites").is_err());
    assert_eq!(f.database.load_game_collections().unwrap().len(), 1);
}

#[test]
fn names_are_tidied_unique_ignoring_case_and_bounded() {
    let mut f = fixture(&[]);
    let id = f
        .database
        .create_game_collection("  Couch   co-op ")
        .unwrap();
    let collections = f.database.load_game_collections().unwrap();
    assert_eq!(collections[1].name, "Couch co-op");
    assert!(f.database.create_game_collection("COUCH CO-OP").is_err());
    assert!(f.database.create_game_collection("favourites").is_err());
    assert!(f.database.create_game_collection("   ").is_err());
    assert!(f.database.create_game_collection(&"x".repeat(61)).is_err());
    f.database
        .rename_game_collection(id, "Couch co-op", "couch CO-OP")
        .unwrap();
    assert_eq!(
        f.database.load_game_collections().unwrap()[1].name,
        "couch CO-OP"
    );
}

#[test]
fn adding_twice_is_one_item_and_a_listed_game_is_required() {
    let mut f = fixture(&["a.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, "a.zip");
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    assert_eq!(favourites(&f.database), vec![(id, Some(id))]);
    assert!(
        f.database
            .add_game_to_collection(favourites_id, id + 1000)
            .is_err()
    );
    f.database
        .remove_current_game_from_collection(favourites_id, id)
        .unwrap();
    assert!(favourites(&f.database).is_empty());
}

#[test]
fn deleting_a_collection_leaves_games_and_other_collections_alone() {
    let mut f = fixture(&["a.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, "a.zip");
    let user = f.database.create_game_collection("Mine").unwrap();
    f.database.add_game_to_collection(user, id).unwrap();
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    f.database.delete_game_collection(user, "Mine").unwrap();
    assert_eq!(f.database.load_archives().unwrap().len(), 1);
    assert_eq!(favourites(&f.database), vec![(id, Some(id))]);
}

#[test]
fn membership_survives_a_rescan_a_metadata_update_and_a_reopen() {
    let mut f = fixture(&["a.zip", "b.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, "a.zip");
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    scan_and_persist(&mut f.database, &f.config, "again").unwrap();
    assert_eq!(favourites(&f.database), vec![(id, Some(id))]);
    f.database
        .connection
        .execute(
            "UPDATE archives SET display_name='Better' WHERE id=?1",
            [id],
        )
        .unwrap();
    drop(f.database);
    let reopened = Database::open_read_only(&f.db_path).unwrap();
    assert_eq!(favourites(&reopened), vec![(id, Some(id))]);
}

#[test]
fn a_file_gone_from_disk_stays_a_member_and_comes_back_with_its_row() {
    let mut f = fixture(&["a.zip", "b.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, "a.zip");
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    let file = f.source.join("a.zip");
    let held = fs::read(&file).unwrap();
    fs::remove_file(&file).unwrap();
    scan_and_persist(&mut f.database, &f.config, "gone").unwrap();
    assert_eq!(favourites(&f.database), vec![(id, Some(id))]);
    fs::write(&file, held).unwrap();
    scan_and_persist(&mut f.database, &f.config, "back").unwrap();
    assert_eq!(favourites(&f.database), vec![(id, Some(id))]);
}

#[test]
fn an_organisation_move_keeps_the_row_and_so_the_membership() {
    let mut f = fixture(&["a.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, "a.zip");
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    f.database
        .connection
        .execute(
            "UPDATE archives SET relative_path=x'6d6f7665642e7a6970' WHERE id=?1",
            [id],
        )
        .unwrap();
    assert_eq!(favourites(&f.database), vec![(id, Some(id))]);
}

#[test]
fn a_recycled_id_never_adopts_a_membership_and_the_saved_title_remains() {
    let mut f = fixture(&["a.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, "a.zip");
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    f.database
        .connection
        .execute_batch(
            "DELETE FROM archive_scan_observations; DELETE FROM platform_assignments; \
             DELETE FROM scan_fingerprints; DELETE FROM archives;",
        )
        .unwrap();
    // A different game at a different path takes the same id, as the scanner's
    // max(id)+1 allocation does once the highest row has been forgotten.
    f.database
        .connection
        .execute(
            "INSERT INTO archives (id, source_folder_id, relative_path, absolute_path_cached, \
              file_name_cached, archive_kind, display_name, normalized_name, first_seen_at, \
              last_seen_at, created_at, updated_at) \
             SELECT ?1, id, x'62', x'62', x'62', 'zip', 'Other', 'other', 'later', 'later', \
              'later', 'later' FROM source_folders LIMIT 1",
            [id],
        )
        .unwrap();
    let items = &f.database.load_game_collections().unwrap()[0].items;
    assert_eq!(items[0].resolved_archive_id, None);
    assert_eq!(items[0].title, "a");
    assert!(items[0].absolute_path.ends_with("a.zip"));
}

#[test]
fn a_forgotten_row_found_again_at_the_same_place_and_size_is_proposed() {
    let mut f = fixture(&["a.zip", "z.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, "a.zip");
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    // Forget and a later scan: the same place under a new id and created_at.
    f.database
        .connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    f.database
        .connection
        .execute(
            "UPDATE archives SET id=id+500, created_at='rescanned' WHERE id=?1",
            [id],
        )
        .unwrap();
    let new_id = id_of(&f.database, "a.zip");
    assert_eq!(favourites(&f.database), vec![(id, Some(new_id))]);
    // Proposed only: the saved row itself is gone, and nothing was written.
    assert_eq!(proposed_rows(&f.database), vec![(id, None, Some(new_id))]);
    // A different file at that path is a different game.
    f.database
        .connection
        .execute(
            "UPDATE archives SET size_bytes=999999 WHERE id=?1",
            [new_id],
        )
        .unwrap();
    assert_eq!(favourites(&f.database), vec![(id, None)]);
}

#[test]
fn reanchoring_moves_an_item_merges_a_duplicate_and_skips_a_stale_anchor() {
    let mut f = fixture(&["a.zip", "b.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let user = f.database.create_game_collection("Mine").unwrap();
    let (a, b) = (id_of(&f.database, "a.zip"), id_of(&f.database, "b.zip"));
    f.database.add_game_to_collection(favourites_id, a).unwrap();
    f.database.add_game_to_collection(user, a).unwrap();
    f.database.add_game_to_collection(user, b).unwrap();
    let created = f.database.load_game_collections().unwrap()[0].items[0]
        .stored_created_at
        .clone();
    let stale = CollectionReanchor {
        stored_archive_id: a,
        stored_created_at: "not it".into(),
        current_archive_id: b,
    };
    f.database.reanchor_collection_items(&[stale]).unwrap();
    assert_eq!(favourites(&f.database), vec![(a, Some(a))]);
    f.database
        .reanchor_collection_items(&[CollectionReanchor {
            stored_archive_id: a,
            stored_created_at: created,
            current_archive_id: b,
        }])
        .unwrap();
    let collections = f.database.load_game_collections().unwrap();
    assert_eq!(collections[0].items[0].stored_archive_id, b);
    assert_eq!(collections[0].items.len(), 1);
    assert_eq!(collections[1].items.len(), 1);
    assert_eq!(collections[1].items[0].stored_archive_id, b);
}

/// Item X was saved against an id, its row left, and a new game took the id.
fn recycled(f: &mut Fixture, name: &str) -> (i64, i64) {
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, name);
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    f.database
        .connection
        .execute(
            "UPDATE archives SET created_at='recycled', relative_path=x'6e6577', size_bytes=1 \
             WHERE id=?1",
            [id],
        )
        .unwrap();
    assert_eq!(favourites(&f.database), vec![(id, None)]);
    (favourites_id, id)
}

#[test]
fn a_gone_item_on_a_recycled_id_does_not_stop_the_new_game_being_starred() {
    let mut f = fixture(&["a.zip"]);
    let (favourites_id, id) = recycled(&mut f, "a.zip");
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    let mut items = favourites(&f.database);
    items.sort();
    assert_eq!(items, vec![(id, None), (id, Some(id))]);
    // Removing the current game leaves the gone item's record alone.
    f.database
        .remove_current_game_from_collection(favourites_id, id)
        .unwrap();
    assert_eq!(favourites(&f.database), vec![(id, None)]);
}

#[test]
fn reanchoring_onto_a_recycled_id_keeps_both_items() {
    let mut f = fixture(&["a.zip", "b.zip"]);
    let (favourites_id, target) = recycled(&mut f, "a.zip");
    let live = id_of(&f.database, "b.zip");
    f.database
        .add_game_to_collection(favourites_id, live)
        .unwrap();
    let created = f.database.load_game_collections().unwrap()[0]
        .items
        .iter()
        .find(|item| item.stored_archive_id == live)
        .unwrap()
        .stored_created_at
        .clone();
    f.database
        .reanchor_collection_items(&[CollectionReanchor {
            stored_archive_id: live,
            stored_created_at: created,
            current_archive_id: target,
        }])
        .unwrap();
    let mut items = favourites(&f.database);
    items.sort();
    assert_eq!(items, vec![(target, None), (target, Some(target))]);
}

#[test]
fn a_row_that_took_the_same_id_at_the_same_place_is_proposed_then_saved_on_confirmation() {
    let mut f = fixture(&["a.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let id = id_of(&f.database, "a.zip");
    f.database
        .add_game_to_collection(favourites_id, id)
        .unwrap();
    f.database
        .connection
        .execute(
            "UPDATE archives SET created_at='rescanned' WHERE id=?1",
            [id],
        )
        .unwrap();
    let item = f.database.load_game_collections().unwrap()[0].items[0].clone();
    assert_eq!(item.resolved_archive_id, None, "another creation time");
    assert_eq!(item.proposed_archive_id, Some(id));
    // What the user's confirmation sends.
    f.database
        .reanchor_collection_items(&[CollectionReanchor {
            stored_archive_id: id,
            stored_created_at: item.stored_created_at,
            current_archive_id: id,
        }])
        .unwrap();
    let saved = f.database.load_game_collections().unwrap()[0].items[0].clone();
    assert_eq!(saved.stored_created_at, "rescanned");
}

#[test]
fn a_deleted_collections_id_is_never_given_to_another() {
    let mut f = fixture(&[]);
    let old = f.database.create_game_collection("Old").unwrap();
    f.database.delete_game_collection(old, "Old").unwrap();
    let new = f.database.create_game_collection("Keep").unwrap();
    assert_ne!(new, old);
    // A second click from a window still showing "Old" reaches nothing.
    assert!(f.database.delete_game_collection(old, "Old").is_err());
    assert_eq!(f.database.load_game_collections().unwrap().len(), 2);
}

#[test]
fn a_collection_renamed_since_it_was_seen_is_left_alone() {
    let mut f = fixture(&[]);
    let id = f.database.create_game_collection("First").unwrap();
    f.database
        .rename_game_collection(id, "First", "Second")
        .unwrap();
    assert!(f.database.delete_game_collection(id, "First").is_err());
    assert!(
        f.database
            .rename_game_collection(id, "First", "Third")
            .is_err()
    );
    assert_eq!(
        f.database.load_game_collections().unwrap()[1].name,
        "Second"
    );
}

/// The scanner hands out max(id)+1. Once the highest row is forgotten, a new
/// game must not be given the id a membership still names: within one second
/// it could even share that row's created_at.
#[test]
fn the_scanner_never_reuses_an_id_a_membership_still_names() {
    let mut f = fixture(&["a.zip", "z.zip"]);
    let favourites_id = f.database.favourites_collection_id().unwrap();
    let highest = f
        .database
        .load_archives()
        .unwrap()
        .iter()
        .map(|archive| archive.id)
        .max()
        .unwrap();
    f.database
        .add_game_to_collection(favourites_id, highest)
        .unwrap();
    f.database
        .connection
        .execute_batch(&format!(
            "DELETE FROM archive_scan_observations WHERE archive_id={highest}; \
             DELETE FROM platform_assignments WHERE archive_id={highest}; \
             DELETE FROM archives WHERE id={highest};"
        ))
        .unwrap();
    // Its file is gone too, so the only new row is a different game.
    let gone = f.database.load_game_collections().unwrap()[0].items[0]
        .absolute_path
        .clone();
    fs::remove_file(&gone).unwrap();
    fs::write(f.source.join("new.zip"), b"a different game").unwrap();
    scan_and_persist(&mut f.database, &f.config, "test").unwrap();
    let new_id = id_of(&f.database, "new.zip");
    assert!(
        new_id > highest,
        "id {highest} named by a favourite was given to another game"
    );
    assert_eq!(favourites(&f.database), vec![(highest, None)]);
}

// ---- Owner clarification 2026-11-15: a completed EmuWiz rename is saved ----
//
// Only EmuWiz's own finished rename record moves an item without asking;
// guesses, failed, undone, ambiguous or stale records change nothing.

fn journals(f: &Fixture) -> PathBuf {
    f.db_path
        .parent()
        .unwrap()
        .join(crate::dat::rename_apply::RENAME_TRANSACTIONS_DIRECTORY)
}

/// Records one rename transaction in `state`, each entry in `entry_state`.
fn record(f: &Fixture, id: u32, state: &str, entry_state: &str, moves: &[(&str, &str)]) {
    let entries: Vec<serde_json::Value> = moves
        .iter()
        .map(|(from, to)| {
            serde_json::json!({
                "source_path": f.source.join(from), "destination_path": f.source.join(to),
                "original_basename": "x", "proposed_basename": "y",
                "identity": {"size_bytes": 1, "modified_unix": 1, "kind": "regular_file"},
                "state": entry_state
            })
        })
        .collect();
    let transaction: crate::dat::rename_apply::model::RenameTransaction =
        serde_json::from_value(serde_json::json!({
            "transaction_id": format!("{id}-0"), "plan_generation": 0, "created_at_unix": id,
            "source_scan_root": f.source, "state": state, "entries": entries
        }))
        .unwrap();
    crate::dat::rename_apply::journal::write_journal(&journals(f), &transaction).unwrap();
}

/// Moves the file on disk and rescans: the old row is marked missing and a
/// new row stands at the destination, as after a real Organise rename.
fn rename_on_disk(f: &mut Fixture, from: &str, to: &str) -> i64 {
    if let Some(parent) = f.source.join(to).parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::rename(f.source.join(from), f.source.join(to)).unwrap();
    scan_and_persist(&mut f.database, &f.config, "after rename").unwrap();
    id_by_path(&f.database, to)
}

fn id_by_path(database: &Database, relative: &str) -> i64 {
    database
        .load_archives()
        .unwrap()
        .iter()
        .find(|archive| {
            archive.relative_path == std::path::Path::new(relative)
                && archive.last_verified_missing_at.is_none()
        })
        .unwrap()
        .id
}

fn stored_ids(database: &Database) -> Vec<Vec<i64>> {
    database
        .load_game_collections()
        .unwrap()
        .iter()
        .map(|collection| {
            let mut ids: Vec<i64> = collection
                .items
                .iter()
                .map(|item| item.stored_archive_id)
                .collect();
            ids.sort();
            ids
        })
        .collect()
}

#[test]
fn a_completed_rename_saves_favourites_and_collections_once_and_survives_a_restart() {
    let mut f = fixture(&["a.zip", "z.zip"]);
    let favourites = f.database.favourites_collection_id().unwrap();
    let mine = f.database.create_game_collection("Mine").unwrap();
    let old = id_of(&f.database, "a.zip");
    f.database.add_game_to_collection(favourites, old).unwrap();
    f.database.add_game_to_collection(mine, old).unwrap();
    let new = rename_on_disk(&mut f, "a.zip", "a (Renamed).zip");
    record(&f, 1, "applied", "applied", &[("a.zip", "a (Renamed).zip")]);
    assert_eq!(stored_ids(&f.database), vec![vec![old], vec![old]]);

    let dir = journals(&f);
    assert_eq!(
        f.database.save_journalled_collection_moves(&dir).unwrap(),
        super::JournalledMoves {
            moved: 1,
            paused: None
        },
        "a normal run reports no pause"
    );
    assert_eq!(stored_ids(&f.database), vec![vec![new], vec![new]]);
    let items = f.database.load_game_collections().unwrap();
    assert!(
        items
            .iter()
            .all(|c| c.items[0].resolved_archive_id == Some(new))
    );

    // Idempotent: nothing left to do, and the file is not even written.
    let before = fs::read(&f.db_path).unwrap();
    assert_eq!(
        f.database
            .save_journalled_collection_moves(&dir)
            .unwrap()
            .moved,
        0
    );
    assert_eq!(fs::read(&f.db_path).unwrap(), before);
    // A restart reads the saved move.
    let reopened = Database::open_existing(&f.db_path).unwrap();
    assert_eq!(stored_ids(&reopened), vec![vec![new], vec![new]]);
}

#[test]
fn a_completed_rename_still_applies_after_the_old_entry_was_cleaned_up() {
    let mut f = fixture(&["a.zip"]);
    let favourites = f.database.favourites_collection_id().unwrap();
    let old = id_of(&f.database, "a.zip");
    f.database.add_game_to_collection(favourites, old).unwrap();
    let new = rename_on_disk(&mut f, "a.zip", "moved/a.zip");
    record(&f, 1, "applied", "applied", &[("a.zip", "moved/a.zip")]);
    f.database
        .connection
        .execute_batch(&format!(
            "PRAGMA foreign_keys=OFF; DELETE FROM archives WHERE id={old};"
        ))
        .unwrap();
    assert_eq!(
        f.database
            .save_journalled_collection_moves(&journals(&f))
            .unwrap()
            .moved,
        1
    );
    assert_eq!(stored_ids(&f.database), vec![vec![new]]);
}

#[test]
fn only_the_renamed_one_of_two_same_named_games_moves() {
    let mut f = fixture(&[]);
    for folder in ["x", "y"] {
        fs::create_dir_all(f.source.join(folder)).unwrap();
        fs::write(f.source.join(folder).join("a.zip"), folder).unwrap();
    }
    scan_and_persist(&mut f.database, &f.config, "two").unwrap();
    let favourites = f.database.favourites_collection_id().unwrap();
    let x = id_by_path(&f.database, "x/a.zip");
    let y = id_by_path(&f.database, "y/a.zip");
    f.database.add_game_to_collection(favourites, x).unwrap();
    f.database.add_game_to_collection(favourites, y).unwrap();
    let new = rename_on_disk(&mut f, "x/a.zip", "x/b.zip");
    record(&f, 1, "applied", "applied", &[("x/a.zip", "x/b.zip")]);
    f.database
        .save_journalled_collection_moves(&journals(&f))
        .unwrap();
    let mut expected = vec![new, y];
    expected.sort();
    assert_eq!(stored_ids(&f.database), vec![expected]);
}

#[test]
fn failed_undone_unfinished_ambiguous_or_stale_records_change_nothing() {
    type Setup = fn(&Fixture);
    let cases: [(&str, Setup); 8] = [
        ("undone", |f| {
            record(f, 1, "rolled_back", "rolled_back", &[("a.zip", "b.zip")])
        }),
        ("failed", |f| {
            record(f, 1, "apply_failed", "applied", &[("a.zip", "b.zip")])
        }),
        ("interrupted", |f| {
            record(f, 1, "applying", "applied", &[("a.zip", "b.zip")])
        }),
        ("rollback failed", |f| {
            record(f, 1, "rollback_failed", "applied", &[("a.zip", "b.zip")])
        }),
        ("planned only", |f| {
            record(f, 1, "planned", "planned", &[("a.zip", "b.zip")])
        }),
        ("two destinations", |f| {
            record(f, 1, "applied", "applied", &[("a.zip", "b.zip")]);
            record(f, 2, "applied", "applied", &[("a.zip", "c.zip")]);
        }),
        ("a later unfinished move of the destination", |f| {
            record(f, 1, "applied", "applied", &[("a.zip", "b.zip")]);
            record(f, 2, "applying", "applied", &[("b.zip", "d.zip")]);
        }),
        ("two sources onto one file", |f| {
            record(f, 1, "applied", "applied", &[("a.zip", "b.zip")]);
            record(f, 2, "applied", "applied", &[("z.zip", "b.zip")]);
        }),
    ];
    for (name, setup) in cases {
        let mut f = fixture(&["a.zip", "z.zip"]);
        let favourites = f.database.favourites_collection_id().unwrap();
        let old = id_of(&f.database, "a.zip");
        f.database.add_game_to_collection(favourites, old).unwrap();
        rename_on_disk(&mut f, "a.zip", "b.zip");
        setup(&f);
        let before = fs::read(&f.db_path).unwrap();
        assert_eq!(
            f.database
                .save_journalled_collection_moves(&journals(&f))
                .unwrap()
                .moved,
            0,
            "{name}"
        );
        assert_eq!(
            fs::read(&f.db_path).unwrap(),
            before,
            "{name}: nothing written"
        );
        assert_eq!(stored_ids(&f.database), vec![vec![old]], "{name}");
    }
}

#[test]
fn a_record_that_disagrees_with_the_catalogue_changes_nothing() {
    // The old row is still present and not missing: the file did not leave.
    let mut f = fixture(&["a.zip", "b.zip"]);
    let favourites = f.database.favourites_collection_id().unwrap();
    let old = id_of(&f.database, "a.zip");
    f.database.add_game_to_collection(favourites, old).unwrap();
    record(&f, 1, "applied", "applied", &[("a.zip", "b.zip")]);
    let dir = journals(&f);
    assert_eq!(
        f.database
            .save_journalled_collection_moves(&dir)
            .unwrap()
            .moved,
        0
    );
    assert_eq!(stored_ids(&f.database), vec![vec![old]]);

    // A different size at the destination is a different file.
    let mut g = fixture(&["a.zip"]);
    let favourites = g.database.favourites_collection_id().unwrap();
    let old = id_of(&g.database, "a.zip");
    g.database.add_game_to_collection(favourites, old).unwrap();
    rename_on_disk(&mut g, "a.zip", "b.zip");
    fs::write(g.source.join("b.zip"), "a different, longer file").unwrap();
    scan_and_persist(&mut g.database, &g.config, "changed").unwrap();
    record(&g, 1, "applied", "applied", &[("a.zip", "b.zip")]);
    assert_eq!(
        g.database
            .save_journalled_collection_moves(&journals(&g))
            .unwrap()
            .moved,
        0
    );
    assert_eq!(stored_ids(&g.database), vec![vec![old]]);
}

#[test]
fn a_guess_by_name_path_or_size_alone_is_never_saved() {
    // Same name and size elsewhere, no rename record: nothing moves.
    let mut f = fixture(&["a.zip"]);
    let favourites = f.database.favourites_collection_id().unwrap();
    let old = id_of(&f.database, "a.zip");
    f.database.add_game_to_collection(favourites, old).unwrap();
    rename_on_disk(&mut f, "a.zip", "elsewhere/a.zip");
    record(
        &f,
        1,
        "applied",
        "applied",
        &[("unrelated.zip", "other.zip")],
    );
    assert_eq!(
        f.database
            .save_journalled_collection_moves(&journals(&f))
            .unwrap()
            .moved,
        0
    );
    assert_eq!(stored_ids(&f.database), vec![vec![old]]);
}

fn legacy(f: &Fixture, id: u32, state: &str, entry_state: &str, moves: &[(&str, &str)]) {
    let entries: Vec<serde_json::Value> = moves
        .iter()
        .map(|(from, to)| {
            serde_json::json!({
                "source_path": f.source.join(from), "destination_path": f.source.join(to),
                "original_basename": "x", "proposed_basename": "y",
                "identity": {"size_bytes": 1, "modified_unix": 1, "kind": "regular_file"},
                "state": entry_state
            })
        })
        .collect();
    let transaction: crate::dat::rename_apply::model::RenameTransaction =
        serde_json::from_value(serde_json::json!({
            "transaction_id": format!("{id}-0"), "plan_generation": 0, "created_at_unix": id,
            "source_scan_root": f.source, "state": state, "entries": entries
        }))
        .unwrap();
    let dir = f
        .db_path
        .parent()
        .unwrap()
        .join(crate::dat::rename_apply::LEGACY_RENAME_TRANSACTIONS_DIRECTORY);
    crate::dat::rename_apply::journal::write_journal(&dir, &transaction).unwrap();
}

/// Owner follow-ups 2026-11-15: an unreadable record pauses every automatic
/// update and says so; older (unsealed) records are never evidence, an
/// unfinished one blocks only the files it touched, and ordinary settled ones
/// neither warn nor block.
#[test]
fn record_problems_pause_or_block_exactly_as_much_as_they_must() {
    type Setup = fn(&Fixture);
    // (case, setup, expect moved, expect a pause)
    let cases: [(&str, Setup, bool, bool); 6] = [
        (
            "unreadable sealed record",
            |f| fs::write(journals(f).join("2-0.json"), b"{ not a journal").unwrap(),
            false,
            true,
        ),
        (
            "unreadable older record",
            |f| {
                let dir = f
                    .db_path
                    .parent()
                    .unwrap()
                    .join(crate::dat::rename_apply::LEGACY_RENAME_TRANSACTIONS_DIRECTORY);
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("9-0.json"), b"{ not a journal").unwrap()
            },
            false,
            true,
        ),
        (
            "older unfinished record on this file",
            |f| legacy(f, 9, "applying", "applying", &[("a.zip", "c.zip")]),
            false,
            false,
        ),
        (
            "older unfinished record elsewhere",
            |f| legacy(f, 9, "applying", "applying", &[("z.zip", "q.zip")]),
            true,
            false,
        ),
        (
            "ordinary settled older records",
            |f| {
                legacy(f, 8, "applied", "applied", &[("z.zip", "q.zip")]);
                legacy(f, 9, "rolled_back", "rolled_back", &[("z.zip", "r.zip")]);
            },
            true,
            false,
        ),
        ("no problem at all", |_| {}, true, false),
    ];
    for (case, setup, moves, paused) in cases {
        let mut f = fixture(&["a.zip", "z.zip"]);
        let favourites = f.database.favourites_collection_id().unwrap();
        let old = id_of(&f.database, "a.zip");
        f.database.add_game_to_collection(favourites, old).unwrap();
        let new = rename_on_disk(&mut f, "a.zip", "b.zip");
        record(&f, 1, "applied", "applied", &[("a.zip", "b.zip")]);
        setup(&f);
        let before = fs::read(&f.db_path).unwrap();
        let result = f
            .database
            .save_journalled_collection_moves(&journals(&f))
            .unwrap();
        assert_eq!(result.paused.is_some(), paused, "{case}: {result:?}");
        assert_eq!(result.moved, usize::from(moves), "{case}");
        let expected = if moves { new } else { old };
        assert_eq!(stored_ids(&f.database), vec![vec![expected]], "{case}");
        if !moves {
            assert_eq!(
                fs::read(&f.db_path).unwrap(),
                before,
                "{case}: nothing written"
            );
        }
    }
}

#[test]
fn older_unsealed_records_alone_are_never_evidence() {
    let mut f = fixture(&["a.zip"]);
    let favourites = f.database.favourites_collection_id().unwrap();
    let old = id_of(&f.database, "a.zip");
    f.database.add_game_to_collection(favourites, old).unwrap();
    rename_on_disk(&mut f, "a.zip", "b.zip");
    legacy(&f, 1, "applied", "applied", &[("a.zip", "b.zip")]);
    fs::create_dir_all(journals(&f)).unwrap();
    let result = f
        .database
        .save_journalled_collection_moves(&journals(&f))
        .unwrap();
    assert_eq!(
        result,
        super::JournalledMoves::default(),
        "no move, no warning"
    );
    assert_eq!(stored_ids(&f.database), vec![vec![old]]);
}
