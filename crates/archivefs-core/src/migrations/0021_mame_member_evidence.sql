CREATE TABLE mame_member_evidence (
    id                 INTEGER PRIMARY KEY,
    dat_source_id      TEXT NOT NULL,
    logical_set_name   TEXT NOT NULL,
    source_path        BLOB NOT NULL,
    current_name       TEXT NOT NULL,
    file_size          INTEGER NOT NULL,
    modified_time_ns   INTEGER NOT NULL,
    sha1               TEXT,
    crc32              TEXT,
    target_set_name    TEXT,
    target_member_name TEXT,
    actionable         INTEGER NOT NULL,
    failure_reason     TEXT,
    evidence_version   TEXT NOT NULL,
    observed_at        TEXT NOT NULL,
    UNIQUE(dat_source_id, source_path, current_name)
);

CREATE INDEX mame_member_evidence_set
    ON mame_member_evidence(dat_source_id, logical_set_name);
CREATE INDEX mame_member_evidence_checksum
    ON mame_member_evidence(dat_source_id, sha1)
    WHERE sha1 IS NOT NULL;
