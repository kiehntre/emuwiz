-- Safe deterministic scanner outcomes for non-archive files. The compact
-- class is enough to recreate the existing skip branch without reopening or
-- probing the file; it is still guarded by the same metadata/version envelope.
ALTER TABLE scan_fingerprints ADD COLUMN non_archive_kind TEXT;
