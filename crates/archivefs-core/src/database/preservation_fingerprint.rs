//! Import-time SHA-256 fingerprint for preservation-only recordings.
//!
//! A `preservation_recording` row is kept for its bytes, not as a game, so it
//! never reaches DAT verification - the only path that records hashes for
//! other rows. The scan therefore records one fingerprint in the reserved
//! `archives.content_hash` column (full-file SHA-256 hex, see
//! `docs/DATABASE_DESIGN.md`), which an upsert already clears when the file's
//! size or modification time changes.
//!
//! Two operations stay separate:
//!
//! * [`Database::record_preservation_fingerprint`] writes the import-time
//!   fingerprint once, only when the file still matches the row the scan just
//!   wrote, and only through the shared bounded streaming hasher with its
//!   changed-while-reading guard. Any doubt leaves the column `NULL` and is
//!   reported; a partial or stale value is never stored.
//! * [`Database::verify_preservation_fingerprint`] re-hashes the current file
//!   and compares. It never writes, so a later change is reported as a change,
//!   not silently re-recorded.
use super::*;
use crate::identity_source::hashing::FileFingerprint;
use crate::repair::exact_duplicate::hash_full_file_sha256;
use crate::safe_read::TrustedRoots;

/// `(id, content_hash, size_bytes, modified_time_unix_seconds)` of one row.
type RecordingRow = (i64, Option<String>, Option<i64>, Option<i64>);

/// The result of re-checking a recorded fingerprint against the file now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreservationFreshness {
    /// No import-time fingerprint is recorded (hashing was refused, or the
    /// row is not a preservation recording).
    NotRecorded,
    /// The current bytes hash to the recorded fingerprint.
    Fresh { sha256: String },
    /// The current bytes hash to something else.
    Changed { recorded: String, current: String },
    /// The file is missing, unsafe or changed while being read, so no current
    /// fingerprint can be claimed.
    Unavailable { recorded: String, reason: String },
}

impl Database {
    /// Records the import-time fingerprint for one scanned recording when the
    /// row has none. `Ok(Some(reason))` means none was recorded and why.
    pub(super) fn record_preservation_fingerprint(
        &mut self,
        folder: &RegisteredSourceFolder,
        archive: &Archive,
    ) -> Result<Option<String>> {
        let Ok(relative) = archive.path.strip_prefix(&folder.path) else {
            return Ok(Some("recording is outside its source folder".into()));
        };
        let row: Option<RecordingRow> = self
            .connection
            .query_row(
                "SELECT id, content_hash, size_bytes, modified_time_unix_seconds FROM archives \
                 WHERE source_folder_id = ?1 AND relative_path = ?2 \
                 AND archive_kind = 'preservation_recording'",
                params![folder.id, relative.as_os_str().as_bytes()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| db_error("failed to load preservation recording", error))?;
        let Some((id, recorded, size, modified)) = row else {
            return Ok(Some("no library row for the recording".into()));
        };
        if recorded.is_some() {
            return Ok(None);
        }
        // The bytes hashed must be the very file the scan inspected and the
        // row describes: same size and mtime as the row, and the same
        // filesystem object (device, inode, nanosecond mtime) as the scan's
        // observation. Afterwards the whole stamp, ctime included, must not
        // have moved.
        let before = FileFingerprint::observe(&archive.path);
        let scanned_mtime = archive
            .identity
            .modified_time
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
        let matches_scan = before.as_ref().is_some_and(|seen| {
            Some(seen.size_bytes as i64) == size
                && seen.modified_unix_seconds == modified
                && Some(seen.size_bytes) == archive.identity.size_bytes
                && seen.object.is_some_and(|object| {
                    Some(object.device) == archive.identity.filesystem_device
                        && Some(object.inode) == archive.identity.filesystem_inode
                        && scanned_mtime.is_some_and(|scanned| {
                            i64::from(scanned.subsec_nanos()) == object.modified_nanoseconds
                                && Some(scanned.as_secs() as i64) == seen.modified_unix_seconds
                        })
                })
        });
        if !matches_scan {
            return Ok(Some(
                "the file changed after the scan; no fingerprint recorded".into(),
            ));
        }
        let identity = match hash_full_file_sha256(&archive.path, &TrustedRoots::none(), None) {
            Ok(identity) => identity,
            Err(refusal) => return Ok(Some(format!("no fingerprint recorded: {refusal}"))),
        };
        if FileFingerprint::observe(&archive.path) != before
            || Some(identity.size_bytes as i64) != size
        {
            return Ok(Some(
                "the file changed while it was fingerprinted; no fingerprint recorded".into(),
            ));
        }
        self.connection
            .execute(
                "UPDATE archives SET content_hash = ?2 WHERE id = ?1 AND content_hash IS NULL \
                 AND size_bytes IS ?3 AND modified_time_unix_seconds IS ?4",
                params![id, identity.sha256, size, modified],
            )
            .map_err(|error| db_error("failed to record preservation fingerprint", error))?;
        Ok(None)
    }

    pub(super) fn is_preservation_recording(&self, archive_id: i64) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT archive_kind = 'preservation_recording' FROM archives WHERE id = ?1",
                params![archive_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map(|kind| kind.unwrap_or(false))
            .map_err(|error| db_error("failed to load archive kind", error))
    }

    /// The import-time fingerprint of a preservation recording, if recorded.
    pub fn preservation_fingerprint(&self, archive_id: i64) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT content_hash FROM archives WHERE id = ?1 \
                 AND archive_kind = 'preservation_recording'",
                params![archive_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(|error| db_error("failed to load preservation fingerprint", error))
    }

    /// Re-hashes the recording's current file and compares it with the
    /// import-time fingerprint. Read-only.
    pub fn verify_preservation_fingerprint(
        &self,
        archive_id: i64,
    ) -> Result<PreservationFreshness> {
        let Some(recorded) = self.preservation_fingerprint(archive_id)? else {
            return Ok(PreservationFreshness::NotRecorded);
        };
        let path: Vec<u8> = self
            .connection
            .query_row(
                "SELECT absolute_path_cached FROM archives WHERE id = ?1",
                params![archive_id],
                |row| row.get(0),
            )
            .map_err(|error| db_error("failed to load preservation recording path", error))?;
        let path = PathBuf::from(OsString::from_vec(path));
        Ok(
            match hash_full_file_sha256(&path, &TrustedRoots::none(), None) {
                Ok(identity) if identity.sha256 == recorded => {
                    PreservationFreshness::Fresh { sha256: recorded }
                }
                Ok(identity) => PreservationFreshness::Changed {
                    recorded,
                    current: identity.sha256,
                },
                Err(refusal) => PreservationFreshness::Unavailable {
                    recorded,
                    reason: refusal.to_string(),
                },
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file that is no longer the object the scan observed (for example a
    /// same-size, same-second rename-over) gets no fingerprint and a warning.
    #[test]
    fn a_replaced_object_is_refused_and_leaves_the_fingerprint_unset() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("zx");
        std::fs::create_dir(&source).unwrap();
        let path = source.join("session.rzx");
        let mut bytes = b"RZX!\0\x0d\0\0\0\0".to_vec();
        bytes.extend_from_slice(&[0x80, 10, 0, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(&path, &bytes).unwrap();
        let config = Config {
            source_folders: vec![source.clone()],
            mount_root: dir.path().join("mount"),
            ratarmount_bin: String::new(),
            master_rom_root: None,
        };
        let mut database = Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
        scan_and_persist(&mut database, &config, "fingerprint").unwrap();
        let id = database.load_archives().unwrap()[0].id;
        assert!(database.preservation_fingerprint(id).unwrap().is_some());

        database
            .connection
            .execute(
                "UPDATE archives SET content_hash = NULL WHERE id = ?1",
                [id],
            )
            .unwrap();
        let folder = database
            .register_source_folders(std::slice::from_ref(&source))
            .unwrap()
            .remove(0);
        let mut archive = Archive::from_path_in_root(&path, &source).unwrap();
        archive.identity.filesystem_inode = archive.identity.filesystem_inode.map(|i| i ^ 1);
        let warning = database
            .record_preservation_fingerprint(&folder, &archive)
            .unwrap();
        assert!(warning.is_some_and(|w| w.contains("changed after the scan")));
        assert_eq!(database.preservation_fingerprint(id).unwrap(), None);
    }
}
