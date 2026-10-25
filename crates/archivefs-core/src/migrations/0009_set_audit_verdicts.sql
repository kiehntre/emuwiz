-- Current set verdicts are scoped by source_id and game name. A MAME and an
-- FBNeo catalogue may use the same shortname, but their results are never
-- allowed to replace or satisfy one another.
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

CREATE INDEX dat_set_audit_results_archive ON dat_set_audit_results(archive_path);
CREATE INDEX dat_set_audit_results_source ON dat_set_audit_results(source_id);
CREATE INDEX dat_set_audit_results_stale ON dat_set_audit_results(stale) WHERE stale = 1;
CREATE INDEX dat_set_audit_results_state ON dat_set_audit_results(set_state_json);

CREATE TABLE dat_set_audit_dependencies (
    id                  INTEGER PRIMARY KEY,
    result_id           INTEGER NOT NULL REFERENCES dat_set_audit_results(id) ON DELETE CASCADE,
    dependency_kind     TEXT NOT NULL,
    dependency_outcome TEXT NOT NULL,
    target_json         TEXT NOT NULL,
    via_member          TEXT,
    UNIQUE(result_id, dependency_kind, dependency_outcome, target_json, via_member)
);

CREATE INDEX dat_set_audit_dependencies_result ON dat_set_audit_dependencies(result_id);
CREATE INDEX dat_set_audit_dependencies_outcome ON dat_set_audit_dependencies(dependency_outcome);
