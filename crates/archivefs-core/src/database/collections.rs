//! Favourites and user-created collections: which games a person keeps
//! together. Membership is user data over catalogue rows. It never changes a
//! row, a file, or any identity evidence.
//!
//! An item belongs to its own row: same id and same `created_at`, whatever its
//! path is now (organisation moves update a row in place). When that row is
//! gone, the row now at the saved source and relative path, with the same size
//! when both are known, is only *proposed*: the user confirms it or not.
//!
//! The one automatic save is [`Database::save_journalled_collection_moves`]:
//! EmuWiz's own completed rename record proves where the file went.
//!
//! Anything else is reported as no longer in the library, with its saved title
//! and platform. It is kept until the person removes it: an unavailable drive
//! or a Forget must not silently lose a choice, and a recycled id or a matching
//! title must not attach it to a different game.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::dat::rename_apply::journal::list_journals;
use crate::dat::rename_apply::model::{EntryState, TransactionOperation, TransactionState};

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
    /// The saved row itself, when it is still in the catalogue (same id and
    /// creation time); `None`: the saved row is no longer in the library.
    pub resolved_archive_id: Option<i64>,
    /// That row's creation time: always `stored_created_at` when present.
    pub resolved_created_at: Option<String>,
    /// Only when the saved row is gone: a current row at the item's source and
    /// relative path, with the same size when both sizes are known. This is a
    /// proposal, never identity: it is shown for the user to confirm and is
    /// never saved, or treated as membership, without that confirmation.
    pub proposed_archive_id: Option<i64>,
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
                                OR a.size_bytes = i.size_bytes)) \
                 FROM game_collection_items i ORDER BY i.collection_id, i.added_at, i.archive_id",
            )
            .map_err(|error| db_error("failed to read collection games", error))?;
        let rows = items
            .query_map([], |row| {
                let same: Option<i64> = row.get(7)?;
                let located: Option<i64> = row.get(8)?;
                let path: Vec<u8> = row.get(5)?;
                let stored_created_at: String = row.get(2)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    GameCollectionItem {
                        stored_archive_id: row.get(1)?,
                        resolved_archive_id: same,
                        resolved_created_at: same.map(|_| stored_created_at.clone()),
                        proposed_archive_id: if same.is_none() { located } else { None },
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

    /// Renames a user collection, only if it still has the name the user saw.
    /// Favourites keeps its name.
    pub fn rename_game_collection(
        &mut self,
        collection_id: i64,
        seen_name: &str,
        name: &str,
    ) -> Result<()> {
        let name = validate_collection_name(name)?;
        let changed = self
            .connection
            .execute(
                "UPDATE game_collections SET name = ?2, name_key = ?3, updated_at = ?4 \
                 WHERE id = ?1 AND kind = 'user' AND name = ?5",
                params![
                    collection_id,
                    name,
                    name_key(&name),
                    now_utc_string(),
                    seen_name
                ],
            )
            .map_err(|error| map_name_error(error, &name))?;
        if changed == 0 {
            return Err(ArchiveFsError::Database(
                "That collection cannot be renamed.".into(),
            ));
        }
        Ok(())
    }

    /// Deletes a user collection and its membership, only if it still has the
    /// name the user saw. Games are not touched. Favourites cannot be deleted.
    pub fn delete_game_collection(&mut self, collection_id: i64, seen_name: &str) -> Result<()> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM game_collections WHERE id = ?1 AND kind = 'user' AND name = ?2",
                params![collection_id, seen_name],
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
    /// membership keeps working after the old row is forgotten. Call it only
    /// for a move the user confirmed: a match by place and size is a proposal,
    /// never identity, and loading the library writes nothing. An item that
    /// no longer has the expected anchor is left alone, and an item whose
    /// target is already in the same collection merges into it. The whole
    /// change is one transaction.
    pub fn reanchor_collection_items(&mut self, moves: &[CollectionReanchor]) -> Result<()> {
        let error = |error| db_error("failed to update saved games", error);
        let tx = self.connection.transaction().map_err(error)?;
        reanchor_in(&tx, moves)?;
        tx.commit().map_err(error)
    }

    /// Saves, without asking, only the items that EmuWiz's own *completed*
    /// rename record proves were moved: the item's saved file path is the
    /// start of a chain of applied rename-moves in finished transactions,
    /// exactly one current row stands at its end, its size agrees, and the
    /// item's own row (when it still exists) is at the saved path and marked
    /// missing. A path with two destinations, two sources moving onto one
    /// destination, or a part in a transaction that did not finish cleanly is
    /// never followed. File names, paths or sizes alone never move an item.
    /// Nothing is written when nothing qualifies; running it again changes
    /// nothing. When a rename record cannot be trusted at all (see
    /// `completed_rename_ends`) nothing is moved and the result says why.
    pub fn save_journalled_collection_moves(&mut self, journals: &Path) -> Result<JournalledMoves> {
        let renames = match completed_rename_ends(journals) {
            Ok(renames) => renames,
            Err(reason) => {
                return Ok(JournalledMoves {
                    moved: 0,
                    paused: Some(reason),
                });
            }
        };
        if renames.is_empty() || journalled_moves(&self.connection, &renames)?.is_empty() {
            return Ok(JournalledMoves::default());
        }
        let error = |error| db_error("failed to update saved games", error);
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(error)?;
        // Checked again under the write lock.
        let moves = journalled_moves(&tx, &renames)?;
        reanchor_in(&tx, &moves)?;
        tx.commit().map_err(error)?;
        Ok(JournalledMoves {
            moved: moves.len(),
            paused: None,
        })
    }
}

/// What [`Database::save_journalled_collection_moves`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalledMoves {
    /// Saved games moved onto a renamed game's new entry, each updated in
    /// every collection that holds it.
    pub moved: usize,
    /// Set when a rename record could not be trusted, so automatic updates
    /// were paused and nothing was moved; the reason, for the log.
    pub paused: Option<String>,
}

/// Start path -> final path for every chain of rename-moves that EmuWiz's own
/// sealed journals prove: an applied rename-move in a finished transaction
/// (Applied or Finalised) links source to destination, and a rename-move a
/// finished Undo reversed (transaction and entry RolledBack) links destination
/// back to source. Failed, interrupted and planned transactions add no link,
/// and a path any unfinished transaction touched is never followed.
///
/// Journals in the older, unsealed folder beside it are never evidence: they
/// only mark the paths of an unfinished transaction as not to be followed.
///
/// Fails closed: if any journal in either folder cannot be read, the reason is
/// returned and nothing is moved.
fn completed_rename_ends(dir: &Path) -> std::result::Result<HashMap<PathBuf, PathBuf>, String> {
    const MAX_HOPS: usize = 16;
    let (transactions, problems) = list_journals(dir);
    if let Some(problem) = problems.first() {
        return Err(format!("a rename record could not be read: {problem}"));
    }
    let settled = |state: &TransactionState| {
        matches!(
            state,
            TransactionState::Planned
                | TransactionState::Applied
                | TransactionState::RolledBack
                | TransactionState::Finalised
        )
    };
    let mut unsettled: HashSet<PathBuf> = HashSet::new();
    if let Some(legacy) = dir
        .parent()
        .map(|parent| parent.join(crate::dat::rename_apply::LEGACY_RENAME_TRANSACTIONS_DIRECTORY))
        .filter(|legacy| legacy.exists())
    {
        let (old, old_problems) = list_journals(&legacy);
        if let Some(problem) = old_problems.first() {
            return Err(format!(
                "an older rename record could not be read: {problem}"
            ));
        }
        for transaction in old.into_iter().filter(|t| !settled(&t.state)) {
            for entry in transaction.entries {
                unsettled.insert(entry.source_path);
                unsettled.insert(entry.destination_path);
            }
        }
    }
    let mut next: HashMap<PathBuf, BTreeSet<PathBuf>> = HashMap::new();
    let mut into: HashMap<PathBuf, BTreeSet<PathBuf>> = HashMap::new();
    for transaction in transactions {
        let finished = matches!(
            transaction.state,
            TransactionState::Applied | TransactionState::Finalised
        );
        let undone = transaction.state == TransactionState::RolledBack;
        let is_settled = settled(&transaction.state);
        for entry in transaction.entries {
            if !matches!(entry.operation, TransactionOperation::RenameMove) {
                continue;
            }
            let (from, to) = if finished && entry.state == EntryState::Applied {
                (entry.source_path, entry.destination_path)
            } else if undone && entry.state == EntryState::RolledBack {
                (entry.destination_path, entry.source_path)
            } else {
                if !is_settled {
                    unsettled.insert(entry.source_path);
                    unsettled.insert(entry.destination_path);
                }
                continue;
            };
            next.entry(from.clone()).or_default().insert(to.clone());
            into.entry(to).or_default().insert(from);
        }
    }
    let step = |at: &Path| -> Option<PathBuf> {
        let ends = next.get(at)?;
        let end = (ends.len() == 1).then(|| ends.iter().next())??;
        let sources = into.get(end)?;
        (sources.len() == 1 && !unsettled.contains(at) && !unsettled.contains(end))
            .then(|| end.clone())
    };
    let mut ends = HashMap::new();
    for start in next.keys() {
        let mut seen = HashSet::from([start.clone()]);
        let mut at = start.clone();
        let mut ok = true;
        let mut hops = 0;
        while next.contains_key(&at) {
            hops += 1;
            match step(&at) {
                Some(end) if hops <= MAX_HOPS && seen.insert(end.clone()) => at = end,
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && at != *start {
            ends.insert(start.clone(), at);
        }
    }
    Ok(ends)
}

/// The items `renames` proves moved, each onto the one current row at the end
/// of its chain. Read-only.
fn journalled_moves(
    connection: &rusqlite::Connection,
    renames: &HashMap<PathBuf, PathBuf>,
) -> Result<Vec<CollectionReanchor>> {
    let error = |error| db_error("failed to read collection games", error);
    let mut items = connection
        .prepare(
            "SELECT DISTINCT archive_id, archive_created_at, absolute_path, size_bytes \
             FROM game_collection_items",
        )
        .map_err(error)?;
    let rows = items
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
        .map_err(error)?;
    let mut moves = Vec::new();
    for (id, created_at, path, size) in rows {
        let saved = PathBuf::from(std::ffi::OsString::from_vec(path.clone()));
        let Some(end) = renames.get(&saved) else {
            continue;
        };
        // The item's own row, if it still exists, must agree that the file left.
        let own: Option<(Vec<u8>, Option<String>)> = connection
            .query_row(
                "SELECT absolute_path_cached, last_verified_missing_at FROM archives \
                 WHERE id = ?1 AND created_at = ?2",
                params![id, created_at],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(error)?;
        if own.is_some_and(|(own_path, missing)| own_path != path || missing.is_none()) {
            continue;
        }
        let mut found = connection
            .prepare(
                "SELECT id, size_bytes FROM archives \
                 WHERE absolute_path_cached = ?1 AND last_verified_missing_at IS NULL",
            )
            .map_err(error)?;
        let targets = found
            .query_map([end.as_os_str().as_bytes()], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
            .map_err(error)?;
        let [(target, target_size)] = targets.as_slice() else {
            continue;
        };
        if matches!((size, target_size), (Some(a), Some(b)) if a != *b) || *target == id {
            continue;
        }
        moves.push(CollectionReanchor {
            stored_archive_id: id,
            stored_created_at: created_at,
            current_archive_id: *target,
        });
    }
    Ok(moves)
}

/// Moves each item onto its target row inside `tx`; see
/// [`Database::reanchor_collection_items`].
fn reanchor_in(tx: &rusqlite::Transaction<'_>, moves: &[CollectionReanchor]) -> Result<()> {
    let error = |error| db_error("failed to update saved games", error);
    {
        for item in moves {
            let Some(anchor) = archive_anchor(tx, item.current_archive_id)? else {
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
    }
    Ok(())
}
