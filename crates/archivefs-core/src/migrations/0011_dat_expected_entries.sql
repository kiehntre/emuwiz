-- Migration 0011: durable, named expected DAT inventory.
--
-- Everything before this migration that survives a validation run is
-- numeric (`dat_sources.toml`'s `DatSourceHealth.entry_count`/`rom_count`)
-- or scoped to items the library already owns (`library_dat_identities`,
-- `dat_set_audit_results` - both only ever gain a row for something a local
-- file was actually compared against). None of those can answer "what does
-- this DAT source expect that the library has zero representation of at
-- all" - that requires the DAT's own declared identities, which otherwise
-- exist only transiently inside `ParsedDat` while a DAT file is open, and
-- are discarded the moment parsing finishes.
--
-- One row per `<game>`/`<machine>` element a source's DAT file(s) declare,
-- keyed by the exact, unmodified name the catalogue itself assigned
-- (`crate::dat::expected_inventory`'s `canonical_identity` - the same
-- string `PersistedLibraryDatIdentity.canonical.canonical_dat_name` already
-- stores for a verified match, so comparing the two needs no new
-- normalisation). No ecosystem-specific unit: a Redump disc-level `<game>`,
-- a No-Intro release, a TOSEC entry, and a MAME `<machine>` are all exactly
-- one row here, at whatever granularity that ecosystem's own DAT already
-- declares.
--
-- `dat_source_id` is the same stable id `library_dat_identities` and
-- `dat_set_audit_results` already scope by. A row here says nothing about
-- whether the library owns or has verified this identity - that comparison
-- is made by joining against those tables at read time, never duplicated
-- into this one.

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

-- One row per identity per source. A DAT that (pathologically) declares the
-- same `<game name="...">` twice is not represented twice here - see
-- `crate::dat::expected_inventory`'s doc for why that is refused rather
-- than guessed, exactly like `dat::set`'s existing `DuplicateGameName`
-- handling for the same underlying ambiguity.
CREATE UNIQUE INDEX dat_expected_entries_source_identity
    ON dat_expected_entries(dat_source_id, canonical_identity);

-- Supports "how many does this source expect" and the full-replacement
-- sweep ("delete every row for this source before inserting the new set").
CREATE INDEX dat_expected_entries_source ON dat_expected_entries(dat_source_id);
