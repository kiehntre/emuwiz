CREATE TABLE media_topology_evidence (
    id INTEGER PRIMARY KEY,
    set_namespace TEXT NOT NULL,
    set_value TEXT NOT NULL,
    producer TEXT NOT NULL,
    producer_version TEXT NOT NULL,
    producer_schema INTEGER NOT NULL,
    source_fingerprint TEXT NOT NULL,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    refreshed_at TEXT NOT NULL,
    evidence_json BLOB NOT NULL,
    UNIQUE(set_namespace, set_value, producer, producer_version, producer_schema)
);

CREATE INDEX media_topology_evidence_key
    ON media_topology_evidence(set_namespace, set_value);
