-- Additive cache for the cheap identity envelope used by incremental scans.
-- Rows are mutable: this is evidence, not an append-only scan history.
CREATE TABLE scan_fingerprints (
    source_folder_id INTEGER NOT NULL REFERENCES source_folders(id),
    relative_path BLOB NOT NULL,
    size_bytes INTEGER NOT NULL,
    modified_time_ns INTEGER NOT NULL,
    archive_kind TEXT NOT NULL,
    display_name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    platform TEXT,
    platform_provenance TEXT,
    scanner_version TEXT NOT NULL,
    parser_version TEXT NOT NULL,
    cache_version INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (source_folder_id, relative_path)
);
CREATE INDEX scan_fingerprints_source ON scan_fingerprints(source_folder_id);
