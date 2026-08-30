-- Migration 0008: persist per-library-item DAT identity results so the
-- normal selected-item path can reconstruct a
-- `crate::dat::library_identity_summary::LibraryDatIdentitySummary` later
-- without re-running a DAT audit, reopening a DAT file, rehashing anything,
-- or depending on transient `DatAuditOutcome` page state.
--
-- One row per (library item, DAT source). Different DAT ecosystems/sources
-- (No-Intro, Redump, MAME, FBNeo, ...) are scoped by `dat_source_id` and
-- never overwrite each other; `library_dat_identities_for_item` returns them
-- all so the query API can report multi-source results truthfully.
--
-- `archive_id` is the stable durable library-item key (`archives.id`, the
-- surrogate every other per-item table already references). It is not the
-- identity key itself - identity is `(source_folder_id, relative_path)` -
-- but it is the join key that survives scans.
--
-- `facts_json` is a serialised
-- `crate::dat::library_identity_summary::PersistedLibraryDatIdentity`: it
-- snapshots every display string (source name, catalogue names, canonical
-- entry name, matched hash, competing candidate names, the audited hash
-- snapshot) so reconstruction never needs the original DAT file, which may
-- since have been deleted. The few columns pulled out alongside it exist
-- only so the queries here (source scoping, revision-drift marking, and the
-- "a partial run must not clobber a positive prior result" guard) do not
-- have to deserialise every row.
--
-- `verification_state` / `completeness` / `dat_ecosystem` store the
-- variant's stable machine name (never a human-readable label, which is
-- free to reword).
--
-- Arcade set / dependency verdicts are deliberately NOT stored here: that
-- persistence does not exist on this base. The query seam is designed so a
-- future `set_audit_verdicts`-style row can be LEFT JOINed on
-- `(archive_id, dat_source_id)` and folded into the reconstructed
-- summary's `set_dependency` field without changing this table.

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

-- One authoritative result per item per source. A repeated exhaustive audit
-- from the same source UPSERTs this row atomically; a different source adds
-- a separate row.
CREATE UNIQUE INDEX library_dat_identities_item_source
    ON library_dat_identities(archive_id, dat_source_id);

-- Supports both "every stored source result for this item" and the bulk
-- "this source's DAT revision changed" stale-marking sweep.
CREATE INDEX library_dat_identities_archive ON library_dat_identities(archive_id);
CREATE INDEX library_dat_identities_source ON library_dat_identities(dat_source_id);
