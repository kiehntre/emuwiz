-- Migration 0007: persist every ingestion-discovery item found by a scan
-- (not just the bounded in-memory sample `ScanPersistSummary` already
-- carries), so a Collection Discovery view can page through the full
-- result set of a large collection without holding it all in memory.
--
-- One row per `crate::ingestion::GameDiscovery` item, tagged with the
-- `scan_runs` row (`DiscoveryRunId` = `scan_run_id`) that found it -
-- `scan_runs.status` already distinguishes running/completed/failed/
-- interrupted, so no separate run-state model is needed here.
--
-- `container`/`content`/`validation_state`/`skip_reason` store the
-- variant's stable machine name (never the human-readable label(), which
-- is free to reword), so filtering never depends on display text.
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

-- Supports both "every row for this run, in deterministic (insertion)
-- order" paging and the bounded-history cleanup delete.
CREATE INDEX discovery_details_run_order ON discovery_details(scan_run_id, id);
