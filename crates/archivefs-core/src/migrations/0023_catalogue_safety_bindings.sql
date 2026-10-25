-- Opening only adds safety bookkeeping; it does not reconcile old evidence
-- or infer an accepted device for historical sources.
CREATE TABLE catalogue_health_epoch (id INTEGER PRIMARY KEY CHECK (id = 1), revision INTEGER NOT NULL);
INSERT INTO catalogue_health_epoch VALUES (1, 0);
CREATE TABLE source_scan_bindings (
    source_folder_id INTEGER PRIMARY KEY REFERENCES source_folders(id),
    root_identity_json TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0)
);
-- Nested filesystem boundaries (mounts) first observed beneath a source. A later
-- scan that cannot prove the same filesystem is still there withholds Missing
-- evidence beneath it, so an unmounted volume is never mistaken for deleted files.
CREATE TABLE source_nested_boundaries (
    source_folder_id INTEGER NOT NULL REFERENCES source_folders(id),
    relative_path BLOB NOT NULL,
    -- NULL: a mount was seen here but its identity could not be captured
    -- continuously. It stays unproven until a later scan captures it.
    binding_json TEXT,
    PRIMARY KEY (source_folder_id, relative_path)
);
CREATE TRIGGER catalogue_epoch_archives_insert AFTER INSERT ON archives BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_archives_update AFTER UPDATE ON archives BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_archives_delete AFTER DELETE ON archives BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_source_folders_insert AFTER INSERT ON source_folders BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_source_folders_update AFTER UPDATE ON source_folders BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_source_folders_delete AFTER DELETE ON source_folders BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_scan_runs_insert AFTER INSERT ON scan_runs BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_scan_runs_update AFTER UPDATE ON scan_runs BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_scan_runs_delete AFTER DELETE ON scan_runs BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_scan_source_coverage_insert AFTER INSERT ON scan_source_coverage BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_scan_source_coverage_update AFTER UPDATE ON scan_source_coverage BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_scan_source_coverage_delete AFTER DELETE ON scan_source_coverage BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_source_scan_bindings_insert AFTER INSERT ON source_scan_bindings BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_source_scan_bindings_update AFTER UPDATE ON source_scan_bindings BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_source_scan_bindings_delete AFTER DELETE ON source_scan_bindings BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_archive_scan_observations_insert AFTER INSERT ON archive_scan_observations BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_archive_scan_observations_update AFTER UPDATE ON archive_scan_observations BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_archive_scan_observations_delete AFTER DELETE ON archive_scan_observations BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_verified_identity_facts_insert AFTER INSERT ON verified_identity_facts BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_verified_identity_facts_update AFTER UPDATE ON verified_identity_facts BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_verified_identity_facts_delete AFTER DELETE ON verified_identity_facts BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
ALTER TABLE scan_source_coverage ADD COLUMN source_generation INTEGER;
CREATE TRIGGER catalogue_epoch_platform_assignments_insert AFTER INSERT ON platform_assignments BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_platform_assignments_update AFTER UPDATE ON platform_assignments BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
CREATE TRIGGER catalogue_epoch_platform_assignments_delete AFTER DELETE ON platform_assignments BEGIN UPDATE catalogue_health_epoch SET revision = revision + 1 WHERE id = 1; END;
-- Sources that already had catalogue history when this schema was applied. Such a
-- source stays unbound until a person reviews it: the storage at its old path is
-- never trusted merely because a first scan happens afterwards, and an absent
-- `last_successful_scan_at` (older databases never recorded one) is not evidence
-- of a new source. This only records *which* sources need review; it binds
-- nothing, changes no historical row and writes no Missing evidence. A brand-new
-- source never appears here, and a reviewed rebind removes its row.
CREATE TABLE source_review_required (
    source_folder_id INTEGER PRIMARY KEY REFERENCES source_folders(id)
);
INSERT INTO source_review_required(source_folder_id)
    SELECT id FROM source_folders
    WHERE last_successful_scan_at IS NOT NULL
       OR EXISTS (SELECT 1 FROM archives WHERE archives.source_folder_id = source_folders.id);
