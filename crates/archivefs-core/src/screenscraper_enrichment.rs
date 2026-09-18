//! Explicit, user-approved ScreenScraper metadata enrichment.
//!
//! This module is deliberately separate from identity and platform evidence.
//! It models a reviewable proposal and the small set of descriptive fields
//! that `ArchiveMetadata` can persist.  Applying a proposal never changes a
//! hash, platform assignment, DAT result, or identity verdict.

use serde::{Deserialize, Serialize};

use crate::ArchiveMetadata;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        };
        let encoded = serde_json::to_vec(&receipt).unwrap();
        let decoded: ScreenScraperEnrichmentReceipt = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, receipt);
        assert!(!encoded.windows(6).any(|window| window == b"secret"));
    }
}
