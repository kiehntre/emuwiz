//! Provider-neutral, cache-first artwork and descriptive metadata evidence.
//!
//! This module is deliberately a resolver, not a provider client.  Existing
//! local, ES-DE, RomM and ScreenScraper imports can project their read-only
//! records into these candidates without adding a second network path.  A
//! repaint therefore cannot fetch, and a failed provider cannot erase a
//! previously usable cached asset.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Local,
    EsDe,
    Romm,
    ScreenScraper,
    Bundled,
}

impl Provider {
    pub const ALL: [Self; 5] = [
        Self::Local,
        Self::EsDe,
        Self::Romm,
        Self::ScreenScraper,
        Self::Bundled,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::EsDe => "ES-DE",
            Self::Romm => "RomM",
            Self::ScreenScraper => "ScreenScraper",
            Self::Bundled => "Bundled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    CoverFront,
    CoverBack,
    Screenshot,
    Logo,
    HeroBackground,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataField {
    Title,
    Description,
    ReleaseDate,
    Developer,
    Publisher,
    Genre,
    Region,
    Platform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceClass {
    /// A user-selected local override.  It wins only for the field/asset it owns.
    LocalOverride,
    /// A verified DAT/platform fact.  Descriptive providers cannot replace it.
    VerifiedIdentity,
    LocalEvidence,
    ProviderCache,
    BundledFallback,
}

impl SourceClass {
    fn rank(self) -> u16 {
        match self {
            Self::LocalOverride => 500,
            Self::VerifiedIdentity => 450,
            Self::LocalEvidence => 400,
            Self::ProviderCache => 300,
            Self::BundledFallback => 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub provider: Provider,
    pub source_class: SourceClass,
    pub retrieved_at_unix_seconds: u64,
    pub detail: String,
    pub cache_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataCandidate {
    pub field: MetadataField,
    pub value: String,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtworkCandidate {
    pub kind: AssetKind,
    pub path: PathBuf,
    pub provenance: Provenance,
    /// Whether the file was already cached locally when this candidate was built.
    pub cached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub provider: Provider,
    pub active: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedMetadata {
    pub fields: BTreeMap<MetadataField, MetadataCandidate>,
    pub artwork: BTreeMap<AssetKind, ArtworkCandidate>,
    pub conflicts: Vec<MetadataCandidate>,
    pub missing_artwork: Vec<AssetKind>,
    pub provider_status: Vec<ProviderStatus>,
    pub fallback_chain: Vec<Provider>,
}

#[derive(Debug, Default)]
pub struct AggregationInput {
    pub metadata: Vec<MetadataCandidate>,
    pub artwork: Vec<ArtworkCandidate>,
    pub provider_status: Vec<ProviderStatus>,
}

/// Projects an already-retrieved ScreenScraper result into the shared
/// provider-neutral model. This function performs no network I/O and does not
/// project identity contributions or URL-only media references into identity
/// or local artwork.
pub fn add_screenscraper(
    input: &mut AggregationInput,
    enrichment: &crate::identity_source::screenscraper::ScreenScraperEnrichment,
) {
    use crate::identity_source::screenscraper::EnrichedField;

    let add =
        |input: &mut AggregationInput, field: MetadataField, value: Option<&EnrichedField>| {
            if let Some(value) = value.filter(|value| !value.value.trim().is_empty()) {
                input.metadata.push(MetadataCandidate {
                    field,
                    value: value.value.clone(),
                    provenance: Provenance {
                        provider: Provider::ScreenScraper,
                        source_class: SourceClass::ProviderCache,
                        retrieved_at_unix_seconds: value.provenance.retrieved_at_unix_seconds,
                        detail: format!(
                            "record {} · {}",
                            value.provenance.provider_record_id, value.provenance.match_basis
                        ),
                        cache_path: None,
                    },
                });
            }
        };
    add(input, MetadataField::Title, enrichment.title.as_ref());
    add(
        input,
        MetadataField::Description,
        enrichment.description.as_ref(),
    );
    add(
        input,
        MetadataField::ReleaseDate,
        enrichment.release_date.as_ref(),
    );
    add(
        input,
        MetadataField::Developer,
        enrichment.developer.as_ref(),
    );
    add(
        input,
        MetadataField::Publisher,
        enrichment.publisher.as_ref(),
    );
    add(input, MetadataField::Genre, enrichment.genre.as_ref());
    add(input, MetadataField::Region, enrichment.region.as_ref());
    input.provider_status.push(ProviderStatus {
        provider: Provider::ScreenScraper,
        active: true,
        detail: format!("cached record {}", enrichment.provider_game_id),
    });
}

/// Resolves each field and asset independently.  This is the important
/// distinction from a provider-level winner: a cover may come from Local,
/// screenshots from ES-DE, and description from a cached ScreenScraper record.
pub fn resolve(input: AggregationInput) -> ResolvedMetadata {
    let mut fields = BTreeMap::new();
    let mut conflicts = Vec::new();
    for candidate in input.metadata {
        match fields.get(&candidate.field) {
            None => {
                fields.insert(candidate.field, candidate);
            }
            Some(current)
                if candidate.provenance.source_class.rank()
                    > current.provenance.source_class.rank() =>
            {
                if candidate.value != current.value {
                    conflicts.push(current.clone());
                }
                fields.insert(candidate.field, candidate);
            }
            Some(current) => {
                if candidate.value != current.value {
                    conflicts.push(candidate);
                }
            }
        }
    }
    let mut artwork = BTreeMap::new();
    for candidate in input.artwork {
        match artwork.get(&candidate.kind) {
            None => {
                artwork.insert(candidate.kind, candidate);
            }
            Some(current)
                if candidate.provenance.source_class.rank()
                    > current.provenance.source_class.rank() =>
            {
                artwork.insert(candidate.kind, candidate);
            }
            Some(_) => {}
        }
    }
    let missing_artwork = [
        AssetKind::CoverFront,
        AssetKind::CoverBack,
        AssetKind::Screenshot,
        AssetKind::Logo,
        AssetKind::HeroBackground,
    ]
    .into_iter()
    .filter(|kind| !artwork.contains_key(kind))
    .collect();
    let fallback_chain = Provider::ALL
        .into_iter()
        .filter(|provider| {
            input
                .provider_status
                .iter()
                .any(|status| status.provider == *provider && status.active)
        })
        .collect();
    ResolvedMetadata {
        fields,
        artwork,
        conflicts,
        missing_artwork,
        provider_status: input.provider_status,
        fallback_chain,
    }
}

/// Stable, provider-scoped cache key.  It contains no URL or secret and is
/// suitable for an existing persistent artwork-cache directory.
pub fn artwork_cache_key(
    identity: &str,
    provider: Provider,
    kind: AssetKind,
    reference: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(identity.as_bytes());
    digest.update([0]);
    digest.update(provider.label().as_bytes());
    digest.update([0]);
    digest.update(format!("{kind:?}").as_bytes());
    digest.update([0]);
    digest.update(reference.as_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Cache reads are intentionally explicit.  This helper does not create,
/// fetch, or replace anything; stale cached media remains a valid candidate.
pub fn cached_artwork(
    path: &Path,
    provenance: Provenance,
    kind: AssetKind,
) -> Option<ArtworkCandidate> {
    path.is_file().then_some(ArtworkCandidate {
        kind,
        path: path.to_path_buf(),
        provenance,
        cached: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn meta(
        provider: Provider,
        class: SourceClass,
        field: MetadataField,
        value: &str,
    ) -> MetadataCandidate {
        MetadataCandidate {
            field,
            value: value.into(),
            provenance: Provenance {
                provider,
                source_class: class,
                retrieved_at_unix_seconds: 1,
                detail: "fixture".into(),
                cache_path: None,
            },
        }
    }
    fn art(
        provider: Provider,
        class: SourceClass,
        kind: AssetKind,
        name: &str,
    ) -> ArtworkCandidate {
        ArtworkCandidate {
            kind,
            path: PathBuf::from(name),
            cached: true,
            provenance: Provenance {
                provider,
                source_class: class,
                retrieved_at_unix_seconds: 1,
                detail: "fixture".into(),
                cache_path: Some(PathBuf::from(name)),
            },
        }
    }
    #[test]
    fn each_provider_can_supply_a_local_only_result() {
        for provider in Provider::ALL {
            let result = resolve(AggregationInput {
                metadata: vec![meta(
                    provider,
                    SourceClass::ProviderCache,
                    MetadataField::Title,
                    "game",
                )],
                ..Default::default()
            });
            assert_eq!(
                result.fields[&MetadataField::Title].provenance.provider,
                provider
            );
        }
    }
    #[test]
    fn providers_contribute_independent_fields() {
        let result = resolve(AggregationInput {
            metadata: vec![
                meta(
                    Provider::EsDe,
                    SourceClass::ProviderCache,
                    MetadataField::Title,
                    "title",
                ),
                meta(
                    Provider::ScreenScraper,
                    SourceClass::ProviderCache,
                    MetadataField::Genre,
                    "action",
                ),
            ],
            artwork: vec![art(
                Provider::Local,
                SourceClass::LocalOverride,
                AssetKind::CoverFront,
                "cover",
            )],
            ..Default::default()
        });
        assert_eq!(
            result.fields[&MetadataField::Genre].provenance.provider,
            Provider::ScreenScraper
        );
        assert_eq!(
            result.artwork[&AssetKind::CoverFront].provenance.provider,
            Provider::Local
        );
    }
    #[test]
    fn local_override_and_verified_identity_are_not_replaced() {
        let result = resolve(AggregationInput {
            metadata: vec![
                meta(
                    Provider::Romm,
                    SourceClass::ProviderCache,
                    MetadataField::Platform,
                    "wrong",
                ),
                meta(
                    Provider::Local,
                    SourceClass::VerifiedIdentity,
                    MetadataField::Platform,
                    "SNES",
                ),
            ],
            artwork: vec![
                art(
                    Provider::Local,
                    SourceClass::LocalOverride,
                    AssetKind::CoverFront,
                    "local",
                ),
                art(
                    Provider::Romm,
                    SourceClass::ProviderCache,
                    AssetKind::CoverFront,
                    "romm",
                ),
            ],
            ..Default::default()
        });
        assert_eq!(result.fields[&MetadataField::Platform].value, "SNES");
        assert_eq!(
            result.artwork[&AssetKind::CoverFront].path,
            PathBuf::from("local")
        );
        assert_eq!(result.conflicts.len(), 1);
    }
    #[test]
    fn unavailable_provider_does_not_erase_cached_art() {
        let result = resolve(AggregationInput {
            artwork: vec![art(
                Provider::Romm,
                SourceClass::ProviderCache,
                AssetKind::CoverFront,
                "stale-cache",
            )],
            provider_status: vec![ProviderStatus {
                provider: Provider::Romm,
                active: false,
                detail: "unavailable; cache retained".into(),
            }],
            ..Default::default()
        });
        assert_eq!(
            result.artwork[&AssetKind::CoverFront].path,
            PathBuf::from("stale-cache")
        );
    }
    #[test]
    fn cache_key_is_stable_and_provider_scoped() {
        assert_eq!(
            artwork_cache_key("id", Provider::Romm, AssetKind::CoverFront, "ref"),
            artwork_cache_key("id", Provider::Romm, AssetKind::CoverFront, "ref")
        );
        assert_ne!(
            artwork_cache_key("id", Provider::Romm, AssetKind::CoverFront, "ref"),
            artwork_cache_key("id", Provider::EsDe, AssetKind::CoverFront, "ref")
        );
    }

    #[test]
    fn screenscraper_adapter_adds_descriptive_fields_only() {
        use crate::identity_source::screenscraper::{
            EnrichedField, IdentityContribution, MetadataProvenance, ScreenScraperEnrichment,
        };
        let field = |value: &str| EnrichedField {
            value: value.into(),
            provenance: MetadataProvenance {
                provider: "ScreenScraper",
                provider_record_id: "42".into(),
                retrieved_at_unix_seconds: 7,
                match_basis: "hash match".into(),
            },
        };
        let enrichment = ScreenScraperEnrichment {
            identity_contribution: IdentityContribution::None,
            provider_game_id: "42".into(),
            title: Some(field("Provider title")),
            alternative_title: None,
            description: Some(field("Description")),
            release_date: None,
            developer: None,
            publisher: None,
            genre: Some(field("RPG")),
            players: None,
            rating: None,
            region: None,
            language: None,
            external_url: None,
            media_references: Vec::new(),
        };
        let mut input = AggregationInput::default();
        add_screenscraper(&mut input, &enrichment);
        let resolved = resolve(input);
        assert_eq!(
            resolved.fields[&MetadataField::Title].value,
            "Provider title"
        );
        assert_eq!(resolved.fields[&MetadataField::Genre].value, "RPG");
        assert!(resolved
            .fields
            .values()
            .all(|candidate| candidate.provenance.provider == Provider::ScreenScraper));
        assert!(resolved.fields.get(&MetadataField::Platform).is_none());
    }
}
