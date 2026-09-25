//! Read-only discovery over existing providers. No title/filename matching.
use super::library::Library;
use archivefs_core::identity_source::{
    model::{ExternalIdentityRecord, IdentityProvider},
    settings::{ProviderSettings, SettingsLocation, default_identity_root},
    status::IdentitySourceApi,
};
use archivefs_core::metadata_aggregation::{
    self, AggregationInput, ArtworkCandidate, AssetKind, MetadataCandidate, MetadataField,
    Provenance, Provider, ProviderStatus, SourceClass,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

pub(super) fn screenshot_diagnostic(
    path: &std::path::Path,
    local: usize,
    romm: Option<(&str, usize)>,
    sources: &str,
    incomplete: bool,
) -> String {
    let mut message = format!(
        "Identity key: exact original archive path `{}`. {sources}\nLocal screenshot candidates: {local}. ",
        path.display()
    );
    let remote = if let Some((id, count)) = romm {
        message.push_str(&format!("RomM provider record `{id}`. "));
        if count == 0 {
            message.push_str("RomM matched this game, but that record has no screenshots. ");
        } else {
            message.push_str(&format!("RomM screenshot candidates: {count}. "));
        }
        count
    } else {
        message.push_str("No unambiguous exact-path RomM record matched. ");
        0
    };
    if incomplete {
        message.push_str("Some sources could not be checked; the screenshot search is incomplete.");
    } else {
        message.push_str(&format!("Screenshot candidates: {}.", local + remote));
    }
    message.push_str(" Filename/title guessing is not used.");
    message
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Kind {
    Cover,
    Screenshot(usize),
}

#[derive(Clone)]
pub(super) enum Source {
    Local(PathBuf),
    Remote {
        record: Arc<ExternalIdentityRecord>,
        kind: Kind,
    },
}

#[derive(Default)]
pub(super) struct MediaIndex {
    pub covers: HashMap<i64, Source>,
    pub screenshots: HashMap<i64, Vec<Source>>,
    pub identity_root: PathBuf,
    pub server: String,
    pub settings: ProviderSettings,
    pub trusted_roots: Vec<PathBuf>,
    pub elapsed_ms: u128,
    pub warnings: Vec<String>,
    pub descriptions: HashMap<i64, String>,
    /// The provider-neutral result used by every GUI-v2 presentation surface.
    /// `covers`/`screenshots` below are delivery schedules derived from this
    /// result, not an independent precedence table.
    pub resolved: HashMap<i64, metadata_aggregation::ResolvedMetadata>,
    pub diagnostics: HashMap<i64, String>,
}

impl MediaIndex {
    pub fn source(&self, game: i64, kind: Kind) -> Option<&Source> {
        match kind {
            Kind::Cover => self.covers.get(&game),
            Kind::Screenshot(index) => self.screenshots.get(&game)?.get(index),
        }
    }
    pub fn discover(library: &Library) -> Self {
        let start = Instant::now();
        let mut index = Self::default();
        log::debug!(
            "gui_v2 artwork indexing started: {} library games",
            library.games.len()
        );
        let config = archivefs_core::Config::load_default().ok();
        index.trusted_roots = config
            .as_ref()
            .map(|config| config.source_folders.clone())
            .unwrap_or_default();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let esde = home.as_ref().map(|home| home.join("ES-DE")).filter(|root| root.is_dir()).map(|root|
            archivefs_core::emulator_environment::es_de_metadata::discover_provider_snapshot_with_rom_root(&root, 1, config.as_ref().and_then(|config| config.master_rom_root.as_deref())));
        if config.is_none() {
            index
                .warnings
                .push("Game-folder configuration could not be read.".into());
        }
        if let Some(provider) = &esde {
            index.warnings.extend(provider.warnings.clone());
        }
        // Same environment/default discovery roots as the legacy local provider.
        let launchbox_root = std::env::var_os("LAUNCHBOX_ROOT")
            .map(PathBuf::from)
            .or_else(|| {
                home.as_ref().map(|home| {
                    home.join(".wine/drive_c/users")
                        .join(std::env::var_os("USER").unwrap_or_else(|| "user".into()))
                        .join("LaunchBox")
                })
            });
        let launchbox = launchbox_root
            .filter(|root| root.is_dir())
            .and_then(|root| {
                match archivefs_core::identity_source::launchbox_local::discover_launchbox_local(
                    &root, 1,
                ) {
                    Ok(snapshot) => Some(snapshot),
                    Err(error) => {
                        index.warnings.push(error);
                        None
                    }
                }
            });
        let mut records = HashMap::new();
        let mut ambiguous = HashSet::new();
        if let Ok(root) = default_identity_root() {
            index.identity_root = root.clone();
            index.settings = SettingsLocation::new(&root, IdentityProvider::Romm)
                .load()
                .unwrap_or_default();
            match IdentitySourceApi::new(&root, IdentityProvider::Romm).open_cache(None) {
                Ok(cache) => {
                    index.server = cache.server_id.clone();
                    for record in cache.records {
                        if let Some(path) = record.archivefs_path.clone()
                            && records.insert(path.clone(), Arc::new(record)).is_some()
                        {
                            ambiguous.insert(path);
                        }
                    }
                    for path in &ambiguous {
                        records.remove(path);
                    }
                }
                Err(error) => index
                    .warnings
                    .push(format!("RomM cache could not be checked: {error:?}")),
            }
        } else {
            index
                .warnings
                .push("RomM cache location could not be resolved.".into());
        }
        log::debug!(
            "gui_v2 artwork indexing provider snapshots ready: {} cached identity records",
            records.len()
        );
        for game in &library.games {
            let id = game.archive.id;
            let path = &game.archive.absolute_path;
            let mut metadata = Vec::new();
            let mut artwork_candidates = Vec::<(ArtworkCandidate, Source)>::new();
            let add_metadata = |metadata: &mut Vec<MetadataCandidate>,
                                field,
                                value: Option<String>,
                                provider,
                                detail: &str| {
                if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                    metadata.push(MetadataCandidate {
                        field,
                        value,
                        provenance: Provenance {
                            provider,
                            source_class: SourceClass::ProviderCache,
                            retrieved_at_unix_seconds: 0,
                            detail: detail.into(),
                            cache_path: None,
                        },
                    });
                }
            };
            add_metadata(
                &mut metadata,
                MetadataField::Title,
                Some(game.title.clone()),
                Provider::Local,
                "local catalogue",
            );
            add_metadata(
                &mut metadata,
                MetadataField::Platform,
                Some(game.platform.clone()),
                Provider::Local,
                "verified/local platform projection",
            );
            let mut sources = format!(
                "ES-DE: {}. LaunchBox: {}.",
                if esde.is_some() {
                    "local index checked"
                } else {
                    "not available at the configured/default location"
                },
                if launchbox.is_some() {
                    "local index checked"
                } else {
                    "not available at the configured/default location"
                }
            );
            // Prefer already-local pictures; a remote server must never hold them up.
            if let Some(entry) = esde
                .as_ref()
                .and_then(|provider| provider.lookup_path(&game.platform, path))
            {
                sources.push_str(" ES-DE matched this game.");
                if let Some(path) = entry.entry.media.cover {
                    artwork_candidates.push((
                        ArtworkCandidate {
                            kind: AssetKind::CoverFront,
                            path: path.clone(),
                            cached: true,
                            provenance: Provenance {
                                provider: Provider::EsDe,
                                source_class: SourceClass::ProviderCache,
                                retrieved_at_unix_seconds: 0,
                                detail: entry.entry.provenance.clone(),
                                cache_path: Some(path.clone()),
                            },
                        },
                        Source::Local(path),
                    ));
                }
                if let Some(path) = entry.entry.media.screenshot {
                    artwork_candidates.push((
                        ArtworkCandidate {
                            kind: AssetKind::Screenshot,
                            path: path.clone(),
                            cached: true,
                            provenance: Provenance {
                                provider: Provider::EsDe,
                                source_class: SourceClass::ProviderCache,
                                retrieved_at_unix_seconds: 0,
                                detail: entry.entry.provenance.clone(),
                                cache_path: Some(path.clone()),
                            },
                        },
                        Source::Local(path),
                    ));
                }
                add_metadata(
                    &mut metadata,
                    MetadataField::Title,
                    entry.entry.name.clone(),
                    Provider::EsDe,
                    "ES-DE gamelist cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Description,
                    entry.entry.description.clone(),
                    Provider::EsDe,
                    "ES-DE gamelist cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::ReleaseDate,
                    entry.entry.release_date.clone(),
                    Provider::EsDe,
                    "ES-DE gamelist cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Developer,
                    entry.entry.developer.clone(),
                    Provider::EsDe,
                    "ES-DE gamelist cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Publisher,
                    entry.entry.publisher.clone(),
                    Provider::EsDe,
                    "ES-DE gamelist cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Genre,
                    entry.entry.genre.clone(),
                    Provider::EsDe,
                    "ES-DE gamelist cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Players,
                    entry.entry.players.clone(),
                    Provider::EsDe,
                    "ES-DE gamelist cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Rating,
                    entry.entry.rating.clone(),
                    Provider::EsDe,
                    "ES-DE gamelist cache",
                );
            }
            if let Some(provider) = &launchbox
                && let Some(found) = provider.lookup(None, Some(path), Some(&game.platform), None)
            {
                sources.push_str(" LaunchBox matched this game.");
                let snapshot = provider.media_snapshot(&found);
                if let Some(path) = snapshot
                    .cover
                    .and_then(|reference| reference.hosted_reference)
                {
                    let path: PathBuf = path.into();
                    artwork_candidates.push((
                        ArtworkCandidate {
                            kind: AssetKind::CoverFront,
                            path: path.clone(),
                            cached: true,
                            provenance: Provenance {
                                provider: Provider::Local,
                                source_class: SourceClass::LocalEvidence,
                                retrieved_at_unix_seconds: 0,
                                detail: "local provider artwork".into(),
                                cache_path: Some(path.clone()),
                            },
                        },
                        Source::Local(path),
                    ));
                }
                for reference in snapshot.screenshots.into_iter().take(8) {
                    if let Some(path) = reference.hosted_reference {
                        let path: PathBuf = path.into();
                        artwork_candidates.push((
                            ArtworkCandidate {
                                kind: AssetKind::Screenshot,
                                path: path.clone(),
                                cached: true,
                                provenance: Provenance {
                                    provider: Provider::Local,
                                    source_class: SourceClass::LocalEvidence,
                                    retrieved_at_unix_seconds: 0,
                                    detail: "local provider artwork".into(),
                                    cache_path: Some(path.clone()),
                                },
                            },
                            Source::Local(path),
                        ));
                    }
                }
            }
            if let Some(record) = records.get(path) {
                add_metadata(
                    &mut metadata,
                    MetadataField::Description,
                    record.synopsis.clone(),
                    Provider::Romm,
                    "RomM identity cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Genre,
                    (!record.genres.is_empty()).then(|| record.genres.join(" · ")),
                    Provider::Romm,
                    "RomM identity cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Players,
                    record.players.clone(),
                    Provider::Romm,
                    "RomM identity cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Rating,
                    record.rating.map(|value| value.to_string()),
                    Provider::Romm,
                    "RomM identity cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::ReleaseDate,
                    record.release_year.map(|value| value.to_string()),
                    Provider::Romm,
                    "RomM identity cache",
                );
            }
            let local_count = index.screenshots.get(&id).map_or(0, Vec::len);
            if let Some(record) = records.get(path)
                && let Some(artwork) = &record.artwork
            {
                artwork_candidates.push((
                    ArtworkCandidate {
                        kind: AssetKind::CoverFront,
                        path: PathBuf::from(format!("romm://{}/cover", record.provider_game_id)),
                        cached: false,
                        provenance: Provenance {
                            provider: Provider::Romm,
                            source_class: SourceClass::ProviderCache,
                            retrieved_at_unix_seconds: record.imported_at_unix_seconds.max(0)
                                as u64,
                            detail: "RomM identity cache".into(),
                            cache_path: None,
                        },
                    },
                    Source::Remote {
                        record: record.clone(),
                        kind: Kind::Cover,
                    },
                ));
                for ordinal in 0..record
                    .artwork
                    .as_ref()
                    .map_or(0, |art| art.screenshots.len())
                    .min(8)
                {
                    artwork_candidates.push((
                        ArtworkCandidate {
                            kind: AssetKind::Screenshot,
                            path: PathBuf::from(format!(
                                "romm://{}/screenshot/{ordinal}",
                                record.provider_game_id
                            )),
                            cached: false,
                            provenance: Provenance {
                                provider: Provider::Romm,
                                source_class: SourceClass::ProviderCache,
                                retrieved_at_unix_seconds: record.imported_at_unix_seconds.max(0)
                                    as u64,
                                detail: "RomM identity cache".into(),
                                cache_path: None,
                            },
                        },
                        Source::Remote {
                            record: record.clone(),
                            kind: Kind::Screenshot(ordinal),
                        },
                    ));
                }
            }
            if let Some(saved) = &game.screenscraper {
                let values = &saved.values;
                add_metadata(
                    &mut metadata,
                    MetadataField::Title,
                    values.title.clone(),
                    Provider::ScreenScraper,
                    "accepted ScreenScraper cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Description,
                    values.synopsis.clone(),
                    Provider::ScreenScraper,
                    "accepted ScreenScraper cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Developer,
                    values.developer.clone(),
                    Provider::ScreenScraper,
                    "accepted ScreenScraper cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Publisher,
                    values.publisher.clone(),
                    Provider::ScreenScraper,
                    "accepted ScreenScraper cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Genre,
                    values.genre.clone(),
                    Provider::ScreenScraper,
                    "accepted ScreenScraper cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::Players,
                    values.players.clone(),
                    Provider::ScreenScraper,
                    "accepted ScreenScraper cache",
                );
                add_metadata(
                    &mut metadata,
                    MetadataField::ReleaseDate,
                    values.release_year.map(|value| value.to_string()),
                    Provider::ScreenScraper,
                    "accepted ScreenScraper cache",
                );
            }
            let resolved = metadata_aggregation::resolve(AggregationInput {
                metadata,
                artwork: artwork_candidates
                    .iter()
                    .map(|(candidate, _)| candidate.clone())
                    .collect(),
                provider_status: vec![
                    ProviderStatus {
                        provider: Provider::Local,
                        active: true,
                        detail: "local catalogue".into(),
                    },
                    ProviderStatus {
                        provider: Provider::EsDe,
                        active: esde.is_some(),
                        detail: "ES-DE cache".into(),
                    },
                    ProviderStatus {
                        provider: Provider::Romm,
                        active: records.contains_key(path),
                        detail: "RomM cache".into(),
                    },
                    ProviderStatus {
                        provider: Provider::ScreenScraper,
                        active: game.screenscraper.is_some(),
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
            if let Some(candidate) = resolved.artwork.get(&AssetKind::CoverFront) {
                if let Some((_, source)) = artwork_candidates
                    .iter()
                    .find(|(item, _)| item.kind == candidate.kind && item.path == candidate.path)
                {
                    index.covers.insert(id, source.clone());
                }
            }
            for (ordinal, candidate) in resolved
                .artwork
                .values()
                .filter(|candidate| candidate.kind == AssetKind::Screenshot)
                .enumerate()
            {
                if let Some((_, source)) = artwork_candidates
                    .iter()
                    .find(|(item, _)| item.kind == candidate.kind && item.path == candidate.path)
                {
                    index
                        .screenshots
                        .entry(id)
                        .or_default()
                        .insert(ordinal, source.clone());
                }
            }
            if let Some(description) = resolved.fields.get(&MetadataField::Description) {
                index.descriptions.insert(id, description.value.clone());
            }
            index.resolved.insert(id, resolved);
            if ambiguous.contains(path) {
                sources.push_str(" Competing RomM records were refused.");
            }
            sources.push_str(&index.warnings.join("\n"));
            index.diagnostics.insert(
                id,
                screenshot_diagnostic(
                    path,
                    local_count,
                    records.get(path).map(|record| {
                        (
                            record.provider_game_id.as_str(),
                            record
                                .artwork
                                .as_ref()
                                .map_or(0, |artwork| artwork.screenshots.len()),
                        )
                    }),
                    &sources,
                    !index.warnings.is_empty() || ambiguous.contains(path),
                ),
            );
        }
        index.elapsed_ms = start.elapsed().as_millis();
        log::debug!(
            "gui_v2 artwork indexing: {} covers, {} ms",
            index.covers.len(),
            index.elapsed_ms
        );
        index
    }
}
