-- Schema 29: "checked against every eligible catalogue for the game's system,
-- listed in none". One row per game at most; the exact catalogue set it was
-- checked against is stored once per set. Derived records: a row only says
-- the game need not be read again while it still holds (same path, system,
-- file object and catalogue set), which is judged again every time.
CREATE TABLE dat_catalogue_sets (
    set_sha256      TEXT NOT NULL PRIMARY KEY,
    platform        TEXT NOT NULL,
    catalogues_json BLOB NOT NULL,
    created_at      TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE dat_catalogue_set_no_matches (
    archive_id      INTEGER NOT NULL PRIMARY KEY,
    archive_path    BLOB NOT NULL,
    platform        TEXT NOT NULL,
    set_sha256      TEXT NOT NULL,
    catalogue_count INTEGER NOT NULL,
    checked_at      TEXT NOT NULL,
    facts_json      BLOB NOT NULL
) WITHOUT ROWID;
CREATE INDEX dat_catalogue_set_no_matches_by_set ON dat_catalogue_set_no_matches(set_sha256);
CREATE TRIGGER discovery_touch_dat_catalogue_set_no_matches_insert AFTER INSERT ON dat_catalogue_set_no_matches
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_dat_catalogue_set_no_matches_update AFTER UPDATE ON dat_catalogue_set_no_matches
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id OR archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_dat_catalogue_set_no_matches_delete AFTER DELETE ON dat_catalogue_set_no_matches
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id; END;
