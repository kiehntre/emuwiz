-- SQLite is the recovery journal for this DB-only operation. Receipt and
-- created rows commit together; ownership survives Undo and cannot be reset.
CREATE TABLE discovery_import_operations (
    operation_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('committed','undone')),
    created_at TEXT NOT NULL,
    undone_at TEXT,
    receipt_json BLOB NOT NULL
);
CREATE TABLE discovery_import_records (
    operation_id TEXT NOT NULL REFERENCES discovery_import_operations(operation_id),
    archive_id INTEGER NOT NULL,
    source_folder_id INTEGER NOT NULL,
    relative_path BLOB NOT NULL,
    absolute_path BLOB NOT NULL,
    snapshot_json BLOB NOT NULL,
    touched INTEGER NOT NULL DEFAULT 0 CHECK (touched IN (0,1)),
    PRIMARY KEY (operation_id, archive_id)
);
CREATE INDEX discovery_import_records_archive ON discovery_import_records(archive_id);
CREATE INDEX discovery_import_records_path ON discovery_import_records(absolute_path);
CREATE TRIGGER discovery_import_evidence_keep BEFORE DELETE ON discovery_import_records
BEGIN SELECT RAISE(ABORT, 'Discovery import ownership evidence is retained'); END;
CREATE TRIGGER discovery_import_evidence_immutable BEFORE UPDATE ON discovery_import_records
WHEN NEW.operation_id IS NOT OLD.operation_id OR NEW.archive_id IS NOT OLD.archive_id
 OR NEW.source_folder_id IS NOT OLD.source_folder_id OR NEW.relative_path IS NOT OLD.relative_path
 OR NEW.absolute_path IS NOT OLD.absolute_path OR NEW.snapshot_json IS NOT OLD.snapshot_json
 OR NEW.touched < OLD.touched
BEGIN SELECT RAISE(ABORT, 'Discovery import ownership evidence is immutable'); END;
CREATE TRIGGER discovery_import_receipt_keep BEFORE DELETE ON discovery_import_operations
BEGIN SELECT RAISE(ABORT, 'Discovery import receipt is retained'); END;
CREATE TRIGGER discovery_import_receipt_immutable BEFORE UPDATE ON discovery_import_operations
WHEN NEW.operation_id IS NOT OLD.operation_id OR NEW.created_at IS NOT OLD.created_at
 OR NEW.receipt_json IS NOT OLD.receipt_json OR OLD.state = 'undone'
 OR NEW.state != 'undone' OR NEW.undone_at IS NULL
BEGIN SELECT RAISE(ABORT, 'Discovery import receipt is immutable'); END;
CREATE TRIGGER discovery_touch_archives_insert AFTER INSERT ON archives
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.id; END;
CREATE TRIGGER discovery_touch_archives_update AFTER UPDATE ON archives
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.id OR archive_id = NEW.id; END;
CREATE TRIGGER discovery_touch_archives_delete AFTER DELETE ON archives
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.id; END;
CREATE TRIGGER discovery_touch_platform_assignments_insert AFTER INSERT ON platform_assignments
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_platform_assignments_update AFTER UPDATE ON platform_assignments
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id OR archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_platform_assignments_delete AFTER DELETE ON platform_assignments
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id; END;
CREATE TRIGGER discovery_touch_archive_scan_observations_insert AFTER INSERT ON archive_scan_observations
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_archive_scan_observations_update AFTER UPDATE ON archive_scan_observations
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id OR archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_archive_scan_observations_delete AFTER DELETE ON archive_scan_observations
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id; END;
CREATE TRIGGER discovery_touch_library_dat_identities_insert AFTER INSERT ON library_dat_identities
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_library_dat_identities_update AFTER UPDATE ON library_dat_identities
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id OR archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_library_dat_identities_delete AFTER DELETE ON library_dat_identities
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id; END;
CREATE TRIGGER discovery_touch_verified_identity_facts_insert AFTER INSERT ON verified_identity_facts
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_verified_identity_facts_update AFTER UPDATE ON verified_identity_facts
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id OR archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_verified_identity_facts_delete AFTER DELETE ON verified_identity_facts
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id; END;
CREATE TRIGGER discovery_touch_screenscraper_enrichments_insert AFTER INSERT ON screenscraper_enrichments
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_screenscraper_enrichments_update AFTER UPDATE ON screenscraper_enrichments
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id OR archive_id = NEW.archive_id; END;
CREATE TRIGGER discovery_touch_screenscraper_enrichments_delete AFTER DELETE ON screenscraper_enrichments
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id = OLD.archive_id; END;
CREATE TRIGGER discovery_touch_dat_set_audit_results_insert AFTER INSERT ON dat_set_audit_results
BEGIN UPDATE discovery_import_records SET touched=1 WHERE (archive_id = NEW.archive_id OR absolute_path = NEW.archive_path); END;
CREATE TRIGGER discovery_touch_dat_set_audit_results_update AFTER UPDATE ON dat_set_audit_results
BEGIN UPDATE discovery_import_records SET touched=1 WHERE (archive_id = OLD.archive_id OR absolute_path = OLD.archive_path) OR (archive_id = NEW.archive_id OR absolute_path = NEW.archive_path); END;
CREATE TRIGGER discovery_touch_dat_set_audit_results_delete AFTER DELETE ON dat_set_audit_results
BEGIN UPDATE discovery_import_records SET touched=1 WHERE (archive_id = OLD.archive_id OR absolute_path = OLD.archive_path); END;
CREATE TRIGGER discovery_touch_scan_fingerprints_insert AFTER INSERT ON scan_fingerprints
BEGIN UPDATE discovery_import_records SET touched=1 WHERE (source_folder_id = NEW.source_folder_id AND relative_path = NEW.relative_path); END;
CREATE TRIGGER discovery_touch_scan_fingerprints_update AFTER UPDATE ON scan_fingerprints
BEGIN UPDATE discovery_import_records SET touched=1 WHERE (source_folder_id = OLD.source_folder_id AND relative_path = OLD.relative_path) OR (source_folder_id = NEW.source_folder_id AND relative_path = NEW.relative_path); END;
CREATE TRIGGER discovery_touch_scan_fingerprints_delete AFTER DELETE ON scan_fingerprints
BEGIN UPDATE discovery_import_records SET touched=1 WHERE (source_folder_id = OLD.source_folder_id AND relative_path = OLD.relative_path); END;
CREATE TRIGGER discovery_touch_discovery_details_insert AFTER INSERT ON discovery_details
BEGIN UPDATE discovery_import_records SET touched=1 WHERE absolute_path = NEW.path; END;
CREATE TRIGGER discovery_touch_discovery_details_update AFTER UPDATE ON discovery_details
BEGIN UPDATE discovery_import_records SET touched=1 WHERE absolute_path = OLD.path OR absolute_path = NEW.path; END;
CREATE TRIGGER discovery_touch_discovery_details_delete AFTER DELETE ON discovery_details
BEGIN UPDATE discovery_import_records SET touched=1 WHERE absolute_path = OLD.path; END;
CREATE TRIGGER discovery_touch_mame_member_evidence_insert AFTER INSERT ON mame_member_evidence
BEGIN UPDATE discovery_import_records SET touched=1 WHERE absolute_path = NEW.source_path; END;
CREATE TRIGGER discovery_touch_mame_member_evidence_update AFTER UPDATE ON mame_member_evidence
BEGIN UPDATE discovery_import_records SET touched=1 WHERE absolute_path = OLD.source_path OR absolute_path = NEW.source_path; END;
CREATE TRIGGER discovery_touch_mame_member_evidence_delete AFTER DELETE ON mame_member_evidence
BEGIN UPDATE discovery_import_records SET touched=1 WHERE absolute_path = OLD.source_path; END;
CREATE TRIGGER discovery_touch_dat_dependencies_insert AFTER INSERT ON dat_set_audit_dependencies
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id IN (SELECT archive_id FROM dat_set_audit_results WHERE id = NEW.result_id)
 OR absolute_path IN (SELECT archive_path FROM dat_set_audit_results WHERE id = NEW.result_id); END;
CREATE TRIGGER discovery_touch_topology_insert AFTER INSERT ON media_topology_evidence
BEGIN UPDATE discovery_import_records SET touched=1 WHERE touched=0; END;
CREATE TRIGGER discovery_touch_dat_dependencies_update AFTER UPDATE ON dat_set_audit_dependencies
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id IN (SELECT archive_id FROM dat_set_audit_results WHERE id = OLD.result_id OR id = NEW.result_id)
 OR absolute_path IN (SELECT archive_path FROM dat_set_audit_results WHERE id = OLD.result_id OR id = NEW.result_id); END;
CREATE TRIGGER discovery_touch_topology_update AFTER UPDATE ON media_topology_evidence
BEGIN UPDATE discovery_import_records SET touched=1 WHERE touched=0; END;
CREATE TRIGGER discovery_touch_dat_dependencies_delete AFTER DELETE ON dat_set_audit_dependencies
BEGIN UPDATE discovery_import_records SET touched=1 WHERE archive_id IN (SELECT archive_id FROM dat_set_audit_results WHERE id = OLD.result_id)
 OR absolute_path IN (SELECT archive_path FROM dat_set_audit_results WHERE id = OLD.result_id); END;
CREATE TRIGGER discovery_touch_topology_delete AFTER DELETE ON media_topology_evidence
BEGIN UPDATE discovery_import_records SET touched=1 WHERE touched=0; END;
-- REPLACE may bypass DELETE triggers when recursive_triggers is disabled.
-- Reject duplicate identities before its implicit deletion can occur.
CREATE TRIGGER discovery_import_receipt_no_replace BEFORE INSERT ON discovery_import_operations
WHEN EXISTS(SELECT 1 FROM discovery_import_operations WHERE operation_id=NEW.operation_id)
BEGIN SELECT RAISE(ABORT, 'Discovery import operation identity already exists'); END;
CREATE TRIGGER discovery_import_evidence_no_replace BEFORE INSERT ON discovery_import_records
WHEN EXISTS(SELECT 1 FROM discovery_import_records WHERE operation_id=NEW.operation_id AND archive_id=NEW.archive_id)
BEGIN SELECT RAISE(ABORT, 'Discovery import ownership identity already exists'); END;
-- Retained ownership is also an ID tombstone. A delayed worker must never
-- attach evidence for an undone row to a later record reusing its integer ID.
CREATE TRIGGER discovery_import_archive_no_reuse AFTER INSERT ON archives
WHEN EXISTS(SELECT 1 FROM discovery_import_records WHERE archive_id=NEW.id)
BEGIN SELECT RAISE(ABORT, 'Archive identity was reserved by an earlier Discovery import'); END;
CREATE TRIGGER discovery_import_archive_no_rekey BEFORE UPDATE OF id ON archives
WHEN NEW.id != OLD.id AND EXISTS(SELECT 1 FROM discovery_import_records WHERE archive_id=NEW.id)
BEGIN SELECT RAISE(ABORT, 'Archive identity was reserved by an earlier Discovery import'); END;
CREATE INDEX discovery_import_records_untouched ON discovery_import_records(touched) WHERE touched=0;
