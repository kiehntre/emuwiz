//! Base-only results are reproducible from the immutable catalogue snapshot.
//! Retain provider-specific results; format generic diagnostics only on demand.
use super::*;
use std::borrow::Cow;

pub(super) fn base_metadata(game: &super::super::library::Game) -> Vec<MetadataCandidate> {
    [
        (MetadataField::Title, &game.title),
        (MetadataField::Platform, &game.platform),
    ]
    .into_iter()
    .filter(|(_, value)| !value.trim().is_empty())
    .map(|(field, value)| MetadataCandidate {
        field,
        value: value.clone(),
        provenance: Provenance {
            provider: Provider::Local,
            source_class: SourceClass::ProviderCache,
            retrieved_at_unix_seconds: 0,
            detail: if field == MetadataField::Platform {
                "verified/local platform projection"
            } else {
                "local catalogue"
            }
            .into(),
            cache_path: None,
        },
    })
    .collect()
}
pub(super) fn provider_status(esde: bool, romm: bool, screenscraper: bool) -> Vec<ProviderStatus> {
    [
        (Provider::Local, true, "local catalogue"),
        (Provider::EsDe, esde, "ES-DE cache"),
        (Provider::Romm, romm, "RomM cache"),
        (
            Provider::ScreenScraper,
            screenscraper,
            "accepted ScreenScraper cache",
        ),
        (Provider::Bundled, true, "bundled fallback"),
    ]
    .into_iter()
    .map(|(provider, active, detail)| ProviderStatus {
        provider,
        active,
        detail: detail.into(),
    })
    .collect()
}
impl MediaIndex {
    /// The same neutral projection for an indexed game, including implicit
    /// local-only rows. No provider query, file read or identity matching here.
    pub fn resolved_for<'a>(
        &'a self,
        game: &super::super::library::Game,
    ) -> Cow<'a, metadata_aggregation::ResolvedMetadata> {
        if let Some(resolved) = self.resolved.get(&game.archive.id) {
            return Cow::Borrowed(resolved);
        }
        Cow::Owned(metadata_aggregation::resolve(AggregationInput {
            metadata: base_metadata(game),
            artwork: vec![],
            provider_status: provider_status(
                self.providers
                    .as_ref()
                    .is_some_and(|p| !p.esde_collections.is_empty()),
                false,
                false,
            ),
        }))
    }
    pub fn diagnostic_for(&self, game: &super::super::library::Game) -> Option<String> {
        if let Some(text) = self.diagnostics.get(&game.archive.id) {
            return Some(text.clone());
        }
        self.providers.as_ref()?;
        let sources = format!("{}{}", self.fallback_sources, self.warnings.join("\n"));
        Some(screenshot_diagnostic(
            &game.archive.absolute_path,
            0,
            None,
            &sources,
            !self.warnings.is_empty(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_only_projection_preserves_fields_provenance_status_and_incomplete_diagnostic() {
        let game = super::super::super::tests::archive(1, "Synthetic local title", Some("SNES"));
        let game = super::super::super::library::Game::from_archive(game);
        let mut index = MediaIndex::default();
        index.providers = Some(Arc::new(ProviderData {
            esde_collections: vec![],
            esde_root_count: 0,
            records: HashMap::new(),
            wrapper_matches: HashMap::new(),
            ambiguous: HashSet::new(),
        }));
        index.fallback_sources="ES-DE: not available at the configured/default location. LaunchBox: not available at the configured/default location.".into();
        index.warnings.push("A source could not be checked.".into());
        let result = index.resolved_for(&game);
        assert_eq!(
            result.fields[&MetadataField::Title].value,
            "Synthetic local title"
        );
        assert_eq!(result.fields[&MetadataField::Platform].value, "SNES");
        assert_eq!(
            result.fields[&MetadataField::Title].provenance.provider,
            Provider::Local
        );
        assert_eq!(
            result.fields[&MetadataField::Title].provenance.detail,
            "local catalogue"
        );
        let original = metadata_aggregation::resolve(AggregationInput {
            metadata: vec![
                MetadataCandidate {
                    field: MetadataField::Title,
                    value: "Synthetic local title".into(),
                    provenance: Provenance {
                        provider: Provider::Local,
                        source_class: SourceClass::ProviderCache,
                        retrieved_at_unix_seconds: 0,
                        detail: "local catalogue".into(),
                        cache_path: None,
                    },
                },
                MetadataCandidate {
                    field: MetadataField::Platform,
                    value: "SNES".into(),
                    provenance: Provenance {
                        provider: Provider::Local,
                        source_class: SourceClass::ProviderCache,
                        retrieved_at_unix_seconds: 0,
                        detail: "verified/local platform projection".into(),
                        cache_path: None,
                    },
                },
            ],
            provider_status: vec![
                ProviderStatus {
                    provider: Provider::Local,
                    active: true,
                    detail: "local catalogue".into(),
                },
                ProviderStatus {
                    provider: Provider::EsDe,
                    active: false,
                    detail: "ES-DE cache".into(),
                },
                ProviderStatus {
                    provider: Provider::Romm,
                    active: false,
                    detail: "RomM cache".into(),
                },
                ProviderStatus {
                    provider: Provider::ScreenScraper,
                    active: false,
                    detail: "accepted ScreenScraper cache".into(),
                },
                ProviderStatus {
                    provider: Provider::Bundled,
                    active: true,
                    detail: "bundled fallback".into(),
                },
            ],
            ..Default::default()
        });
        assert_eq!(
            *result, original,
            "every neutral field/provenance/fallback/status is preserved"
        );
        assert!(result.artwork.is_empty());
        assert!(result.conflicts.is_empty());
        assert_eq!(
            result
                .provider_status
                .iter()
                .map(|p| (p.provider, p.active))
                .collect::<Vec<_>>(),
            vec![
                (Provider::Local, true),
                (Provider::EsDe, false),
                (Provider::Romm, false),
                (Provider::ScreenScraper, false),
                (Provider::Bundled, true)
            ]
        );
        let diagnostic = index.diagnostic_for(&game).unwrap();
        assert!(diagnostic.contains("incomplete"));
        assert!(diagnostic.contains("No unambiguous exact-path RomM record"));
        assert!(!diagnostic.contains("Screenshot candidates: 0."));
        assert!(index.resolved.is_empty());
        assert!(
            index.diagnostics.is_empty(),
            "selected queries do not grow per-library caches"
        );
        let saved = metadata_aggregation::resolve(AggregationInput {
            metadata: vec![MetadataCandidate {
                field: MetadataField::Description,
                value: "Provider's exact result".into(),
                provenance: Provenance {
                    provider: Provider::Romm,
                    source_class: SourceClass::ProviderCache,
                    retrieved_at_unix_seconds: 7,
                    detail: "existing snapshot".into(),
                    cache_path: None,
                },
            }],
            ..Default::default()
        });
        index.resolved.insert(1, saved.clone());
        index
            .diagnostics
            .insert(1, "Existing exact-path diagnostic".into());
        assert_eq!(*index.resolved_for(&game), saved);
        assert_eq!(
            index.diagnostic_for(&game).unwrap(),
            "Existing exact-path diagnostic"
        );
    }
}
