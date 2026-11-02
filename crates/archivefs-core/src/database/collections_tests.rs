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

fn favourites(database: &Database) -> Vec<(i64, Option<i64>)> {
    database.load_game_collections().unwrap()[0]
        .items
        .iter()
        .map(|item| (item.stored_archive_id, item.resolved_archive_id))
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
fn a_forgotten_row_found_again_at_the_same_place_and_size_reattaches() {
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
fn a_row_that_took_the_same_id_at_the_same_place_is_saved_against() {
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
    assert_eq!(item.resolved_archive_id, Some(id));
    assert_eq!(item.resolved_created_at.as_deref(), Some("rescanned"));
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
