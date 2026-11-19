-- Schema 30: exact skipped-file totals per scan run, source and reason.
-- Counted for every skipped file during the walk, so they never depend on the
-- bounded skipped-file sample or the bounded fingerprint cache. A scan writes
-- one row per reason (zeros included) for every source it walked; a run or
-- source without rows did not record a breakdown (older runs, sources that
-- could not be walked), which readers report as "not recorded", never as 0.
-- Additive: no existing table or row changes.
CREATE TABLE scan_run_skip_counts (
    scan_run_id      INTEGER NOT NULL REFERENCES scan_runs(id),
    source_folder_id INTEGER NOT NULL REFERENCES source_folders(id),
    reason           TEXT NOT NULL,
    file_count       INTEGER NOT NULL CHECK (file_count >= 0),
    PRIMARY KEY (scan_run_id, source_folder_id, reason)
) WITHOUT ROWID;
