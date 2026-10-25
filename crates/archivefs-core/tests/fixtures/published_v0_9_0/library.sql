-- Library database created by the published EmuWiz v0.9.0 release asset
-- (archivefs-v0.9.0-x86_64-linux.tar.gz, sha256
-- 744a348ef981e8cb7f5ab61b7f546315578ac578d507967bedf1f40d3d0530b8) in a clean
-- Ubuntu 24.04 container: two source folders, then 'library-scan' of three
-- .gb files and two .zip archives. Schema objects in v0.9.0's creation order.
PRAGMA user_version=16;
BEGIN;
CREATE TABLE schema_migrations (
    version     INTEGER PRIMARY KEY,
    description TEXT NOT NULL,
    applied_at  TEXT NOT NULL
);
CREATE TABLE source_folders (
    id                     INTEGER PRIMARY KEY,
    path                   BLOB NOT NULL,
    first_seen_at          TEXT NOT NULL,
    last_seen_in_config_at TEXT NOT NULL,
    removed_from_config_at TEXT
, last_scan_status TEXT, last_scan_error TEXT, last_scan_at TEXT, last_successful_scan_at TEXT, last_archive_count INTEGER, assigned_platform TEXT);
CREATE TABLE archives (
    id                          INTEGER PRIMARY KEY,
    source_folder_id            INTEGER NOT NULL REFERENCES source_folders(id),
    relative_path               BLOB NOT NULL,
    absolute_path_cached        BLOB NOT NULL,
    file_name_cached             BLOB NOT NULL,
    archive_kind                TEXT NOT NULL,
    display_name                TEXT NOT NULL,
    normalized_name              TEXT NOT NULL,
    size_bytes                  INTEGER,
    modified_time_unix_seconds  INTEGER,
    content_hash                 TEXT,
    archive_hash                 TEXT,
    internal_listing_hash         TEXT,
    last_known_health           TEXT NOT NULL DEFAULT 'Pending',
    first_seen_at                TEXT NOT NULL,
    last_seen_at                  TEXT NOT NULL,
    last_verified_missing_at      TEXT,
    created_at                   TEXT NOT NULL,
    updated_at                   TEXT NOT NULL
, identity_report_json BLOB, identity_report_size_bytes INTEGER, identity_report_modified_time_unix_seconds INTEGER);
CREATE TABLE scan_runs (
    id                     INTEGER PRIMARY KEY,
    started_at             TEXT NOT NULL,
    finished_at            TEXT,
    triggered_by           TEXT NOT NULL,
    config_content_digest  BLOB,
    source_folders_scanned INTEGER NOT NULL DEFAULT 0,
    archives_seen          INTEGER NOT NULL DEFAULT 0,
    archives_added         INTEGER NOT NULL DEFAULT 0,
    archives_updated       INTEGER NOT NULL DEFAULT 0,
    archives_missing       INTEGER NOT NULL DEFAULT 0,
    errors_count           INTEGER NOT NULL DEFAULT 0,
    status                 TEXT NOT NULL DEFAULT 'running',
    error_message          TEXT
, archives_unchanged INTEGER NOT NULL DEFAULT 0, skipped_unsupported_extension INTEGER NOT NULL DEFAULT 0, skipped_ambiguous_platform INTEGER NOT NULL DEFAULT 0, discovery_details_fingerprint TEXT, discovery_details_source_run_id INTEGER, discovery_details_source_folder_id INTEGER);
CREATE TABLE archive_scan_observations (
    id                          INTEGER PRIMARY KEY,
    scan_run_id                  INTEGER NOT NULL REFERENCES scan_runs(id),
    archive_id                   INTEGER NOT NULL REFERENCES archives(id),
    observation                 TEXT NOT NULL,
    size_bytes                  INTEGER,
    modified_time_unix_seconds  INTEGER,
    observed_at                  TEXT NOT NULL
);
CREATE TABLE platform_assignments (
    id          INTEGER PRIMARY KEY,
    archive_id  INTEGER NOT NULL REFERENCES archives(id),
    platform    TEXT NOT NULL,
    source      TEXT NOT NULL,
    confidence  TEXT,
    is_current  INTEGER NOT NULL DEFAULT 1,
    assigned_at TEXT NOT NULL
);
CREATE TABLE platform_aliases (
    id               INTEGER PRIMARY KEY,
    alias            TEXT NOT NULL,
    normalized_alias TEXT NOT NULL,
    platform         TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);
CREATE TABLE discovery_details (
    id                     INTEGER PRIMARY KEY,
    scan_run_id            INTEGER NOT NULL REFERENCES scan_runs(id),
    path                   BLOB NOT NULL,
    container              TEXT NOT NULL,
    content                TEXT,
    platform_hint          TEXT,
    validation_state       TEXT NOT NULL,
    skip_reason            TEXT,
    skip_reason_detail     TEXT,
    explanation            TEXT NOT NULL,
    identity_display_name  TEXT,
    identity_platform      TEXT
);
CREATE TABLE library_dat_identities (
    id                    INTEGER PRIMARY KEY,
    archive_id            INTEGER NOT NULL REFERENCES archives(id),
    dat_source_id         TEXT NOT NULL,
    dat_ecosystem         TEXT,
    source_revision       TEXT,
    verification_state    TEXT NOT NULL,
    completeness          TEXT NOT NULL,
    audited_at            TEXT NOT NULL,
    revision_marked_stale INTEGER NOT NULL DEFAULT 0,
    facts_json            BLOB NOT NULL,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);
CREATE TABLE dat_set_audit_results (
    id                    INTEGER PRIMARY KEY,
    archive_id            INTEGER REFERENCES archives(id),
    archive_path          BLOB NOT NULL,
    source_id             TEXT NOT NULL,
    game_name             TEXT NOT NULL,
    platform              TEXT,
    set_state_json        TEXT NOT NULL,
    dependency_state_json TEXT NOT NULL,
    ecosystem              TEXT,
    dat_revision           TEXT,
    audited_at             TEXT NOT NULL,
    stale                 INTEGER NOT NULL DEFAULT 0,
    exhaustive            INTEGER NOT NULL DEFAULT 1,
    UNIQUE(archive_path, source_id, game_name)
);
CREATE TABLE dat_set_audit_dependencies (
    id                  INTEGER PRIMARY KEY,
    result_id           INTEGER NOT NULL REFERENCES dat_set_audit_results(id) ON DELETE CASCADE,
    dependency_kind     TEXT NOT NULL,
    dependency_outcome TEXT NOT NULL,
    target_json         TEXT NOT NULL,
    via_member          TEXT,
    UNIQUE(result_id, dependency_kind, dependency_outcome, target_json, via_member)
);
CREATE TABLE verified_identity_facts (
    id                         INTEGER PRIMARY KEY,
    archive_id                 INTEGER NOT NULL REFERENCES archives(id),
    kind                       TEXT NOT NULL,
    value                      TEXT NOT NULL,
    confidence                 TEXT NOT NULL,
    method                     TEXT,
    member_path                BLOB,
    observed_at                TEXT NOT NULL,
    file_device                INTEGER NOT NULL,
    file_inode                 INTEGER NOT NULL,
    file_size_bytes            INTEGER NOT NULL,
    file_modified_unix_seconds INTEGER,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL
);
CREATE TABLE dat_expected_entries (
    id                  INTEGER PRIMARY KEY,
    dat_source_id       TEXT NOT NULL,
    canonical_identity  TEXT NOT NULL,
    display_name        TEXT NOT NULL,
    source_revision     TEXT,
    ecosystem           TEXT,
    metadata_json       BLOB,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE TABLE dat_expected_inventory_meta (
    dat_source_id           TEXT PRIMARY KEY,
    source_revision         TEXT,
    ecosystem               TEXT,
    entry_count             INTEGER NOT NULL,
    duplicate_names_skipped INTEGER NOT NULL DEFAULT 0,
    validated_at            TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);
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
    updated_at TEXT NOT NULL, ingestion_version TEXT, ingestion_listing_hash TEXT, ingestion_member_count INTEGER, ingestion_member_name TEXT, ingestion_listing_available INTEGER, non_archive_kind TEXT,
    PRIMARY KEY (source_folder_id, relative_path)
);
INSERT INTO "schema_migrations" VALUES(1,'initial schema: schema_migrations, source_folders, archives, scan_runs, archive_scan_observations, platform_assignments','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(2,'add platform_aliases: persistent custom folder-name -> canonical platform mappings','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(3,'add source_folders scan-status columns: last_scan_status, last_scan_error, last_scan_at, last_successful_scan_at, last_archive_count','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(4,'retain unchanged, unsupported-extension, and ambiguous-platform scan counts','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(5,'add optional persisted source-level platform assignment','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(6,'persist bounded direct-image identity reports with source metadata','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(7,'persist every ingestion-discovery item per scan run for paged Collection Discovery browsing','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(8,'persist per-library-item DAT identity results (library_dat_identities), source-scoped, so the selected-item path reconstructs LibraryDatIdentitySummary without re-auditing','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(9,'persist DAT set-completeness and dependency verdicts with source provenance','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(10,'persist verified per-game identity facts (verified_identity_facts) as a catalogue-side cache/projection - never a launch trust anchor','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(11,'persist named expected DAT inventory (dat_expected_entries) per source, so Expected/Missing/Full-set coverage can be proven without reopening a DAT file','2026-10-21T22:53:39Z');
INSERT INTO "schema_migrations" VALUES(12,'per-source expected-inventory metadata (dat_expected_inventory_meta): the validation generation, revision, entry count, and duplicate-name-skip count the dat_expected_entries rows describe','2026-10-21T22:53:40Z');
INSERT INTO "schema_migrations" VALUES(13,'persist cheap per-file scan fingerprints for safe unchanged-input reuse','2026-10-21T22:53:40Z');
INSERT INTO "schema_migrations" VALUES(14,'add compact ingestion archive listing evidence to scan fingerprints','2026-10-21T22:53:40Z');
INSERT INTO "schema_migrations" VALUES(15,'reuse unchanged ingestion discovery detail evidence across scan runs','2026-10-21T22:53:40Z');
INSERT INTO "schema_migrations" VALUES(16,'persist safe deterministic non-archive scanner outcomes','2026-10-21T22:53:40Z');
INSERT INTO "source_folders" VALUES(1,X'2f686f6d652f7465737465722f726f6d732f6762','2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,'success',NULL,'2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',3,NULL);
INSERT INTO "source_folders" VALUES(2,X'2f686f6d652f7465737465722f726f6d732f6172636869766573','2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,'success',NULL,'2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',2,NULL);
INSERT INTO "archives" VALUES(1,1,X'53796e7468657469632047616d6520302e6762',X'2f686f6d652f7465737465722f726f6d732f67622f53796e7468657469632047616d6520302e6762',X'53796e7468657469632047616d6520302e6762','direct_game_image','Synthetic Game 0','synthetic_game_0',32768,1792623219,NULL,NULL,NULL,'Pending','2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,'2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,NULL,NULL);
INSERT INTO "archives" VALUES(2,1,X'53796e7468657469632047616d6520312e6762',X'2f686f6d652f7465737465722f726f6d732f67622f53796e7468657469632047616d6520312e6762',X'53796e7468657469632047616d6520312e6762','direct_game_image','Synthetic Game 1','synthetic_game_1',32768,1792623219,NULL,NULL,NULL,'Pending','2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,'2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,NULL,NULL);
INSERT INTO "archives" VALUES(3,1,X'53796e7468657469632047616d6520322e6762',X'2f686f6d652f7465737465722f726f6d732f67622f53796e7468657469632047616d6520322e6762',X'53796e7468657469632047616d6520322e6762','direct_game_image','Synthetic Game 2','synthetic_game_2',32768,1792623219,NULL,NULL,NULL,'Pending','2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,'2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,NULL,NULL);
INSERT INTO "archives" VALUES(4,2,X'4172636869766520302e7a6970',X'2f686f6d652f7465737465722f726f6d732f61726368697665732f4172636869766520302e7a6970',X'4172636869766520302e7a6970','zip','Archive 0','archive_0',65660,1792623219,NULL,NULL,NULL,'Pending','2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,'2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,NULL,NULL);
INSERT INTO "archives" VALUES(5,2,X'4172636869766520312e7a6970',X'2f686f6d652f7465737465722f726f6d732f61726368697665732f4172636869766520312e7a6970',X'4172636869766520312e7a6970','zip','Archive 1','archive_1',65660,1792623219,NULL,NULL,NULL,'Pending','2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,'2026-10-21T22:53:40Z','2026-10-21T22:53:40Z',NULL,NULL,NULL);
INSERT INTO "scan_runs" VALUES(1,'2026-10-21T22:53:40Z','2026-10-21T22:53:40Z','cli-library-scan',NULL,2,5,5,0,0,0,'completed',NULL,0,0,0,'03ebdafe90089f1ef5d534673510e952c08d6abc6d635f776053330b7e45994c',NULL,2);
INSERT INTO "archive_scan_observations" VALUES(1,1,1,'added',32768,1792623219,'2026-10-21T22:53:40Z');
INSERT INTO "archive_scan_observations" VALUES(2,1,2,'added',32768,1792623219,'2026-10-21T22:53:40Z');
INSERT INTO "archive_scan_observations" VALUES(3,1,3,'added',32768,1792623219,'2026-10-21T22:53:40Z');
INSERT INTO "archive_scan_observations" VALUES(4,1,4,'added',65660,1792623219,'2026-10-21T22:53:40Z');
INSERT INTO "archive_scan_observations" VALUES(5,1,5,'added',65660,1792623219,'2026-10-21T22:53:40Z');
INSERT INTO "platform_assignments" VALUES(1,1,'Game Boy','folder_alias',NULL,1,'2026-10-21T22:53:40Z');
INSERT INTO "platform_assignments" VALUES(2,2,'Game Boy','folder_alias',NULL,1,'2026-10-21T22:53:40Z');
INSERT INTO "platform_assignments" VALUES(3,3,'Game Boy','folder_alias',NULL,1,'2026-10-21T22:53:40Z');
INSERT INTO "discovery_details" VALUES(1,1,X'2f686f6d652f7465737465722f726f6d732f67622f53796e7468657469632047616d6520302e6762','direct_file','rom_cartridge','Game Boy','accepted',NULL,NULL,'ROM cartridge.','Synthetic Game 0','Game Boy');
INSERT INTO "discovery_details" VALUES(2,1,X'2f686f6d652f7465737465722f726f6d732f67622f53796e7468657469632047616d6520312e6762','direct_file','rom_cartridge','Game Boy','accepted',NULL,NULL,'ROM cartridge.','Synthetic Game 1','Game Boy');
INSERT INTO "discovery_details" VALUES(3,1,X'2f686f6d652f7465737465722f726f6d732f67622f53796e7468657469632047616d6520322e6762','direct_file','rom_cartridge','Game Boy','accepted',NULL,NULL,'ROM cartridge.','Synthetic Game 2','Game Boy');
INSERT INTO "discovery_details" VALUES(4,1,X'2f686f6d652f7465737465722f726f6d732f61726368697665732f4172636869766520312e7a6970','archive','rom_cartridge',NULL,'skipped','no_identity_match',NULL,'ZIP archive containing ROM cartridge (Archive 1.gba).','Archive 1',NULL);
INSERT INTO "discovery_details" VALUES(5,1,X'2f686f6d652f7465737465722f726f6d732f61726368697665732f4172636869766520302e7a6970','archive','rom_cartridge',NULL,'skipped','no_identity_match',NULL,'ZIP archive containing ROM cartridge (Archive 0.gba).','Archive 0',NULL);
INSERT INTO "scan_fingerprints" VALUES(1,X'53796e7468657469632047616d6520302e6762',32768,1792623219664519877,'direct_game_image','Synthetic Game 0','synthetic_game_0','Game Boy','folder_alias','scan-v2-direct-media','archive-parser-v1',1,'2026-10-21T22:53:40Z',NULL,NULL,NULL,NULL,NULL,NULL);
INSERT INTO "scan_fingerprints" VALUES(1,X'53796e7468657469632047616d6520312e6762',32768,1792623219664519877,'direct_game_image','Synthetic Game 1','synthetic_game_1','Game Boy','folder_alias','scan-v2-direct-media','archive-parser-v1',1,'2026-10-21T22:53:40Z',NULL,NULL,NULL,NULL,NULL,NULL);
INSERT INTO "scan_fingerprints" VALUES(1,X'53796e7468657469632047616d6520322e6762',32768,1792623219664519877,'direct_game_image','Synthetic Game 2','synthetic_game_2','Game Boy','folder_alias','scan-v2-direct-media','archive-parser-v1',1,'2026-10-21T22:53:40Z',NULL,NULL,NULL,NULL,NULL,NULL);
INSERT INTO "scan_fingerprints" VALUES(2,X'4172636869766520302e7a6970',65660,1792623219665519913,'zip','Archive 0','archive_0',NULL,NULL,'scan-v2-direct-media','archive-parser-v1',1,'2026-10-21T22:53:40Z','ingestion-parser-v1','8afa2ebbf24e2fcb3976b5a2ef95fec15770ecf830a8d85cbda1b9fc4aeb6c20',1,'Archive 0.gba',1,NULL);
INSERT INTO "scan_fingerprints" VALUES(2,X'4172636869766520312e7a6970',65660,1792623219665519913,'zip','Archive 1','archive_1',NULL,NULL,'scan-v2-direct-media','archive-parser-v1',1,'2026-10-21T22:53:40Z','ingestion-parser-v1','dd620d1eabd0a941d4601d2beb7b742be151c6458b33e5c8dda9847ef79b9c44',1,'Archive 1.gba',1,NULL);
CREATE UNIQUE INDEX source_folders_path ON source_folders(path);
CREATE UNIQUE INDEX archives_identity ON archives(source_folder_id, relative_path);
CREATE INDEX archives_normalized_name ON archives(normalized_name);
CREATE INDEX archives_content_hash ON archives(content_hash) WHERE content_hash IS NOT NULL;
CREATE INDEX archives_missing ON archives(last_verified_missing_at) WHERE last_verified_missing_at IS NOT NULL;
CREATE INDEX scan_runs_status ON scan_runs(status) WHERE status = 'running';
CREATE INDEX archive_scan_observations_archive ON archive_scan_observations(archive_id, observed_at);
CREATE INDEX archive_scan_observations_run ON archive_scan_observations(scan_run_id);
CREATE UNIQUE INDEX platform_assignments_current ON platform_assignments(archive_id) WHERE is_current = 1;
CREATE INDEX platform_assignments_archive ON platform_assignments(archive_id);
CREATE INDEX platform_assignments_platform ON platform_assignments(platform);
CREATE UNIQUE INDEX platform_aliases_normalized_alias ON platform_aliases(normalized_alias);
CREATE INDEX discovery_details_run_order ON discovery_details(scan_run_id, id);
CREATE UNIQUE INDEX library_dat_identities_item_source
    ON library_dat_identities(archive_id, dat_source_id);
CREATE INDEX library_dat_identities_archive ON library_dat_identities(archive_id);
CREATE INDEX library_dat_identities_source ON library_dat_identities(dat_source_id);
CREATE INDEX dat_set_audit_results_archive ON dat_set_audit_results(archive_path);
CREATE INDEX dat_set_audit_results_source ON dat_set_audit_results(source_id);
CREATE INDEX dat_set_audit_results_stale ON dat_set_audit_results(stale) WHERE stale = 1;
CREATE INDEX dat_set_audit_results_state ON dat_set_audit_results(set_state_json);
CREATE INDEX dat_set_audit_dependencies_result ON dat_set_audit_dependencies(result_id);
CREATE INDEX dat_set_audit_dependencies_outcome ON dat_set_audit_dependencies(dependency_outcome);
CREATE UNIQUE INDEX verified_identity_facts_archive_kind
    ON verified_identity_facts(archive_id, kind);
CREATE INDEX verified_identity_facts_archive
    ON verified_identity_facts(archive_id);
CREATE UNIQUE INDEX dat_expected_entries_source_identity
    ON dat_expected_entries(dat_source_id, canonical_identity);
CREATE INDEX dat_expected_entries_source ON dat_expected_entries(dat_source_id);
CREATE INDEX scan_fingerprints_source ON scan_fingerprints(source_folder_id);
CREATE INDEX scan_runs_discovery_details_fingerprint
    ON scan_runs(discovery_details_source_folder_id, discovery_details_fingerprint);
COMMIT;
