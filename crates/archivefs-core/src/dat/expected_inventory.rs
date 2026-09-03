//! Projecting a DAT source's already-parsed [`ParsedDat`] into the
//! durable, named expected-entry records
//! [`crate::database::Database::replace_expected_dat_inventory`] persists.
//!
//! Pure: reads only `games: &[DatGameEntry]` a parse the caller already
//! performed (see `crate::dat::sources::validation::validate_dat_source`,
//! which calls this once per DAT file inside the loop that already calls
//! `parse_dat_file` - no second parse). No I/O, no database access, no
//! hashing.
//!
//! # Canonical identity: the DAT's own `<game name="...">`, unmodified
//!
//! [`ExpectedDatEntryRecord::canonical_identity`] is exactly
//! [`crate::dat::model::DatGameEntry::name`] - the same string
//! [`crate::dat::library_identity_summary::DatCanonicalIdentity::canonical_dat_name`]
//! already stores verbatim for a verified match (see
//! [`crate::dat::library_identity_summary::verdict_names`] and
//! `summarize_library_dat_identity`'s canonical-name construction). Coverage
//! can therefore compare "does a verified identity represent this expected
//! entry" as a plain string equality join, with no new normalisation layer
//! and no risk of the two sides disagreeing about what "the same identity"
//! means.
//!
//! This works because, in every DAT ecosystem this crate supports, the
//! `<game>` `name` attribute is already the field that disambiguates
//! release variants - a No-Intro/Redump/TOSEC DAT bakes region and revision
//! directly into the name (`"Super Mario Bros. (USA)"` and
//! `"Super Mario Bros. (Europe)"` are two different `<game>` elements with
//! two different names, not one entry with two labels), and a Redump
//! multi-disc release is already one `<game>` per disc. No per-ecosystem
//! region/revision extraction is needed for correctness: whatever
//! distinguishes two releases in a well-formed DAT is already baked into
//! `name`, and this module defers to that entirely rather than re-deriving
//! it.
//!
//! # A duplicate `<game name="...">` is refused, never guessed
//!
//! [`crate::dat::set::NeedsReviewReason::DuplicateGameName`] already
//! documents that `name` is not *guaranteed* unique within a DAT - a
//! malformed or pathological catalogue can declare the same name twice.
//! [`project_expected_dat_inventory`] applies the identical policy: the
//! first `<game>` with a given name is projected, every later one sharing
//! that exact name is counted in
//! [`ExpectedDatInventoryProjection::duplicate_names_skipped`] and never
//! silently merged, overwritten, or renamed into uniqueness. This mirrors
//! `dat_expected_entries`'s own `UNIQUE(dat_source_id, canonical_identity)`
//! constraint - the projection can never hand the database layer two rows
//! that would collide.

use serde::{Deserialize, Serialize};

use super::model::DatGameEntry;

/// The small, optional part of [`ExpectedDatEntryRecord`] that is worth
/// keeping but not worth its own column - stored as `metadata_json` on
/// `dat_expected_entries`. Never consulted for matching; `canonical_identity`
/// alone is the match key (see this module's doc).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedDatEntryMetadata {
    pub dat_game_id: Option<String>,
    pub rom_count: usize,
}

/// One DAT-declared identity, projected and ready to persist. Deliberately
/// small: for a several-hundred-thousand-entry MAME DAT this must stay far
/// smaller than the full [`DatGameEntry`] it came from (no ROM/disk member
/// list, no original-metadata blob).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedDatEntryRecord {
    /// The DAT's own `<game name="...">` / `<machine name="...">`,
    /// unmodified - the durable match key. See this module's doc for why
    /// this alone is the right key.
    pub canonical_identity: String,
    /// `description`, when the DAT declares one (most Logiqx DATs do);
    /// falls back to `canonical_identity` for a ClrMamePro-style DAT that
    /// does not.
    pub display_name: String,
    /// The DAT's own `<game id="...">`, when it declares one (No-Intro
    /// style catalogues). Not used for matching - preserved as optional
    /// provenance only, since not every ecosystem publishes it.
    pub dat_game_id: Option<String>,
    /// How many `<rom>` entries this identity declares. Provenance /
    /// diagnostic only - never a completeness signal by itself.
    pub rom_count: usize,
}

/// What one DAT source's parsed content projects to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpectedDatInventoryProjection {
    pub entries: Vec<ExpectedDatEntryRecord>,
    /// `<game>` elements sharing an already-seen exact name within this
    /// source - refused rather than persisted twice or merged. See this
    /// module's doc.
    pub duplicate_names_skipped: usize,
    /// Every name already seen, across every file folded into this
    /// projection so far. A `HashSet` rather than a linear scan of
    /// `entries`: a several-hundred-thousand-entry MAME DAT must stay
    /// O(entries), not O(entries^2).
    seen: std::collections::HashSet<String>,
}

impl ExpectedDatInventoryProjection {
    /// Merges `games` into this projection, keeping every name already seen
    /// (across every DAT file already folded in) unique. Called once per
    /// file in a folder source, so a duplicate spanning two files in the
    /// same source is caught exactly like a duplicate within one file.
    pub fn extend_from(&mut self, games: &[DatGameEntry]) {
        self.entries.reserve(games.len());
        for game in games {
            if !self.seen.insert(game.name.clone()) {
                self.duplicate_names_skipped += 1;
                continue;
            }
            self.entries.push(ExpectedDatEntryRecord {
                canonical_identity: game.name.clone(),
                display_name: game
                    .description
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| game.name.clone()),
                dat_game_id: game.id.clone(),
                rom_count: game.roms.len(),
            });
        }
    }
}

/// Projects one already-parsed DAT file's games into expected-entry
/// records. A convenience wrapper around
/// [`ExpectedDatInventoryProjection::extend_from`] for the common
/// single-file case; a folder source instead builds one
/// [`ExpectedDatInventoryProjection`] and calls `extend_from` once per file
/// so duplicate detection spans the whole source.
pub fn project_expected_dat_inventory(games: &[DatGameEntry]) -> ExpectedDatInventoryProjection {
    let mut projection = ExpectedDatInventoryProjection::default();
    projection.extend_from(games);
    projection
}

#[cfg(test)]
mod tests;
