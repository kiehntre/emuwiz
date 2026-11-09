//! The person's manual system choices, kept outside the library database so
//! they survive a rebuilt, restored or re-created library.
//!
//! A choice is keyed by the game's content (size + SHA-256; for files over
//! [`FULL_HASH_LIMIT`] the first and last MiB), never by its path, so a moved
//! or renamed game keeps it and a different file with the same name never
//! inherits it. The database's `manual` assignment stays the live authority;
//! this file only restores it where the database lost it. Verified evidence
//! never overwrites a manual choice: a disagreement is shown as a conflict.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "manual-platform-choices.json";
/// Larger files are keyed by size plus their first and last MiB.
pub const FULL_HASH_LIMIT: u64 = 512 * 1024 * 1024;
const EDGE: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub size: u64,
    pub key: String,
    pub platform: String,
    /// Where the game was when chosen. A hint for people, never a key.
    pub path_hint: String,
    pub chosen_at_unix: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choices {
    pub version: u32,
    pub choices: Vec<Choice>,
}

/// The store beside the library database.
pub fn store_path(database_path: &Path) -> PathBuf {
    database_path.with_file_name(FILE_NAME)
}

/// Content key of a regular file (symbolic links are refused).
pub fn content_key(path: &Path) -> std::io::Result<(u64, String)> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let size = meta.len();
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    if size <= FULL_HASH_LIMIT {
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        return Ok((size, format!("sha256:{}", hex(&hasher.finalize()))));
    }
    file.read_exact(&mut buffer[..EDGE as usize])?;
    hasher.update(&buffer[..EDGE as usize]);
    file.seek(SeekFrom::Start(size - EDGE))?;
    file.read_exact(&mut buffer[..EDGE as usize])?;
    hasher.update(&buffer[..EDGE as usize]);
    Ok((size, format!("edges-sha256:{}", hex(&hasher.finalize()))))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn load(store: &Path) -> Choices {
    std::fs::read_to_string(store)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Choices {
            version: 1,
            choices: Vec::new(),
        })
}

fn save(store: &Path, choices: &Choices) -> crate::Result<()> {
    let text = serde_json::to_string_pretty(choices)
        .map_err(|e| crate::ArchiveFsError::Database(e.to_string()))?;
    crate::atomic_write_text(store, &text)
}

/// Records (or replaces) the choice for `game`'s content.
pub fn remember(store: &Path, game: &Path, platform: &str) -> crate::Result<()> {
    remember_all(store, std::slice::from_ref(&game.to_path_buf()), platform).map(|_| ())
}

/// Records one choice for each game in one store write. Games that cannot be
/// read (folders, links, missing files) are counted and skipped.
pub fn remember_all(store: &Path, games: &[PathBuf], platform: &str) -> crate::Result<usize> {
    let mut choices = load(store);
    choices.version = 1;
    let mut skipped = 0;
    for game in games {
        let Ok((size, key)) = content_key(game) else {
            skipped += 1;
            continue;
        };
        choices
            .choices
            .retain(|c| !(c.size == size && c.key == key));
        choices.choices.push(Choice {
            size,
            key,
            platform: platform.to_string(),
            path_hint: game.to_string_lossy().into_owned(),
            chosen_at_unix: crate::dat::sources::now_unix(),
        });
    }
    save(store, &choices)?;
    Ok(skipped)
}

/// Forgets the choice for `game`'s content, if any.
pub fn forget(store: &Path, game: &Path) -> crate::Result<()> {
    forget_all(store, std::slice::from_ref(&game.to_path_buf()))
}

pub fn forget_all(store: &Path, games: &[PathBuf]) -> crate::Result<()> {
    let mut choices = load(store);
    let before = choices.choices.len();
    for game in games {
        if let Ok((size, key)) = content_key(game) {
            choices
                .choices
                .retain(|c| !(c.size == size && c.key == key));
        }
    }
    if choices.choices.len() == before {
        return Ok(());
    }
    save(store, &choices)
}

/// Re-applies remembered choices to games the database no longer has a
/// manual choice for. Only files whose size matches a choice are read, so
/// this is cheap after a scan. Returns how many games were restored.
pub fn restore(database: &mut crate::Database, store: &Path) -> crate::Result<usize> {
    let choices = load(store).choices;
    if choices.is_empty() {
        return Ok(0);
    }
    let mut restored = 0;
    for choice in &choices {
        for (archive_id, path) in database.archives_without_manual_platform_of_size(choice.size)? {
            if content_key(&path).is_ok_and(|(size, key)| size == choice.size && key == choice.key)
            {
                database.set_manual_platform(archive_id, &choice.platform)?;
                restored += 1;
            }
        }
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_survives_a_rebuilt_database_and_follows_content_not_names() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        std::fs::create_dir_all(root.join("mixed")).unwrap();
        let game = root.join("mixed/Demo.zip");
        let lookalike = root.join("mixed/Other.zip");
        std::fs::write(&game, b"ps3-ish bytes").unwrap();
        std::fs::write(&lookalike, b"other bytes!!").unwrap(); // same size, other content
        let store = store_path(&temp.path().join("lib.sqlite3"));
        remember(&store, &game, "PS3").unwrap();

        // A brand-new database (rebuild) has no manual choices.
        let mut db = crate::Database::open_or_create(temp.path().join("lib.sqlite3")).unwrap();
        db.register_source_folders(std::slice::from_ref(&root))
            .unwrap();
        let folder = db.list_source_folders().unwrap()[0].id;
        let mut ids = Vec::new();
        for p in [&game, &lookalike] {
            let a = crate::Archive::from_path_in_root(p, &root).unwrap();
            ids.push(db.upsert_archive(folder, &root, &a).unwrap().archive_id);
        }
        assert_eq!(restore(&mut db, &store).unwrap(), 1);
        let current = |id: i64| -> Option<(String, String)> {
            rusqlite::Connection::open(db.path())
                .unwrap()
                .query_row(
                    "SELECT platform, source FROM platform_assignments WHERE archive_id=?1 AND is_current=1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok()
        };
        assert_eq!(current(ids[0]), Some(("PS3".into(), "manual".into())));
        assert_ne!(current(ids[1]).map(|a| a.1).as_deref(), Some("manual"));
        // Idempotent.
        assert_eq!(restore(&mut db, &store).unwrap(), 0);

        forget(&store, &game).unwrap();
        assert!(load(&store).choices.is_empty());
    }
}
