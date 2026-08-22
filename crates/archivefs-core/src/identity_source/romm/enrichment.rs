//! Converts a cached RomM identity record into display-only game
//! enrichment - synopsis, genre, players, rating, release year.
//!
//! # Architecture rule: enrichment is not identity
//!
//! [`enrichment_metadata`] reads exactly five fields from
//! [`ExternalIdentityRecord`] (`synopsis`, `genres`, `players`, `rating`,
//! `release_year`) and writes exactly those into [`ArchiveMetadata`],
//! alongside a human-readable `source`. It never reads or writes `title`,
//! `platform_candidate`, `hashes`, `verification`, or any other field that
//! feeds identity/preservation decisions, and [`RommMetadataProvider`]
//! never mutates the [`IdentityCache`] it reads from. A game with a
//! RomM-confirmed hash but no RomM metadata enrichment is exactly as
//! trustworthy as one that never had RomM configured at all - enrichment
//! success or failure changes nothing about what EmuWiz believes a file is.
//!
//! # No network here
//!
//! [`RommMetadataProvider`] only ever reads an already-loaded
//! [`IdentityCache`] passed in by the caller. Opening that cache (and any
//! network fetch to refresh it) is the caller's responsibility, exactly as
//! it already is for [`super::client`]'s cover-artwork lookups - this
//! module adds no new cache, no new network path, and no new persistence
//! format.

use crate::identity_source::cache::IdentityCache;
use crate::identity_source::model::ExternalIdentityRecord;
use crate::{Archive, ArchiveMetadata, MetadataProvider};

/// Builds display-only enrichment from one cached RomM record.
///
/// Fields the record does not carry stay `None`/empty in the result -
/// exactly like a record from a provider that was never configured. This
/// function cannot fail: there is no network, no parsing, nothing that
/// distinguishes "this record has no synopsis" from "the lookup broke".
pub fn enrichment_metadata(record: &ExternalIdentityRecord) -> ArchiveMetadata {
    let mut metadata = ArchiveMetadata::empty();
    metadata.synopsis = record.synopsis.clone();
    metadata.genre = (!record.genres.is_empty()).then(|| record.genres.join(", "));
    metadata.players = record.players.clone();
    metadata.rating = record.rating;
    metadata.release_year = record.release_year;
    if record.has_game_information() {
        metadata.source = Some(record.provider.label().to_string());
    }
    metadata
}

/// A [`MetadataProvider`] over an already-loaded, already-indexed
/// [`IdentityCache`] - a cache-only lookup by the archive's local path,
/// exactly the matching convention [`super::client`]'s cover lookups
/// already use ([`IdentityCache::record_for_path`]/
/// `ExternalIdentityRecord::archivefs_path`).
///
/// Deliberately does not open, index, or refresh the cache itself: a
/// caller that already holds one (as the Gamer View cover worker already
/// does, to avoid loading it twice) constructs this over a reference to
/// it. Opening/refreshing is a separate, explicit, caller-driven step -
/// never something this type does on the caller's behalf.
pub struct RommMetadataProvider<'a> {
    cache: &'a IdentityCache,
}

impl<'a> RommMetadataProvider<'a> {
    pub fn new(cache: &'a IdentityCache) -> Self {
        Self { cache }
    }
}

impl MetadataProvider for RommMetadataProvider<'_> {
    /// Cache-only: matches by exact local path only, the same path an
    /// already-confirmed RomM identity match already established. A path
    /// with no cached record - not yet synced, not matched, or RomM not
    /// configured at all - returns [`ArchiveMetadata::empty()`], never an
    /// error and never a guess.
    fn metadata_for(&self, archive: &Archive) -> ArchiveMetadata {
        match self.cache.record_for_path(&archive.path) {
            Some(record) => enrichment_metadata(record),
            None => ArchiveMetadata::empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::identity_source::model::{
        ExternalVerification, IdentityProvider, MetadataProviderId,
    };
    use crate::{ArchiveHealth, ArchiveIdentity, archive_kind};

    /// Builds an `Archive` with no filesystem access at all - `archive_kind`
    /// and `ArchiveIdentity::from_path` both work from the path alone.
    fn archive_at(path: &str) -> Archive {
        let path = PathBuf::from(path);
        Archive {
            kind: archive_kind(&path).expect("a recognised archive extension"),
            identity: ArchiveIdentity::from_path(&path, PathBuf::new(), None),
            path,
            health: ArchiveHealth::Pending,
        }
    }

    fn bare_record() -> ExternalIdentityRecord {
        ExternalIdentityRecord {
            provider: IdentityProvider::Romm,
            server_id: "https://romm.example".to_string(),
            provider_platform_id: Some("7".to_string()),
            provider_game_id: "42".to_string(),
            provider_file_id: None,
            provider_path: "roms/gba/game.zip".to_string(),
            archivefs_path: Some(PathBuf::from("/library/gba/game.zip")),
            title: Some("Strong DAT-Verified Title".to_string()),
            platform_candidate: Some("Game Boy Advance".to_string()),
            provider_platform_name: Some("gba".to_string()),
            regions: vec!["USA".to_string()],
            revision: None,
            hashes: Vec::new(),
            file_size_bytes: Some(1024),
            metadata_provider_ids: vec![MetadataProviderId {
                provider: "igdb".to_string(),
                id: "999".to_string(),
            }],
            artwork: None,
            related_files: Vec::new(),
            sibling_game_ids: Vec::new(),
            imported_at_unix_seconds: 1_785_000_000,
            provider_updated_at: None,
            verification: ExternalVerification::ConfirmedExternal,
            conflicts: Vec::new(),
            evidence: vec!["hash confirmed".to_string()],
            synopsis: None,
            genres: Vec::new(),
            players: None,
            rating: None,
            release_year: None,
        }
    }

    #[test]
    fn a_record_with_no_enrichment_fields_produces_empty_metadata_with_no_source() {
        let record = bare_record();
        let metadata = enrichment_metadata(&record);
        assert_eq!(metadata.synopsis, None);
        assert_eq!(metadata.genre, None);
        assert_eq!(metadata.players, None);
        assert_eq!(metadata.rating, None);
        assert_eq!(metadata.release_year, None);
        assert_eq!(
            metadata.source, None,
            "no source should be claimed when nothing was actually enriched"
        );
    }

    #[test]
    fn enrichment_never_touches_identity_relevant_fields() {
        let mut record = bare_record();
        record.synopsis = Some("A short adventure.".to_string());
        record.genres = vec!["Platformer".to_string(), "Action".to_string()];
        record.players = Some("1-2".to_string());
        record.rating = Some(87);
        record.release_year = Some(2003);

        let metadata = enrichment_metadata(&record);

        // The whole point of this test: identity-relevant fields the
        // record carries (title, platform, verification-worthy evidence)
        // must never leak into the enrichment-only ArchiveMetadata this
        // function produces - a strong DAT/hash-verified title must never
        // be silently replaced by anything derived here.
        assert_eq!(metadata.title, None);
        assert_eq!(metadata.platform, None);
        assert_eq!(metadata.region, None);
        assert_eq!(metadata.version, None);
        assert_eq!(metadata.disc, None);
        assert_eq!(metadata.publisher, None);
        assert_eq!(metadata.developer, None);
        assert_eq!(metadata.notes, None);

        // Only the enrichment fields are populated.
        assert_eq!(metadata.synopsis.as_deref(), Some("A short adventure."));
        assert_eq!(metadata.genre.as_deref(), Some("Platformer, Action"));
        assert_eq!(metadata.players.as_deref(), Some("1-2"));
        assert_eq!(metadata.rating, Some(87));
        assert_eq!(metadata.release_year, Some(2003));
        assert_eq!(metadata.source.as_deref(), Some("RomM"));
    }

    #[test]
    fn a_single_genre_is_not_given_a_trailing_separator() {
        let mut record = bare_record();
        record.genres = vec!["RPG".to_string()];
        let metadata = enrichment_metadata(&record);
        assert_eq!(metadata.genre.as_deref(), Some("RPG"));
    }

    #[test]
    fn provider_lookup_by_unmatched_path_returns_empty_metadata_not_an_error() {
        let cache = IdentityCache {
            format_version: crate::identity_source::cache::CACHE_FORMAT_VERSION,
            provider: IdentityProvider::Romm,
            server_id: "https://romm.example".to_string(),
            server_version: None,
            source_fingerprint: "f".to_string(),
            imported_at_unix_seconds: 1,
            platforms: Vec::new(),
            records: Vec::new(),
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_reported_total: None,
        };
        let provider = RommMetadataProvider::new(&cache);
        let archive = archive_at("/library/gba/never-synced.zip");
        let metadata = provider.metadata_for(&archive);
        assert_eq!(metadata.synopsis, None);
        assert_eq!(metadata.source, None);
    }

    #[test]
    fn provider_lookup_by_matched_path_returns_its_enrichment() {
        let mut record = bare_record();
        record.synopsis = Some("Explore a mysterious planet.".to_string());
        record.rating = Some(91);
        let path = record.archivefs_path.clone().unwrap();

        let cache = IdentityCache {
            format_version: crate::identity_source::cache::CACHE_FORMAT_VERSION,
            provider: IdentityProvider::Romm,
            server_id: "https://romm.example".to_string(),
            server_version: None,
            source_fingerprint: "f".to_string(),
            imported_at_unix_seconds: 1,
            platforms: Vec::new(),
            records: vec![record],
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_reported_total: None,
        };
        let provider = RommMetadataProvider::new(&cache);
        let archive = archive_at(path.to_str().expect("a valid utf-8 path"));
        let metadata = provider.metadata_for(&archive);
        assert_eq!(
            metadata.synopsis.as_deref(),
            Some("Explore a mysterious planet.")
        );
        assert_eq!(metadata.rating, Some(91));
        assert_eq!(metadata.source.as_deref(), Some("RomM"));
    }
}
