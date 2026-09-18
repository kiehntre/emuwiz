-- Explicitly accepted ScreenScraper descriptive metadata.  This is not an
-- identity table: it has no platform/hash/verification columns and is never
-- consulted by identity resolution.
CREATE TABLE screenscraper_enrichments (
    archive_id       INTEGER PRIMARY KEY REFERENCES archives(id) ON DELETE CASCADE,
    values_json      BLOB NOT NULL,
    receipt_json     BLOB NOT NULL,
    updated_at       TEXT NOT NULL
);
