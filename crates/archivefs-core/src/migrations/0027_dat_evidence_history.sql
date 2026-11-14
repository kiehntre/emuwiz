-- Saved DAT evidence that a later check replaced. Kept for the record only:
-- nothing reads these rows as proof, and nothing promotes them back.
-- No foreign key: the record outlives the library row it describes, so the
-- row's path at replacement time is kept beside its id.
CREATE TABLE library_dat_identity_history (
    id                    INTEGER PRIMARY KEY,
    archive_id            INTEGER NOT NULL,
    archive_path          BLOB,
    dat_source_id         TEXT NOT NULL,
    verification_state    TEXT NOT NULL,
    completeness          TEXT NOT NULL,
    audited_at            TEXT NOT NULL,
    revision_marked_stale INTEGER NOT NULL,
    facts_json            BLOB NOT NULL,
    row_created_at        TEXT NOT NULL,
    row_updated_at        TEXT NOT NULL,
    replaced_at           TEXT NOT NULL,
    replaced_by_state     TEXT NOT NULL
);
CREATE INDEX library_dat_identity_history_item
    ON library_dat_identity_history(archive_id, dat_source_id);

-- Why the latest re-check of a saved row saved nothing. An explanation for
-- the person, never evidence: a row here says only that a check was tried.
-- Removed when a later check saves a result for the same row.
CREATE TABLE dat_recheck_attempts (
    archive_id       INTEGER NOT NULL,
    dat_source_id    TEXT NOT NULL,
    archive_path     BLOB NOT NULL,
    attempted_at     TEXT NOT NULL,
    reason           TEXT NOT NULL,
    catalogue_sha256 TEXT,
    PRIMARY KEY (archive_id, dat_source_id)
) WITHOUT ROWID;

-- Discovery import Undo: both tables hold per-game records, so a change to
-- either marks that game's import as modified, like every other per-game
-- evidence table (see 0025). Undo also refuses while such a record exists.
CREATE TRIGGER discovery_touch_library_dat_identity_history_insert AFTER INSERT ON library_dat_identity_history
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_library_dat_identity_history_update AFTER UPDATE ON library_dat_identity_history
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id OR archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_library_dat_identity_history_delete AFTER DELETE ON library_dat_identity_history
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id; END;
CREATE TRIGGER discovery_touch_dat_recheck_attempts_insert AFTER INSERT ON dat_recheck_attempts
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_dat_recheck_attempts_update AFTER UPDATE ON dat_recheck_attempts
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id OR archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_dat_recheck_attempts_delete AFTER DELETE ON dat_recheck_attempts
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id; END;
