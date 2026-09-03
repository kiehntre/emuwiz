-- Migration 0012: one metadata row per DAT source describing the exact
-- validation generation its `dat_expected_entries` rows came from.
--
-- `dat_expected_entries` (migration 0011) holds the names; this holds the
-- provenance the names alone cannot carry: which source revision produced
-- them, how many there were, and - critically - whether the projection had
-- to skip any pathological duplicate `<game name="...">` (see
-- `crate::dat::expected_inventory`). Coverage needs all three to decide
-- whether a Full-Set claim is even permitted:
--
--  * `duplicate_names_skipped > 0` means the expected canonical identity
--    set is not proven one-to-one, so Full Set can never be `Complete` for
--    this source, however the counts line up.
--  * `source_revision` lets the caller detect that the configured DAT has
--    been revised since this inventory was captured, degrading
--    Expected/Missing/Full-Set rather than reporting a stale denominator as
--    authoritative.
--  * `entry_count` is the durable expected denominator - never
--    recomputed from a re-parse, and cross-checkable against
--    `SELECT COUNT(*) FROM dat_expected_entries` (they must agree; a
--    disagreement is a bug, not a normal state).
--
-- One row per source (`dat_source_id` PRIMARY KEY), replaced in the same
-- transaction that replaces the `dat_expected_entries` rows - the two
-- always describe the same successful validation, never a half-new
-- inventory with old metadata.

CREATE TABLE dat_expected_inventory_meta (
    dat_source_id           TEXT PRIMARY KEY,
    source_revision         TEXT,
    ecosystem               TEXT,
    entry_count             INTEGER NOT NULL,
    duplicate_names_skipped INTEGER NOT NULL DEFAULT 0,
    validated_at            TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);
