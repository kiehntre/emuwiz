//! Favourites and user-created collections: which games a person keeps
//! together. Membership is user data over catalogue rows. It never changes a
//! row, a file, or any identity evidence.
//!
//! An item is resolved to a catalogue row, strongest first:
//!
//! 1. the same row: same id and same `created_at`, whatever its path is now
//!    (organisation moves update a row in place);
//! 2. the row now at the saved source and relative path, with the same size
//!    when both are known (a forgotten row that a later scan found again).
//!
//! Anything else is reported as no longer in the library, with its saved title
//! and platform. It is kept until the person removes it: an unavailable drive
//! or a Forget must not silently lose a choice, and a recycled id or a matching
//! title must not attach it to a different game.

use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use rusqlite::{OptionalExtension, params};

use super::{ArchiveFsError, Database, Result, db_error, now_utc_string};

pub const MAX_COLLECTION_NAME_CHARS: usize = 60;
const FAVOURITES_KIND: &str = "favourites";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameCollectionKind {
    Favourites,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameCollectionItem {
    /// The catalogue id this item was saved against.
    pub stored_archive_id: i64,
    pub stored_created_at: String,
    /// The catalogue row it resolves to now; `None`: no longer in the library.
    pub resolved_archive_id: Option<i64>,
    /// That row's creation time. Differs from `stored_created_at` for a row
    /// that took over the saved id, which the id alone cannot show.
    pub resolved_created_at: Option<String>,
    pub title: String,
    pub platform: Option<String>,
    pub absolute_path: PathBuf,
    pub added_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameCollection {
    pub id: i64,
    pub kind: GameCollectionKind,
    pub name: String,
    pub items: Vec<GameCollectionItem>,
}

/// An item as saved. The id alone is not enough: a saved game that left the
/// library and a current game can hold the same recycled id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CollectionItemKey {
    pub archive_id: i64,
    pub created_at: String,
}

/// Move an item saved against one row onto the row that replaced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionReanchor {
    pub stored_archive_id: i64,
    pub stored_created_at: String,
    pub current_archive_id: i64,
}

/// The name as stored, or why it cannot be used.
pub fn validate_collection_name(name: &str) -> Result<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(ArchiveFsError::Database(
            "A collection needs a name.".into(),
        ));
    }
    if name.chars().count() > MAX_COLLECTION_NAME_CHARS {
        return Err(ArchiveFsError::Database(format!(
            "A collection name can be up to {MAX_COLLECTION_NAME_CHARS} characters."
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(ArchiveFsError::Database(
            "A collection name cannot contain control characters.".into(),
        ));
    }
    Ok(name)
}

fn name_key(name: &str) -> String {
    name.to_lowercase()
}

fn map_name_error(error: rusqlite::Error, name: &str) -> ArchiveFsError {
    if matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ConstraintViolation)
    ) {
        return ArchiveFsError::Database(format!("A collection called {name} already exists."));
    }
    db_error("failed to save the collection", error)
}

struct ArchiveAnchor {
    created_at: String,
    source_folder_id: i64,
    relative_path: Vec<u8>,
    absolute_path: Vec<u8>,
    size_bytes: Option<i64>,
    title: String,
    platform: Option<String>,
}

fn archive_anchor(
    connection: &rusqlite::Connection,
    archive_id: i64,
) -> Result<Option<ArchiveAnchor>> {
    connection
        .query_row(
            "SELECT a.created_at, a.source_folder_id, a.relative_path, a.absolute_path_cached, \
                    a.size_bytes, a.display_name, \
                    (SELECT p.platform FROM platform_assignments p \
                     WHERE p.archive_id = a.id AND p.is_current = 1 LIMIT 1) \
             FROM archives a WHERE a.id = ?1",
            [archive_id],
            |row| {
                Ok(ArchiveAnchor {
                    created_at: row.get(0)?,
                    source_folder_id: row.get(1)?,
                    relative_path: row.get(2)?,
                    absolute_path: row.get(3)?,
                    size_bytes: row.get(4)?,
                    title: row.get(5)?,
                    platform: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|error| db_error("failed to read the game", error))
}

impl Database {
    /// Every collection, Favourites first, then by name. Read-only.
    pub fn load_game_collections(&self) -> Result<Vec<GameCollection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, kind, name FROM game_collections \
                 ORDER BY kind = 'user', name_key",
            )
            .map_err(|error| db_error("failed to read collections", error))?;
        let mut collections = statement
            .query_map([], |row| {
                let kind: String = row.get(1)?;
                Ok(GameCollection {
                    id: row.get(0)?,
                    kind: if kind == FAVOURITES_KIND {
                        GameCollectionKind::Favourites
                    } else {
                        GameCollectionKind::User
                    },
                    name: row.get(2)?,
                    items: Vec::new(),
                })
            })
            .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
            .map_err(|error| db_error("failed to read collections", error))?;
        let mut items = self
            .connection
            .prepare(
                "SELECT i.collection_id, i.archive_id, i.archive_created_at, i.title, i.platform, \
                        i.absolute_path, i.added_at, \
                        (SELECT a.id FROM archives a \
                         WHERE a.id = i.archive_id AND a.created_at = i.archive_created_at), \
                        (SELECT a.id FROM archives a \
                         WHERE a.source_folder_id = i.source_folder_id \
                           AND a.relative_path = i.relative_path \
                           AND (a.size_bytes IS NULL OR i.size_bytes IS NULL \
                                OR a.size_bytes = i.size_bytes)), \
                        (SELECT a.created_at FROM archives a \
                         WHERE a.source_folder_id = i.source_folder_id \
                           AND a.relative_path = i.relative_path \
                           AND (a.size_bytes IS NULL OR i.size_bytes IS NULL \
                                OR a.size_bytes = i.size_bytes)) \
                 FROM game_collection_items i ORDER BY i.collection_id, i.added_at, i.archive_id",
            )
            .map_err(|error| db_error("failed to read collection games", error))?;
        let rows = items
            .query_map([], |row| {
                let same: Option<i64> = row.get(7)?;
                let located: Option<i64> = row.get(8)?;
                let located_created_at: Option<String> = row.get(9)?;
                let path: Vec<u8> = row.get(5)?;
                let stored_created_at: String = row.get(2)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    GameCollectionItem {
                        stored_archive_id: row.get(1)?,
                        resolved_archive_id: same.or(located),
                        resolved_created_at: if same.is_some() {
                            Some(stored_created_at.clone())
                        } else {
                            located_created_at
                        },
                        stored_created_at,
                        title: row.get(3)?,
                        platform: row.get(4)?,
                        absolute_path: std::ffi::OsString::from_vec(path).into(),
                        added_at: row.get(6)?,
                    },
                ))
            })
            .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
            .map_err(|error| db_error("failed to read collection games", error))?;
        for (collection_id, item) in rows {
            if let Some(collection) = collections
                .iter_mut()
                .find(|collection| collection.id == collection_id)
            {
                collection.items.push(item);
            }
        }
        Ok(collections)
    }

    pub fn create_game_collection(&mut self, name: &str) -> Result<i64> {
        let name = validate_collection_name(name)?;
        let now = now_utc_string();
        self.connection
            .execute(
                "INSERT INTO game_collections (kind, name, name_key, created_at, updated_at) \
                 VALUES ('user', ?1, ?2, ?3, ?3)",
                params![name, name_key(&name), now],
            )
            .map_err(|error| map_name_error(error, &name))?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Renames a user collection. Favourites keeps its name.
    pub fn rename_game_collection(&mut self, collection_id: i64, name: &str) -> Result<()> {
        let name = validate_collection_name(name)?;
        let changed = self
            .connection
            .execute(
                "UPDATE game_collections SET name = ?2, name_key = ?3, updated_at = ?4 \
                 WHERE id = ?1 AND kind = 'user'",
                params![collection_id, name, name_key(&name), now_utc_string()],
            )
            .map_err(|error| map_name_error(error, &name))?;
        if changed == 0 {
            return Err(ArchiveFsError::Database(
                "That collection cannot be renamed.".into(),
            ));
        }
        Ok(())
    }

    /// Deletes a user collection and its membership. Games are not touched.
    /// Favourites cannot be deleted.
    pub fn delete_game_collection(&mut self, collection_id: i64) -> Result<()> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM game_collections WHERE id = ?1 AND kind = 'user'",
                [collection_id],
            )
            .map_err(|error| db_error("failed to delete the collection", error))?;
        if changed == 0 {
            return Err(ArchiveFsError::Database(
                "That collection cannot be deleted.".into(),
            ));
        }
        Ok(())
    }

    /// The Favourites collection's id.
    pub fn favourites_collection_id(&self) -> Result<i64> {
        self.connection
            .query_row(
                "SELECT id FROM game_collections WHERE kind = 'favourites'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| db_error("failed to find Favourites", error))
    }

    /// Adds one game. Adding a game that is already there changes nothing.
    pub fn add_game_to_collection(&mut self, collection_id: i64, archive_id: i64) -> Result<()> {
        let anchor = archive_anchor(&self.connection, archive_id)?
            .ok_or_else(|| ArchiveFsError::Database("That game is no longer listed.".into()))?;
        self.connection
            .execute(
                "INSERT OR IGNORE INTO game_collection_items \
                 (collection_id, archive_id, archive_created_at, source_folder_id, relative_path, \
                  absolute_path, size_bytes, title, platform, added_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    collection_id,
                    archive_id,
                    anchor.created_at,
                    anchor.source_folder_id,
                    anchor.relative_path,
                    anchor.absolute_path,
                    anchor.size_bytes,
                    anchor.title,
                    anchor.platform,
                    now_utc_string(),
                ],
            )
            .map_err(|error| db_error("failed to add the game", error))?;
        Ok(())
    }

    /// Removes exactly these saved items.
    pub fn remove_items_from_collection(
        &mut self,
        collection_id: i64,
        items: &[CollectionItemKey],
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction()
            .map_err(|error| db_error("failed to remove the game", error))?;
        for item in items {
            tx.execute(
                "DELETE FROM game_collection_items \
                 WHERE collection_id = ?1 AND archive_id = ?2 AND archive_created_at = ?3",
                params![collection_id, item.archive_id, item.created_at],
            )
            .map_err(|error| db_error("failed to remove the game", error))?;
        }
        tx.commit()
            .map_err(|error| db_error("failed to remove the game", error))
    }

    /// Removes the item saved against this game's current row, however recent:
    /// a click can reach here before its add's saved form has been read back.
    pub fn remove_current_game_from_collection(
        &mut self,
        collection_id: i64,
        archive_id: i64,
    ) -> Result<()> {
        self.connection
            .execute(
                "DELETE FROM game_collection_items WHERE collection_id = ?1 AND archive_id = ?2 \
                 AND archive_created_at = (SELECT created_at FROM archives WHERE id = ?2)",
                params![collection_id, archive_id],
            )
            .map_err(|error| db_error("failed to remove the game", error))?;
        Ok(())
    }

    /// Saves each item against the row that replaced its old one, so the
    /// membership keeps working after the old row is forgotten. An item that
    /// no longer has the expected anchor is left alone, and an item whose
    /// target is already in the same collection merges into it. The whole
    /// change is one transaction.
    pub fn reanchor_collection_items(&mut self, moves: &[CollectionReanchor]) -> Result<()> {
        let error = |error| db_error("failed to update saved games", error);
        let tx = self.connection.transaction().map_err(error)?;
        for item in moves {
            let Some(anchor) = archive_anchor(&tx, item.current_archive_id)? else {
                continue;
            };
            // Merge first: the same target row already being in a collection
            // would otherwise collide with the key below. A gone item that only
            // shares a recycled id is a different item and is left alone.
            tx.execute(
                "DELETE FROM game_collection_items \
                 WHERE archive_id = ?1 AND archive_created_at = ?2 \
                   AND NOT (?1 = ?3 AND ?2 = ?4) AND collection_id IN \
                   (SELECT collection_id FROM game_collection_items \
                    WHERE archive_id = ?3 AND archive_created_at = ?4)",
                params![
                    item.stored_archive_id,
                    item.stored_created_at,
                    item.current_archive_id,
                    anchor.created_at
                ],
            )
            .map_err(error)?;
            tx.execute(
                "UPDATE game_collection_items SET archive_id = ?3, archive_created_at = ?4, \
                        source_folder_id = ?5, relative_path = ?6, absolute_path = ?7, \
                        size_bytes = ?8, title = ?9, platform = ?10 \
                 WHERE archive_id = ?1 AND archive_created_at = ?2",
                params![
                    item.stored_archive_id,
                    item.stored_created_at,
                    item.current_archive_id,
                    anchor.created_at,
                    anchor.source_folder_id,
                    anchor.relative_path,
                    anchor.absolute_path,
                    anchor.size_bytes,
                    anchor.title,
                    anchor.platform,
                ],
            )
            .map_err(error)?;
        }
        tx.commit().map_err(error)
    }
}
