-- Migration 0008 (PROVISIONAL NUMBER - see collision note below): persist
-- the already-verified per-game identity facts a
-- `crate::game_identity::GameIdentityReport` produces, so Library, Doctor
-- and other read-only consumers can explain identity / launch readiness
-- without re-inspecting the content every time.
--
-- This is a CACHE / user-visible projection. It is NEVER a trust anchor:
-- launch and cheat/mod execution keep re-verifying from a fresh report (see
-- `crate::launch`'s command planners, which never read a database).
--
-- ============================================================
-- MIGRATION-NUMBER COLLISION - INTEGRATION MUST RECONCILE
-- ============================================================
-- Authoritative main (`50d4007`) has migrations only through 0007, so 0008
-- is the true next number on THIS branch and the invariant
-- `latest_schema_version() == migration count` is kept intact.
--
-- However, two other *isolated, not-yet-integrated* branches each
-- independently also use 0008:
--   * 0990e53 - library DAT identity persistence
--   * e9be4bb  - Arcade set-verdict persistence
--
-- This is a genuine three-way provisional collision. None of the three is
-- on main. Integration MUST pick an order and renumber two of the three
-- (e.g. 0008 library DAT identity, 0009 Arcade set-verdict, 0010 this),
-- then update the migration-guard tests (`disk_format::tests
-- ::the_database_schema_and_migrations_are_unchanged`,
-- `diagnostics::tests::stage_1a_introduces_no_database_migration`) and any
-- `create_representative_older_database` schema-version arguments. This
-- table's name and shape do not overlap either DAT-persistence design, so
-- only the number needs reconciling.
--
-- One row per (archive_id, IdentityKind): one verified value per fact kind
-- per catalogued archive. `archive_id` is the durable per-item join key
-- (`archives.id`), the same surrogate every other per-item table uses.
--
-- `kind` / `confidence` store the enum's stable machine name (snake_case),
-- never a human label. `confidence` is never `filename_only` for a
-- persisted row.
--
-- The `file_*` columns are a point-in-time `(device, inode, size, mtime)`
-- snapshot of the archive file, the same shape
-- `crate::launch::process_spawn::CapturedFileIdentity` uses to notice a
-- file swapped at the same path. A read derives Current / Stale / Unknown
-- freshness from it; a stale row stays visible but never authorizes a
-- launch.

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

-- One authoritative cached value per fact kind per archive. A fresh
-- exhaustive inspection UPSERTs each present kind and deletes the kinds it
-- no longer sees, atomically; an incomplete inspection only adds/updates
-- and never deletes.
CREATE UNIQUE INDEX verified_identity_facts_archive_kind
    ON verified_identity_facts(archive_id, kind);

-- "every cached fact for this archive" for the Library/Doctor read paths.
CREATE INDEX verified_identity_facts_archive
    ON verified_identity_facts(archive_id);
