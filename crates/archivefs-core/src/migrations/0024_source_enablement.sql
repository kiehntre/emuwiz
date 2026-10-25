-- Whether a configured source is enabled, as the database's own Missing authority
-- needs it at the write boundary (the config file cannot be read inside a
-- transaction). No row means enabled with nothing barred, so existing catalogues
-- and path-only registrations behave exactly as before.
-- barred_through_run: scan runs up to this id started before the source was last
-- disabled, so they can never write Missing, even after re-enable. A fresh scan can.
CREATE TABLE IF NOT EXISTS source_enablement (
    source_folder_id INTEGER PRIMARY KEY REFERENCES source_folders(id),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    barred_through_run INTEGER NOT NULL DEFAULT 0 CHECK (barred_through_run >= 0)
);
