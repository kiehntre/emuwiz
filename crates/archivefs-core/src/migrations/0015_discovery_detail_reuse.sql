-- Compact semantic identity for the persisted ingestion detail projection.
-- The current run retains its own scan_runs row; it may point at an older
-- detail projection when the evidence is byte-for-byte equivalent.
ALTER TABLE scan_runs ADD COLUMN discovery_details_fingerprint TEXT;
ALTER TABLE scan_runs ADD COLUMN discovery_details_source_run_id INTEGER;
ALTER TABLE scan_runs ADD COLUMN discovery_details_source_folder_id INTEGER;
CREATE INDEX scan_runs_discovery_details_fingerprint
    ON scan_runs(discovery_details_source_folder_id, discovery_details_fingerprint);
