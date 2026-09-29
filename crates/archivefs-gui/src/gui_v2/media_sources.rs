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

/// Developer-facing counters answering "which provider had what, and did it
/// line up with the catalogue?". Never shown on the normal screens.
#[derive(Default, Clone, Debug)]
pub(super) struct ProviderStats {
    pub esde_entries: usize,
    pub esde_cover_refs: usize,
    pub esde_cover_files_present: usize,
    pub esde_matched_games: usize,
    pub esde_matched_missing_cover_file: usize,
    /// (system, platform, path, same-file-name catalogue rows)
    pub esde_unmatched_samples: Vec<String>,
    pub esde_unmatched_total: usize,
    pub esde_unmatched_near_miss: usize,
    pub romm_records: usize,
    pub romm_with_path: usize,
    pub romm_matched_games: usize,
    pub romm_unmatched_samples: Vec<String>,
    pub romm_unmatched_total: usize,
    pub romm_unmatched_near_miss: usize,
    pub launchbox_matched_games: usize,
    /// Catalogue rows associated with a RomM record only through the
    /// game-folder wrapper rule (no exact path record).
    pub romm_wrapper_matches: usize,
    /// Whether the RomM source (and so remote picture fetches) is enabled.
    pub romm_source_enabled: bool,
    /// RomM records per provider platform: (records, matched a catalogue row,
    /// has artwork).
    pub romm_by_platform: std::collections::BTreeMap<String, (usize, usize, usize)>,
    /// Unmatched RomM records per platform with a same-name catalogue row:
    /// (near misses, exactly one same-platform row, of those size equal,
    /// with artwork).
    pub romm_near_miss_by_platform:
        std::collections::BTreeMap<String, (usize, usize, usize, usize)>,
    pub esde_near_miss_by_platform: std::collections::BTreeMap<String, usize>,
}

/// Provider data kept so "why does this game have no cover?" can be answered
/// for one game on demand, instead of building text for every game up front.
pub(super) struct ProviderData {
    esde_collections:
        Vec<archivefs_core::emulator_environment::es_de_metadata::EsDeProviderCollection>,
    esde_root_count: usize,
    records: HashMap<PathBuf, Arc<ExternalIdentityRecord>>,
    wrapper_matches: HashMap<PathBuf, PathBuf>,
    ambiguous: HashSet<PathBuf>,
}

#[derive(Default)]
pub(super) struct MediaIndex {
    pub providers: Option<Arc<ProviderData>>,
    pub stats: ProviderStats,
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
    /// Structured answer to "why does this game have (no) cover?": what each
    /// provider had, why a candidate was or was not usable, and what was chosen.
    /// Developer/Advanced diagnostics only; computed for one game on demand.
    pub fn explain_cover(&self, game: &super::library::Game) -> String {
        let Some(data) = &self.providers else {
            return "Providers have not been indexed yet.".into();
        };
        let path = &game.archive.absolute_path;
        let mut notes: Vec<String> = Vec::new();
        if data.esde_collections.is_empty() {
            notes.push("ES-DE: not available at the default location".into());
        } else if let Some(entry) = data
            .esde_collections
            .iter()
            .find_map(|provider| provider.lookup_path(&game.platform, path))
        {
            let usable = entry
                .media
                .cover
                .is_some_and(|media| media.exists && media.readable);
            notes.push(match (&entry.entry.media.cover, usable) {
                (Some(_), true) => format!(
                    "ES-DE: matched exact platform+path; cover file present ({})",
                    entry.entry.provenance
                ),
                (Some(missing), false) => format!(
                    "ES-DE: matched, but the referenced cover file is missing or unreadable ({}); not used",
                    missing.display()
                ),
                (None, _) => "ES-DE: matched, but it has no cover reference and no downloaded_media file named after the ROM".into(),
            });
        } else {
            notes.push(format!(
                "ES-DE: no gamelist entry for platform {:?} and this exact path under {} candidate ROM root(s)",
                game.platform, data.esde_root_count
            ));
        }
        if data.ambiguous.contains(path) {
            notes.push("RomM: competing records for this exact path were refused".into());
        } else if let Some(record) = data.records.get(path) {
            notes.push(format!(
                "RomM: record {} matched by exact path; {}",
                record.provider_game_id,
                if record.artwork.is_some() {
                    "it has artwork (fetched on demand)"
                } else {
                    "the record has no artwork"
                }
            ));
        } else if let Some(record) = data
            .wrapper_matches
            .get(path)
            .and_then(|key| data.records.get(key))
        {
            notes.push(format!(
                "RomM: record {} matched through the game-folder wrapper rule; {}",
                record.provider_game_id,
                if record.artwork.is_some() {
                    "it has artwork (fetched on demand)"
                } else {
                    "the record has no artwork"
                }
            ));
        } else {
            notes.push("RomM: no cached record for this exact path and no unique game-folder wrapper match".into());
        }
        notes.push(
            self.resolved
                .get(&game.archive.id)
                .and_then(|resolved| resolved.artwork.get(&AssetKind::CoverFront))
                .map(|candidate| {
                    format!(
                        "Result: selected {} ({})",
                        candidate.provenance.provider.label(),
                        candidate.path.display()
                    )
                })
                .unwrap_or_else(|| "Result: no cover candidate from any provider".into()),
        );
        notes.join(" | ")
    }

    /// RomM covers chosen for games: (already in the local artwork cache, needing
    /// a network fetch). Reads the cache index; used by the developer report only.
    fn romm_cover_cache_split(&self) -> (usize, usize) {
        if self.server.is_empty() {
            return (0, 0);
        }
        let cache = archivefs_core::identity_source::artwork::ArtworkCache::new(
            &self.identity_root,
            IdentityProvider::Romm,
        );
        let (mut cached, mut need_network) = (0, 0);
        for source in self.covers.values() {
            if let Source::Remote { record, .. } = source {
                let request =
                    archivefs_core::identity_source::artwork::ArtworkRequest::from_record(record);
                if cache.lookup(&self.server, &request).is_some() {
                    cached += 1;
                } else {
                    need_network += 1;
                }
            }
        }
        (cached, need_network)
    }

    /// Developer report: covers by winning provider and per platform, plus the
    /// provider-side match statistics. Used by `--diagnose-artwork`.
    pub fn diagnostic_report(&self, library: &Library) -> String {
        use std::collections::BTreeMap;
        let mut by_platform: BTreeMap<&str, [usize; 4]> = BTreeMap::new(); // games, covers, screenshots, metadata
        let mut winners: BTreeMap<String, usize> = BTreeMap::new();
        for game in &library.games {
            let row = by_platform.entry(game.platform.as_str()).or_default();
            row[0] += 1;
            if self.covers.contains_key(&game.archive.id) {
                row[1] += 1;
            }
            if self.screenshots.contains_key(&game.archive.id) {
                row[2] += 1;
            }
            if self.descriptions.contains_key(&game.archive.id) {
                row[3] += 1;
            }
            if let Some(cover) = self
                .resolved
                .get(&game.archive.id)
                .and_then(|resolved| resolved.artwork.get(&AssetKind::CoverFront))
            {
                *winners
                    .entry(cover.provenance.provider.label().to_string())
                    .or_default() += 1;
            }
        }
        let (cached, need_network) = self.romm_cover_cache_split();
        let mut out = String::new();
        out.push_str(&format!(
            "games={} covers={} screenshot_groups={} descriptions={}\n",
            library.games.len(),
            self.covers.len(),
            self.screenshots.len(),
            self.descriptions.len()
        ));
        out.push_str(&format!("cover winners by provider: {winners:?}\n"));
        out.push_str("platform | games | covers | screenshots | metadata\n");
        for (platform, row) in &by_platform {
            out.push_str(&format!(
                "{platform} | {} | {} | {} | {}\n",
                row[0], row[1], row[2], row[3]
            ));
        }
        out.push_str(&format!(
            "RomM covers already in the local artwork cache: {cached}; needing a network fetch: {need_network}; RomM source enabled: {}\n",
            self.stats.romm_source_enabled
        ));
        out.push_str(&format!("{:#?}\n", self.stats));
        out
    }

    pub fn source(&self, game: i64, kind: Kind) -> Option<&Source> {
        match kind {
            Kind::Cover => self.covers.get(&game),
            Kind::Screenshot(index) => self.screenshots.get(&game)?.get(index),
        }
    }
    pub fn discover(library: &Library) -> Self {
        Self::discover_impl(library, false)
    }

    /// Same result as [`Self::discover`], plus the provider-side near-miss
    /// statistics used by the developer report (extra work, never on the GUI's
    /// normal indexing path).
    pub fn discover_with_stats(library: &Library) -> Self {
        Self::discover_impl(library, true)
    }

    fn discover_impl(library: &Library, collect_stats: bool) -> Self {
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
        // ES-DE gamelist paths are relative to ES-DE's own ROM directory, so that
        // is the root that makes them comparable with catalogue paths. The
        // EmuWiz `master_rom_root` stays as a second candidate (it is what
        // EmuWiz's own ES-DE export is relative to). A gamelist entry is only
        // ever matched by an exact platform + full-path key under one of them.
        let es_de_root = home
            .as_ref()
            .map(|home| home.join("ES-DE"))
            .filter(|root| root.is_dir());
        let mut esde_roots: Vec<Option<PathBuf>> = Vec::new();
        if let Some(root) = &es_de_root {
            for candidate in [
                archivefs_core::emulator_environment::es_de_metadata::configured_rom_directory(
                    root,
                ),
                config
                    .as_ref()
                    .and_then(|config| config.master_rom_root.clone()),
            ] {
                if candidate.is_some() && !esde_roots.contains(&candidate) {
                    esde_roots.push(candidate);
                }
            }
            if esde_roots.is_empty() {
                esde_roots.push(None);
            }
        }
        let esde_collections: Vec<_> = es_de_root
            .iter()
            .flat_map(|root| {
                esde_roots.iter().map(move |rom_root| {
                    archivefs_core::emulator_environment::es_de_metadata::discover_provider_snapshot_with_rom_root(
                        root, 1, rom_root.as_deref(),
                    )
                })
            })
            .collect();
        let esde_present = !esde_collections.is_empty();
        // ES-DE indexes are per system, and a lookup key starts with the
        // platform, so a game only needs the indexes of its own platform.
        let mut esde_by_platform: HashMap<
            &str,
            Vec<&archivefs_core::emulator_environment::es_de_metadata::EsDeProviderIndex>,
        > = HashMap::new();
        for provider in &esde_collections {
            for provider_index in &provider.indexes {
                if let Some(platform) = provider_index
                    .entries
                    .iter()
                    .find_map(|entry| entry.canonical_platform.as_deref())
                {
                    esde_by_platform
                        .entry(platform)
                        .or_default()
                        .push(provider_index);
                }
            }
        }
        if config.is_none() {
            index
                .warnings
                .push("Game-folder configuration could not be read.".into());
        }
        if let Some(provider) = esde_collections.first() {
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
        // Diagnostics only: which catalogue rows share a file name, so an
        // unmatched provider entry can be classed as a near miss (same file,
        // different path/platform) versus genuinely absent from the catalogue.
        let mut rows_by_file_name: HashMap<std::ffi::OsString, Vec<(String, PathBuf)>> =
            HashMap::new();
        let mut size_by_path: HashMap<PathBuf, Option<u64>> = HashMap::new();
        for game in library.games.iter().filter(|_| collect_stats) {
            if let Some(name) = game.archive.absolute_path.file_name() {
                rows_by_file_name
                    .entry(name.to_os_string())
                    .or_default()
                    .push((game.platform.clone(), game.archive.absolute_path.clone()));
                size_by_path.insert(
                    game.archive.absolute_path.clone(),
                    game.archive.size_bytes.map(|size| size as u64),
                );
            }
        }
        let mut esde_matched_keys = HashSet::<String>::new();
        let mut esde_matched_entries = HashSet::<String>::new();
        let mut romm_matched_paths = HashSet::<PathBuf>::new();
        index.stats.romm_records = records.len();
        index.stats.romm_with_path = records.len();
        let wrapper_matches = wrapper_record_matches(library, &records);
        index.stats.romm_wrapper_matches = wrapper_matches.len();
        for game in &library.games {
            let id = game.archive.id;
            let path = &game.archive.absolute_path;
            // Exact archive-path record first; otherwise the strictly
            // path-anchored game-folder wrapper rule (see the function).
            let matched_record_path = if records.contains_key(path) {
                Some(path)
            } else {
                wrapper_matches.get(path)
            };
            let matched_record = matched_record_path.and_then(|key| records.get(key));
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
                if esde_present {
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
            if let Some(entry) = esde_by_platform
                .get(game.platform.as_str())
                .and_then(|indexes| {
                    indexes
                        .iter()
                        .find_map(|provider| provider.lookup_path(&game.platform, path))
                })
            {
                sources.push_str(" ES-DE matched this game.");
                index.stats.esde_matched_games += 1;
                esde_matched_entries.insert(entry.entry.provenance.clone());
                esde_matched_keys.insert(
                    archivefs_core::emulator_environment::es_de_metadata::provider_path_key(
                        &game.platform,
                        path,
                    ),
                );
                if entry.entry.media.cover.is_some()
                    && !entry.media.cover.is_some_and(|media| media.exists)
                {
                    index.stats.esde_matched_missing_cover_file += 1;
                }
                artwork_candidates.extend(esde_artwork_candidates(&entry));
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
                index.stats.launchbox_matched_games += 1;
                let snapshot = provider.media_snapshot(&found);
                if let Some(path) = snapshot
                    .cover
                    .and_then(|reference| reference.hosted_reference)
                    .map(PathBuf::from)
                    .filter(|path| path.is_file())
                {
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
                    if let Some(path) = reference
                        .hosted_reference
                        .map(PathBuf::from)
                        .filter(|path| path.is_file())
                    {
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
            if let Some(record) = matched_record {
                index.stats.romm_matched_games += 1;
                if let Some(key) = matched_record_path {
                    romm_matched_paths.insert(key.clone());
                }
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
            if let Some(record) = matched_record
                && record.artwork.is_some()
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
                        active: esde_present,
                        detail: "ES-DE cache".into(),
                    },
                    ProviderStatus {
                        provider: Provider::Romm,
                        active: matched_record.is_some(),
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
            index.diagnostics.insert(id, {
                screenshot_diagnostic(
                    path,
                    local_count,
                    matched_record.map(|record| {
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
                )
            });
        }
        let mut counted_entries = HashSet::<String>::new();
        for provider in esde_collections.iter().filter(|_| collect_stats) {
            for provider_index in &provider.indexes {
                for entry in &provider_index.entries {
                    // The same entry is indexed once per candidate root.
                    if !counted_entries.insert(entry.provenance.clone()) {
                        continue;
                    }
                    index.stats.esde_entries += 1;
                    if let Some(cover) = &entry.media.cover {
                        index.stats.esde_cover_refs += 1;
                        if provider_index
                            .media
                            .get(cover)
                            .is_some_and(|media| media.exists)
                        {
                            index.stats.esde_cover_files_present += 1;
                        }
                    }
                    let Some(platform) = entry.canonical_platform.as_deref() else {
                        continue;
                    };
                    let source = entry.canonical_path.as_deref().unwrap_or(&entry.path);
                    let key =
                        archivefs_core::emulator_environment::es_de_metadata::provider_path_key(
                            platform, source,
                        );
                    if esde_matched_keys.contains(&key)
                        || esde_matched_entries.contains(&entry.provenance)
                    {
                        continue;
                    }
                    index.stats.esde_unmatched_total += 1;
                    let same_name = source
                        .file_name()
                        .and_then(|name| rows_by_file_name.get(name));
                    if same_name.is_some() {
                        index.stats.esde_unmatched_near_miss += 1;
                        *index
                            .stats
                            .esde_near_miss_by_platform
                            .entry(platform.to_string())
                            .or_default() += 1;
                    }
                    if index.stats.esde_unmatched_samples.len() < 12 {
                        index.stats.esde_unmatched_samples.push(format!(
                            "[{}] platform={platform} path={} cover_ref={} same-file-name catalogue rows: {:?}",
                            entry.system,
                            source.display(),
                            entry.media.cover.is_some(),
                            same_name.map(|rows| rows.iter().take(3).cloned().collect::<Vec<_>>())
                        ));
                    }
                }
            }
        }
        for (path, record) in records.iter().filter(|_| collect_stats) {
            let platform = record
                .platform_candidate
                .clone()
                .or_else(|| record.provider_platform_name.clone())
                .unwrap_or_else(|| "(none)".into());
            let entry = index.stats.romm_by_platform.entry(platform).or_default();
            entry.0 += 1;
            entry.1 += usize::from(romm_matched_paths.contains(path));
            entry.2 += usize::from(record.artwork.is_some());
            if romm_matched_paths.contains(path) {
                continue;
            }
            index.stats.romm_unmatched_total += 1;
            let same_name = path
                .file_name()
                .and_then(|name| rows_by_file_name.get(name));
            if let Some(rows) = same_name {
                index.stats.romm_unmatched_near_miss += 1;
                let same_platform: Vec<_> = rows
                    .iter()
                    .filter(|(row_platform, _)| {
                        record.platform_candidate.as_deref() == Some(row_platform.as_str())
                    })
                    .collect();
                let entry = index
                    .stats
                    .romm_near_miss_by_platform
                    .entry(
                        record
                            .platform_candidate
                            .clone()
                            .unwrap_or_else(|| "(none)".into()),
                    )
                    .or_default();
                entry.0 += 1;
                if same_platform.len() == 1 {
                    entry.1 += 1;
                    let size = size_by_path.get(&same_platform[0].1).copied().flatten();
                    if size.is_some() && size == record.file_size_bytes {
                        entry.2 += 1;
                    }
                }
                entry.3 += usize::from(record.artwork.is_some());
            }
            if index.stats.romm_unmatched_samples.len() < 12 {
                index.stats.romm_unmatched_samples.push(format!(
                    "romm id={} platform={:?} path={} same-file-name catalogue rows: {:?}",
                    record.provider_game_id,
                    record.platform_candidate,
                    path.display(),
                    same_name.map(|rows| rows.iter().take(3).cloned().collect::<Vec<_>>())
                ));
            }
        }
        index.stats.romm_source_enabled = index.settings.source.enabled;
        index.providers = Some(Arc::new(ProviderData {
            esde_root_count: esde_roots.len(),
            esde_collections,
            records,
            wrapper_matches,
            ambiguous,
        }));
        index.elapsed_ms = start.elapsed().as_millis();
        log::debug!(
            "gui_v2 artwork indexing: {} covers, {} ms",
            index.covers.len(),
            index.elapsed_ms
        );
        index
    }
}

/// Associates a catalogue row with a RomM record when RomM's path and the
/// catalogue path differ only by ONE wrapper folder: RomM knows
/// `<dir>/<file>` while the catalogue holds `<dir>/<game folder>/<file>` (a
/// multi-file/"set" layout). This is anchored on the record's own mapped path
/// (same directory, same file name) and the same platform, so it is not title
/// or file-name guessing across the library, and it grants only artwork and
/// display metadata; it never changes game identity.
///
/// It is refused whenever the association is not one-to-one: a record with
/// several possible wrapper folders, or a wrapper row that several records
/// (or a record that several rows) could claim.
pub(super) fn wrapper_record_matches(
    library: &Library,
    records: &HashMap<PathBuf, Arc<ExternalIdentityRecord>>,
) -> HashMap<PathBuf, PathBuf> {
    let mut by_dir_and_name: HashMap<(PathBuf, std::ffi::OsString), Vec<&PathBuf>> = HashMap::new();
    for record_path in records.keys() {
        if let (Some(dir), Some(name)) = (record_path.parent(), record_path.file_name()) {
            by_dir_and_name
                .entry((dir.to_path_buf(), name.to_os_string()))
                .or_default()
                .push(record_path);
        }
    }
    let mut claims: HashMap<&PathBuf, Vec<&PathBuf>> = HashMap::new();
    for game in &library.games {
        let path = &game.archive.absolute_path;
        if records.contains_key(path) {
            continue;
        }
        let (Some(grandparent), Some(name)) = (
            path.parent().and_then(|parent| parent.parent()),
            path.file_name(),
        ) else {
            continue;
        };
        let Some(bucket) = by_dir_and_name.get(&(grandparent.to_path_buf(), name.to_os_string()))
        else {
            continue;
        };
        if let [record_path] = bucket.as_slice() {
            let same_platform = records
                .get(*record_path)
                .and_then(|record| record.platform_candidate.as_deref())
                .is_some_and(|platform| platform == game.platform);
            if same_platform {
                claims.entry(*record_path).or_default().push(path);
            }
        }
    }
    // A record already matched exactly by a catalogue row is not re-used.
    let exact: HashSet<&PathBuf> = library
        .games
        .iter()
        .map(|game| &game.archive.absolute_path)
        .filter(|path| records.contains_key(*path))
        .collect();
    claims
        .into_iter()
        .filter(|(record_path, rows)| rows.len() == 1 && !exact.contains(*record_path))
        .map(|(record_path, rows)| (rows[0].clone(), record_path.clone()))
        .collect()
}

/// Artwork candidates for an ES-DE entry that was matched by exact platform and
/// path. A gamelist reference to a file that is not on disk (or unreadable) is
/// not a candidate at all: an unusable higher-priority reference must never win
/// a tie against a usable lower-priority one.
pub(super) fn esde_artwork_candidates(
    entry: &archivefs_core::emulator_environment::es_de_metadata::EsDeResolvedEntry,
) -> Vec<(ArtworkCandidate, Source)> {
    let usable = |availability: Option<
        archivefs_core::emulator_environment::es_de_metadata::EsDeMediaAvailability,
    >| availability.is_some_and(|media| media.exists && media.readable);
    let mut out = Vec::new();
    for (kind, reference, available) in [
        (
            AssetKind::CoverFront,
            &entry.entry.media.cover,
            usable(entry.media.cover),
        ),
        (
            AssetKind::Screenshot,
            &entry.entry.media.screenshot,
            usable(entry.media.screenshot),
        ),
    ] {
        if let (true, Some(path)) = (available, reference) {
            out.push((
                ArtworkCandidate {
                    kind,
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
                Source::Local(path.clone()),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::PersistedArchive;
    use archivefs_core::emulator_environment::es_de_metadata::parse_and_index_gamelist_with_roots;
    use std::path::Path;

    fn row(id: i64, platform: &str, path: &str) -> PersistedArchive {
        PersistedArchive {
            id,
            source_folder_id: 1,
            relative_path: path.into(),
            absolute_path: path.into(),
            archive_kind: "zip".into(),
            display_name: Path::new(path)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            normalized_name: String::new(),
            size_bytes: Some(1),
            modified_time_unix_seconds: Some(1),
            platform: Some(platform.into()),
            platform_source: Some("test".into()),
            last_known_health: "pending".into(),
            last_seen_at: "now".into(),
            last_verified_missing_at: None,
            identity_report: None,
        }
    }

    fn record(id: &str, path: &str, platform: &str, artwork: bool) -> Arc<ExternalIdentityRecord> {
        let mut value = serde_json::json!({
            "provider": "romm", "server_id": "fixture", "provider_game_id": id,
            "provider_path": "roms/x", "archivefs_path": path,
            "platform_candidate": platform,
            "regions": [], "hashes": [], "metadata_provider_ids": [], "related_files": [],
            "sibling_game_ids": [], "imported_at_unix_seconds": 0,
            "verification": "strong_external", "conflicts": [], "evidence": [],
        });
        if artwork {
            value["artwork"] =
                serde_json::json!({"reference": "/c.png", "small_reference": "/c.png"});
        }
        Arc::new(serde_json::from_value(value).unwrap())
    }

    fn library(rows: Vec<PersistedArchive>) -> Library {
        Library::new(rows)
    }

    fn records(
        items: Vec<Arc<ExternalIdentityRecord>>,
    ) -> HashMap<PathBuf, Arc<ExternalIdentityRecord>> {
        items
            .into_iter()
            .map(|record| (record.archivefs_path.clone().unwrap(), record))
            .collect()
    }

    // --- game-folder wrapper rule -------------------------------------

    #[test]
    fn wrapper_rule_links_a_set_folder_layout_to_its_record() {
        let library = library(vec![row(
            1,
            "Sharp X68000",
            "/g/x68/Set 3 (Alice Soft)/Set 3 [FD].zip",
        )]);
        let records = records(vec![record(
            "9",
            "/g/x68/Set 3 [FD].zip",
            "Sharp X68000",
            true,
        )]);
        let matches = wrapper_record_matches(&library, &records);
        assert_eq!(
            matches.get(Path::new("/g/x68/Set 3 (Alice Soft)/Set 3 [FD].zip")),
            Some(&PathBuf::from("/g/x68/Set 3 [FD].zip"))
        );
    }

    #[test]
    fn wrapper_rule_never_crosses_platforms_or_directories() {
        let records = records(vec![record("9", "/g/x68/Game.zip", "Sharp X68000", true)]);
        // Same title and file name, different platform.
        let other_platform = library(vec![row(1, "MegaDrive", "/g/x68/Wrap/Game.zip")]);
        assert!(wrapper_record_matches(&other_platform, &records).is_empty());
        // Same file name under an unrelated directory.
        let other_dir = library(vec![row(1, "Sharp X68000", "/elsewhere/Wrap/Game.zip")]);
        assert!(wrapper_record_matches(&other_dir, &records).is_empty());
        // Two levels of wrapper is not "one wrapper folder".
        let deep = library(vec![row(1, "Sharp X68000", "/g/x68/A/B/Game.zip")]);
        assert!(wrapper_record_matches(&deep, &records).is_empty());
        // A similar but different file name is never matched.
        let renamed = library(vec![row(1, "Sharp X68000", "/g/x68/Wrap/Game (Rev 1).zip")]);
        assert!(wrapper_record_matches(&renamed, &records).is_empty());
    }

    #[test]
    fn wrapper_rule_refuses_ambiguity_and_never_overrides_an_exact_match() {
        let records = records(vec![record("9", "/g/x68/Disk.zip", "Sharp X68000", true)]);
        // Two set folders both claim the one record: neither gets it.
        let ambiguous = library(vec![
            row(1, "Sharp X68000", "/g/x68/Game A/Disk.zip"),
            row(2, "Sharp X68000", "/g/x68/Game B/Disk.zip"),
        ]);
        assert!(wrapper_record_matches(&ambiguous, &records).is_empty());
        // A row that already matches the record exactly keeps it; a wrapper
        // row does not also take it.
        let exact_too = library(vec![
            row(1, "Sharp X68000", "/g/x68/Disk.zip"),
            row(2, "Sharp X68000", "/g/x68/Game A/Disk.zip"),
        ]);
        assert!(wrapper_record_matches(&exact_too, &records).is_empty());
    }

    #[test]
    fn provider_matching_never_creates_verified_identity() {
        let library = library(vec![row(1, "Sharp X68000", "/g/x68/Wrap/Game.zip")]);
        let records = records(vec![record("9", "/g/x68/Game.zip", "Sharp X68000", true)]);
        let matches = wrapper_record_matches(&library, &records);
        assert_eq!(matches.len(), 1);
        let game = &library.games[0];
        assert!(!game.identified);
        assert_ne!(game.identity_summary(), "Verified");
    }

    // --- ES-DE candidates ---------------------------------------------

    fn esde_entry(
        media_root: &Path,
        platform_dir: &str,
        xml: &[u8],
        platform: &str,
        path: &str,
    ) -> Option<archivefs_core::emulator_environment::es_de_metadata::EsDeResolvedEntry> {
        let index = parse_and_index_gamelist_with_roots(
            Path::new("/es/gamelists/x/gamelist.xml"),
            xml,
            platform_dir,
            1,
            media_root,
            Some(Path::new("/roms")),
        );
        index.lookup_path(platform, Path::new(path))
    }

    #[test]
    fn esde_candidates_skip_references_to_files_that_are_not_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("downloaded_media");
        std::fs::create_dir_all(media.join("snes/miximages")).unwrap();
        std::fs::write(media.join("snes/miximages/Present.png"), b"png").unwrap();
        let xml = b"<gameList>\
            <game><path>./Present.sfc</path><image>./images/Present-image.png</image></game>\
            <game><path>./Gone.sfc</path><image>./images/Gone-image.png</image></game>\
            </gameList>";
        let present = esde_entry(&media, "snes", xml, "SNES", "/roms/snes/Present.sfc").unwrap();
        let candidates = esde_artwork_candidates(&present);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0.kind, AssetKind::CoverFront);
        let gone = esde_entry(&media, "snes", xml, "SNES", "/roms/snes/Gone.sfc").unwrap();
        assert!(gone.entry.media.cover.is_some());
        assert!(esde_artwork_candidates(&gone).is_empty());
    }

    #[test]
    fn an_unusable_esde_reference_does_not_suppress_a_usable_lower_priority_cover() {
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("downloaded_media");
        std::fs::create_dir_all(&media).unwrap();
        let xml = b"<gameList><game><path>./Gone.sfc</path><image>./images/Gone-image.png</image></game></gameList>";
        let gone = esde_entry(&media, "snes", xml, "SNES", "/roms/snes/Gone.sfc").unwrap();
        let romm = ArtworkCandidate {
            kind: AssetKind::CoverFront,
            path: PathBuf::from("romm://9/cover"),
            cached: false,
            provenance: Provenance {
                provider: Provider::Romm,
                source_class: SourceClass::ProviderCache,
                retrieved_at_unix_seconds: 0,
                detail: "RomM identity cache".into(),
                cache_path: None,
            },
        };
        let mut artwork: Vec<_> = esde_artwork_candidates(&gone)
            .into_iter()
            .map(|(candidate, _)| candidate)
            .collect();
        artwork.push(romm);
        let resolved = metadata_aggregation::resolve(AggregationInput {
            artwork,
            ..Default::default()
        });
        assert_eq!(
            resolved.artwork[&AssetKind::CoverFront].provenance.provider,
            Provider::Romm
        );
    }

    #[test]
    fn a_usable_local_esde_cover_is_preferred_over_a_remote_one() {
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("downloaded_media");
        std::fs::create_dir_all(media.join("snes/miximages")).unwrap();
        std::fs::write(media.join("snes/miximages/Game.png"), b"png").unwrap();
        let xml = b"<gameList><game><path>./Game.sfc</path><image>./images/Game-image.png</image></game></gameList>";
        let entry = esde_entry(&media, "snes", xml, "SNES", "/roms/snes/Game.sfc").unwrap();
        let mut artwork: Vec<_> = esde_artwork_candidates(&entry)
            .into_iter()
            .map(|(candidate, _)| candidate)
            .collect();
        artwork.push(ArtworkCandidate {
            kind: AssetKind::CoverFront,
            path: PathBuf::from("romm://9/cover"),
            cached: false,
            provenance: Provenance {
                provider: Provider::Romm,
                source_class: SourceClass::ProviderCache,
                retrieved_at_unix_seconds: 0,
                detail: "RomM".into(),
                cache_path: None,
            },
        });
        let resolved = metadata_aggregation::resolve(AggregationInput {
            artwork,
            ..Default::default()
        });
        assert_eq!(
            resolved.artwork[&AssetKind::CoverFront].provenance.provider,
            Provider::EsDe
        );
    }

    #[test]
    fn the_same_file_on_two_platforms_never_cross_matches() {
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("downloaded_media");
        std::fs::create_dir_all(&media).unwrap();
        let xml = b"<gameList><game><path>./Sonic.zip</path><name>Sonic</name></game></gameList>";
        assert!(
            esde_entry(
                &media,
                "genesis",
                xml,
                "MegaDrive",
                "/roms/genesis/Sonic.zip"
            )
            .is_some()
        );
        // The identical path under another platform is a different key.
        assert!(
            esde_entry(
                &media,
                "genesis",
                xml,
                "MasterSystem",
                "/roms/genesis/Sonic.zip"
            )
            .is_none()
        );
        assert!(
            esde_entry(
                &media,
                "genesis",
                xml,
                "MegaDrive",
                "/roms/genesis/Sonic (Rev A).zip"
            )
            .is_none()
        );
    }

    // --- diagnostics ----------------------------------------------------

    #[test]
    fn explain_cover_says_what_each_provider_had_without_guessing() {
        let library = library(vec![
            row(1, "SNES", "/g/snes/Known.zip"),
            row(2, "SNES", "/g/snes/Unknown.zip"),
        ]);
        let mut index = MediaIndex::default();
        index.providers = Some(Arc::new(ProviderData {
            esde_collections: Vec::new(),
            esde_root_count: 0,
            records: records(vec![record("9", "/g/snes/Known.zip", "SNES", false)]),
            wrapper_matches: HashMap::new(),
            ambiguous: HashSet::new(),
        }));
        let known = index.explain_cover(&library.games[0]);
        assert!(known.contains("RomM: record 9 matched by exact path"));
        assert!(known.contains("the record has no artwork"));
        assert!(known.contains("Result: no cover candidate from any provider"));
        let unknown = index.explain_cover(&library.games[1]);
        assert!(unknown.contains("no cached record for this exact path"));
        assert!(unknown.contains("ES-DE: not available"));
        assert_eq!(
            MediaIndex::default().explain_cover(&library.games[0]),
            "Providers have not been indexed yet."
        );
    }
}
