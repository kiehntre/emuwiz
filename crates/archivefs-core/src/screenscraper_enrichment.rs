//! Explicit, user-approved ScreenScraper metadata enrichment.
//!
//! This module is deliberately separate from identity and platform evidence.
//! It models a reviewable proposal and the small set of descriptive fields
//! that `ArchiveMetadata` can persist.  Applying a proposal never changes a
//! hash, platform assignment, DAT result, or identity verdict.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ArchiveMetadata;

/// Maximum number of explicitly selected games in one foreground enrichment
/// session.  This is deliberately small: ScreenScraper requests remain
/// user-triggered, bounded, and reviewable rather than becoming a library
/// crawler.
pub const MAX_EXPLICIT_BATCH_SIZE: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenScraperBatchLookupRoute {
    DirectProviderId(u64),
    Search,
}

/// Stored provider IDs are used only when they are valid numeric ScreenScraper
/// IDs. Anything else deliberately falls back to the bounded search path.
pub fn batch_lookup_route(provider_record_id: Option<&str>) -> ScreenScraperBatchLookupRoute {
    provider_record_id
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(
            ScreenScraperBatchLookupRoute::Search,
            ScreenScraperBatchLookupRoute::DirectProviderId,
        )
}

pub fn validate_explicit_batch_size(selected_count: usize) -> Result<(), String> {
    if selected_count == 0 {
        Err("Select one or more games first.".into())
    } else if selected_count > MAX_EXPLICIT_BATCH_SIZE {
        Err(format!(
            "Select no more than {MAX_EXPLICIT_BATCH_SIZE} games per enrichment batch."
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AcceptedScreenScraperMetadata {
    pub title: Option<String>,
    pub synopsis: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub genre: Option<String>,
    pub players: Option<String>,
    pub region: Option<String>,
    pub languages: Option<Vec<String>>,
    pub release_year: Option<u16>,
}

impl AcceptedScreenScraperMetadata {
    pub fn has_field(&self, field: EnrichmentField) -> bool {
        match field {
            EnrichmentField::Title => self.title.is_some(),
            EnrichmentField::Synopsis => self.synopsis.is_some(),
            EnrichmentField::Developer => self.developer.is_some(),
            EnrichmentField::Publisher => self.publisher.is_some(),
            EnrichmentField::Genre => self.genre.is_some(),
            EnrichmentField::Players => self.players.is_some(),
            EnrichmentField::Region => self.region.is_some(),
            EnrichmentField::Languages => self.languages.is_some(),
            EnrichmentField::ReleaseYear => self.release_year.is_some(),
        }
    }

    pub fn apply_to(&self, metadata: &mut ArchiveMetadata) {
        if let Some(value) = &self.title {
            metadata.title = Some(value.clone());
        }
        if let Some(value) = &self.synopsis {
            metadata.synopsis = Some(value.clone());
        }
        if let Some(value) = &self.developer {
            metadata.developer = Some(value.clone());
        }
        if let Some(value) = &self.publisher {
            metadata.publisher = Some(value.clone());
        }
        if let Some(value) = &self.genre {
            metadata.genre = Some(value.clone());
        }
        if let Some(value) = &self.players {
            metadata.players = Some(value.clone());
        }
        if let Some(value) = &self.region {
            metadata.region = Some(value.clone());
        }
        if let Some(value) = &self.languages {
            metadata.languages = Some(value.clone());
        }
        if let Some(value) = self.release_year {
            metadata.release_year = Some(value);
        }
    }

    pub fn from_existing(metadata: &ArchiveMetadata) -> Self {
        Self {
            title: metadata.title.clone(),
            synopsis: metadata.synopsis.clone(),
            developer: metadata.developer.clone(),
            publisher: metadata.publisher.clone(),
            genre: metadata.genre.clone(),
            players: metadata.players.clone(),
            region: metadata.region.clone(),
            languages: metadata.languages.clone(),
            release_year: metadata.release_year,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenScraperEnrichmentReceipt {
    pub provider: String,
    pub provider_record_id: String,
    pub retrieved_at_unix_seconds: u64,
    pub match_basis: String,
    pub before: AcceptedScreenScraperMetadata,
    pub accepted: AcceptedScreenScraperMetadata,
    pub media_reference_count: usize,
    /// Original source for retained fields when a later batch accepts only
    /// some new fields. The map is bounded by EnrichmentField::ALL.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub retained_field_sources: BTreeMap<EnrichmentField, ScreenScraperFieldSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenScraperFieldSource {
    pub provider_record_id: String,
    pub retrieved_at_unix_seconds: u64,
    pub match_basis: String,
}

impl ScreenScraperEnrichmentReceipt {
    pub fn source_for(&self, field: EnrichmentField) -> ScreenScraperFieldSource {
        self.retained_field_sources
            .get(&field)
            .cloned()
            .unwrap_or_else(|| ScreenScraperFieldSource {
                provider_record_id: self.provider_record_id.clone(),
                retrieved_at_unix_seconds: self.retrieved_at_unix_seconds,
                match_basis: self.match_basis.clone(),
            })
    }
}

pub fn retained_field_sources(
    prior_values: Option<&AcceptedScreenScraperMetadata>,
    prior_receipt: Option<&ScreenScraperEnrichmentReceipt>,
    accepted: &AcceptedScreenScraperMetadata,
) -> BTreeMap<EnrichmentField, ScreenScraperFieldSource> {
    let mut sources = BTreeMap::new();
    if let (Some(prior_values), Some(prior_receipt)) = (prior_values, prior_receipt) {
        for field in EnrichmentField::ALL {
            if prior_values.has_field(field) && !accepted.has_field(field) {
                sources.insert(field, prior_receipt.source_for(field));
            }
        }
    }
    sources
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedScreenScraperEnrichment {
    pub archive_id: i64,
    pub values: AcceptedScreenScraperMetadata,
    pub receipt: ScreenScraperEnrichmentReceipt,
}

/// A field whose value may be reviewed in the GUI.  Provider-only fields such
/// as alternative titles and media URLs stay in the candidate/receipt and are
/// never silently squeezed into an unrelated local field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EnrichmentField {
    Title,
    Synopsis,
    Developer,
    Publisher,
    Genre,
    Players,
    Region,
    Languages,
    ReleaseYear,
}

impl EnrichmentField {
    pub const ALL: [Self; 9] = [
        Self::Title,
        Self::Synopsis,
        Self::Developer,
        Self::Publisher,
        Self::Genre,
        Self::Players,
        Self::Region,
        Self::Languages,
        Self::ReleaseYear,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Synopsis => "Description",
            Self::Developer => "Developer",
            Self::Publisher => "Publisher",
            Self::Genre => "Genre",
            Self::Players => "Players",
            Self::Region => "Region",
            Self::Languages => "Language",
            Self::ReleaseYear => "Release year",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_provider_metadata_does_not_touch_identity_fields() {
        let mut metadata = ArchiveMetadata {
            title: Some("Local title".into()),
            platform: Some("SNES".into()),
            ..ArchiveMetadata::empty()
        };
        let proposal = AcceptedScreenScraperMetadata {
            title: Some("Provider title".into()),
            synopsis: Some("Description".into()),
            ..Default::default()
        };
        proposal.apply_to(&mut metadata);
        assert_eq!(metadata.title.as_deref(), Some("Provider title"));
        assert_eq!(metadata.platform.as_deref(), Some("SNES"));
    }

    #[test]
    fn receipt_round_trips_without_secrets_or_media_bytes() {
        let receipt = ScreenScraperEnrichmentReceipt {
            provider: "ScreenScraper".into(),
            provider_record_id: "42".into(),
            retrieved_at_unix_seconds: 7,
            match_basis: "platform-aware search".into(),
            before: Default::default(),
            accepted: AcceptedScreenScraperMetadata {
                genre: Some("RPG".into()),
                ..Default::default()
            },
            media_reference_count: 2,
            retained_field_sources: Default::default(),
        };
        let encoded = serde_json::to_vec(&receipt).unwrap();
        let decoded: ScreenScraperEnrichmentReceipt = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, receipt);
        assert!(!encoded.windows(6).any(|window| window == b"secret"));
    }

    #[test]
    fn retained_fields_keep_original_provenance_across_repeated_acceptance() {
        let original = ScreenScraperEnrichmentReceipt {
            provider: "ScreenScraper".into(),
            provider_record_id: "first".into(),
            retrieved_at_unix_seconds: 10,
            match_basis: "hash".into(),
            before: Default::default(),
            accepted: Default::default(),
            media_reference_count: 0,
            retained_field_sources: Default::default(),
        };
        let prior_values = AcceptedScreenScraperMetadata {
            synopsis: Some("description".into()),
            genre: Some("Adventure".into()),
            ..Default::default()
        };
        let accepted = AcceptedScreenScraperMetadata {
            genre: Some("Puzzle".into()),
            ..Default::default()
        };
        let retained = retained_field_sources(Some(&prior_values), Some(&original), &accepted);
        assert_eq!(retained.len(), 1);
        assert_eq!(
            retained[&EnrichmentField::Synopsis].provider_record_id,
            "first"
        );
        let second = ScreenScraperEnrichmentReceipt {
            provider_record_id: "second".into(),
            retained_field_sources: retained,
            ..original.clone()
        };
        let later = retained_field_sources(Some(&prior_values), Some(&second), &accepted);
        assert_eq!(
            later[&EnrichmentField::Synopsis].provider_record_id,
            "first"
        );
        assert_eq!(later.len(), 1);
        let legacy = serde_json::to_value(&original).unwrap();
        assert!(legacy.get("retained_field_sources").is_none());
        assert_eq!(
            serde_json::from_value::<ScreenScraperEnrichmentReceipt>(legacy).unwrap(),
            original
        );
    }

    #[test]
    fn explicit_batch_size_is_bounded() {
        assert!(validate_explicit_batch_size(1).is_ok());
        assert!(validate_explicit_batch_size(MAX_EXPLICIT_BATCH_SIZE).is_ok());
        assert!(validate_explicit_batch_size(MAX_EXPLICIT_BATCH_SIZE + 1).is_err());
        assert!(validate_explicit_batch_size(0).is_err());
    }

    #[test]
    fn stored_provider_id_prefers_direct_lookup_but_invalid_values_search() {
        assert_eq!(
            batch_lookup_route(Some("42")),
            ScreenScraperBatchLookupRoute::DirectProviderId(42)
        );
        assert_eq!(
            batch_lookup_route(Some("not-an-id")),
            ScreenScraperBatchLookupRoute::Search
        );
        assert_eq!(
            batch_lookup_route(None),
            ScreenScraperBatchLookupRoute::Search
        );
    }
}
