ALTER TABLE scan_fingerprints ADD COLUMN ingestion_version TEXT;
ALTER TABLE scan_fingerprints ADD COLUMN ingestion_listing_hash TEXT;
ALTER TABLE scan_fingerprints ADD COLUMN ingestion_member_count INTEGER;
ALTER TABLE scan_fingerprints ADD COLUMN ingestion_member_name TEXT;
ALTER TABLE scan_fingerprints ADD COLUMN ingestion_listing_available INTEGER;
