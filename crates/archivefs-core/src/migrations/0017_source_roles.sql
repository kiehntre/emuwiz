-- Migration 0017: persist the explicit processing role of each source folder.
--
-- SR1 is vocabulary and persistence only. The default preserves existing
-- source behavior; scanner routing remains unchanged until a later phase.
ALTER TABLE source_folders
    ADD COLUMN source_role TEXT NOT NULL DEFAULT 'games';
