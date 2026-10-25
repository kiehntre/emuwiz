-- Missing evidence may only be produced by an explicitly complete source walk.
-- Old runs have no coverage proof and remain historical, never authoritative.
CREATE TABLE scan_source_coverage (
    scan_run_id INTEGER NOT NULL REFERENCES scan_runs(id),
    source_folder_id INTEGER NOT NULL REFERENCES source_folders(id),
    state TEXT NOT NULL,
    excluded_roots_json TEXT NOT NULL,
    diagnostic TEXT,
    root_identity_json TEXT NOT NULL,
    PRIMARY KEY (scan_run_id, source_folder_id)
);
