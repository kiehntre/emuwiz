//! Read-only discovery over existing providers. No title/filename matching.
use super::library::Library;
mod sparse;
use archivefs_core::identity_source::{
    model::{ExternalIdentityRecord, IdentityProvider},
    settings::{ProviderSettings, SettingsLocation, default_identity_root},
    status::IdentitySourceApi,
};
use archivefs_core::metadata_aggregation::{
    self, AggregationInput, ArtworkCandidate, AssetKind, MetadataCandidate, MetadataField,
    Provenance, Provider, ProviderStatus, SourceClass,
};
use sparse::{base_metadata, provider_status};
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
    pub(super) fallback_sources: String,
}

impl MediaIndex {
    /// Manuals require an unambiguous exact path AND canonical platform.
    /// Artwork-only wrapper matching never establishes a document association.
    pub(super) fn manual_reference(
        &self,
        game: &super::library::Game,
    ) -> Option<(
        &str,
        &archivefs_core::identity_source::model::MediaReference,
    )> {
        self.manual_reference_checked(game).ok().flatten()
    }

    /// Like [`Self::manual_reference`], but explains why a manual that the
    /// provider does list is being withheld, so the panel can refuse clearly
    /// instead of silently showing nothing. `Ok(None)` means the provider
    /// lists no manual for this game.
    pub(super) fn manual_reference_checked(
        &self,
        game: &super::library::Game,
    ) -> Result<
        Option<(
            &str,
            &archivefs_core::identity_source::model::MediaReference,
        )>,
        &'static str,
    > {
        use archivefs_core::identity_source::model::ExternalVerification;
        let Some(data) = self.providers.as_ref() else {
            return Ok(None);
        };
        let path = &game.archive.absolute_path;
        let Some(record) = data.records.get(path) else {
            return Ok(None);
        };
        let Some(manual) = record.artwork.as_ref().and_then(|a| a.manual.as_ref()) else {
            return Ok(None);
        };
        if data.ambiguous.contains(path)
            || matches!(record.verification, ExternalVerification::Ambiguous)
        {
            return Err(
                "RomM lists a manual, but more than one RomM record matches this game, so it is not shown.",
            );
        }
        if matches!(record.verification, ExternalVerification::Stale) {
            return Err(
                "RomM lists a manual, but its record for this game is out of date. Refresh the RomM index to use it.",
            );
        }
        if matches!(record.verification, ExternalVerification::Unmatched) {
            return Err("RomM lists a manual, but its record is not matched to this game.");
        }
        if !record.conflicts.is_empty() {
            return Err(
                "RomM lists a manual, but its record conflicts with this game, so it is not shown.",
            );
        }
        let platform_matches = record
            .platform_candidate
            .as_deref()
            .and_then(archivefs_core::canonical_platform_for_alias)
            .is_some_and(|platform| {
                Some(platform) == archivefs_core::canonical_platform_for_alias(&game.platform)
            });
        if !platform_matches {
            return Err("RomM lists a manual, but its platform does not match this game.");
        }
        Ok(Some((&record.provider_game_id, manual)))
    }

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
        let local_folders = super::local_artwork_folders::configured();
        let custom_esde: Vec<_> = local_folders
            .iter()
            .filter(|folder| {
                folder.enabled
                    && folder.layout == super::local_artwork_folders::Layout::EsDe
                    && super::local_artwork_folders::safe_existing(folder)
            })
            .map(|folder| folder.path.clone())
            .collect();
        let mut esde_root_specs: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        if let Some(root) = &es_de_root {
            let mut roots = Vec::new();
            for candidate in [
                archivefs_core::emulator_environment::es_de_metadata::configured_rom_directory(
                    root,
                ),
                config
                    .as_ref()
                    .and_then(|config| config.master_rom_root.clone()),
            ] {
                if candidate.is_some() && !roots.contains(&candidate) {
                    roots.push(candidate);
                }
            }
            if roots.is_empty() {
                roots.push(None);
            }
            esde_root_specs.extend(roots.into_iter().map(|rom_root| (root.clone(), rom_root)));
        }
        for root in custom_esde {
            let rom_roots = [
                archivefs_core::emulator_environment::es_de_metadata::configured_rom_directory(
                    &root,
                ),
                config
                    .as_ref()
                    .and_then(|config| config.master_rom_root.clone()),
            ];
            let mut candidates = Vec::new();
            for candidate in rom_roots {
                if candidate.is_some() && !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }
            if candidates.is_empty() {
                candidates.push(None);
            }
            esde_root_specs.extend(
                candidates
                    .into_iter()
                    .map(|rom_root| (root.clone(), rom_root)),
            );
        }
        let esde_collections: Vec<_> = esde_root_specs.iter()
            .map(|(root, rom_root)| archivefs_core::emulator_environment::es_de_metadata::discover_provider_snapshot_with_rom_root(root, 1, rom_root.as_deref()))
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
        let mut launchbox_roots: Vec<PathBuf> = launchbox_root.into_iter().collect();
        launchbox_roots.extend(
            local_folders
                .iter()
                .filter(|folder| {
                    folder.enabled
                        && folder.layout == super::local_artwork_folders::Layout::LaunchBox
                        && super::local_artwork_folders::safe_existing(folder)
                })
                .map(|folder| folder.path.clone()),
        );
        let mut seen_launchbox_roots = HashSet::new();
        launchbox_roots.retain(|root| seen_launchbox_roots.insert(root.clone()));
        let launchbox: Vec<_> = launchbox_roots
            .into_iter()
            .filter(|root| root.is_dir())
            .filter_map(|root| {
                match archivefs_core::identity_source::launchbox_local::discover_launchbox_local(
                    &root, 1,
                ) {
                    Ok(snapshot) => Some(snapshot),
                    Err(error) => {
                        index.warnings.push(error);
                        None
                    }
                }
            })
            .collect();
        index.fallback_sources = format!(
            "ES-DE: {}. LaunchBox: {}.",
            if esde_present {
                "local index checked"
            } else {
                "not available at the configured/default location"
            },
            if !launchbox.is_empty() {
                "local index checked"
            } else {
                "not available at the configured/default location"
            }
        );
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
            // No provider can contribute to this row. Its neutral local-only
            // projection and generic diagnostic are generated for selection,
            // rather than retaining identical scaffolding for every game.
            if matched_record.is_none()
                && !ambiguous.contains(path)
                && !esde_by_platform.contains_key(game.platform.as_str())
                && launchbox.is_empty()
                && game.screenscraper.is_none()
            {
                continue;
            }
            let mut external_match = matched_record.is_some()
                || ambiguous.contains(path)
                || game.screenscraper.is_some();
            let mut metadata = base_metadata(game);
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
            let mut sources = index.fallback_sources.clone();
            // Prefer already-local pictures; a remote server must never hold them up.
            if let Some(entry) = esde_by_platform
                .get(game.platform.as_str())
                .and_then(|indexes| {
                    indexes
                        .iter()
                        .find_map(|provider| provider.lookup_path(&game.platform, path))
                })
            {
                external_match = true;
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
            if let Some((provider, found)) = launchbox.iter().find_map(|provider| {
                provider
                    .lookup(None, Some(path), Some(&game.platform), None)
                    .map(|found| (provider, found))
            }) {
                external_match = true;
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
                                detail: found.game.provenance.clone(),
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
                                    detail: found.game.provenance.clone(),
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
            let local_count = artwork_candidates
                .iter()
                .filter(|(candidate, source)| {
                    candidate.kind == AssetKind::Screenshot && matches!(source, Source::Local(_))
                })
                .count();
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
            if !external_match {
                continue;
            }
            let resolved = metadata_aggregation::resolve(AggregationInput {
                metadata,
                artwork: artwork_candidates
                    .iter()
                    .map(|(candidate, _)| candidate.clone())
                    .collect(),
                provider_status: provider_status(
                    esde_present,
                    matched_record.is_some(),
                    game.screenscraper.is_some(),
                ),
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
            // The neutral resolver chooses a representative per artwork kind.
            // Keep the full screenshot gallery belonging to that selected source,
            // in provider order, instead of treating the representative as the
            // entire gallery. Other providers' screenshots cannot enter it.
            if let Some(winner) = resolved.artwork.get(&AssetKind::Screenshot) {
                let mut seen = HashSet::new();
                let gallery: Vec<_> = artwork_candidates
                    .iter()
                    .filter(|(candidate, _)| {
                        candidate.kind == AssetKind::Screenshot
                            && candidate.provenance.provider == winner.provenance.provider
                            && candidate.provenance.source_class == winner.provenance.source_class
                            && candidate.provenance.detail == winner.provenance.detail
                            && seen.insert(candidate.path.clone())
                    })
                    .take(8)
                    .map(|(_, source)| source.clone())
                    .collect();
                if !gallery.is_empty() {
                    index.screenshots.insert(id, gallery);
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
            esde_root_count: esde_root_specs.len(),
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
    if records.is_empty() {
        return HashMap::new();
    }
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

    #[test]
    fn same_title_on_two_platforms_resolves_only_its_own_artwork() {
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("downloaded_media");
        for dir in ["apple2", "bbcmicro"] {
            std::fs::create_dir_all(media.join(dir).join("covers")).unwrap();
            std::fs::write(media.join(dir).join("covers/Adventure.png"), dir.as_bytes()).unwrap();
        }
        let xml =
            b"<gameList><game><path>./Adventure.zip</path><name>Adventure</name></game></gameList>";
        let index_for = |dir: &str| {
            parse_and_index_gamelist_with_roots(
                Path::new("/es/gamelists/x/gamelist.xml"),
                xml,
                dir,
                1,
                &media,
                Some(Path::new("/roms")),
            )
        };
        let apple = index_for("apple2");
        let bbc = index_for("bbcmicro");
        // Each platform resolves exactly its own file, via its own index.
        let apple_hit = apple
            .lookup_path("Apple II", Path::new("/roms/apple2/Adventure.zip"))
            .unwrap();
        let bbc_hit = bbc
            .lookup_path("BBC Micro", Path::new("/roms/bbcmicro/Adventure.zip"))
            .unwrap();
        assert_eq!(
            apple_hit.entry.media.cover,
            Some(media.join("apple2/covers/Adventure.png"))
        );
        assert_eq!(
            bbc_hit.entry.media.cover,
            Some(media.join("bbcmicro/covers/Adventure.png"))
        );
        // Same title AND same file name, asked under the other platform: no hit
        // from either index, so nothing can fall back to the wrong cover.
        assert!(
            apple
                .lookup_path("BBC Micro", Path::new("/roms/apple2/Adventure.zip"))
                .is_none()
        );
        assert!(
            bbc.lookup_path("Apple II", Path::new("/roms/bbcmicro/Adventure.zip"))
                .is_none()
        );
        assert!(
            apple
                .lookup_path("Apple II", Path::new("/roms/bbcmicro/Adventure.zip"))
                .is_none()
        );
        // The resolver only ever sees the candidates of the matched index.
        let apple_candidates = esde_artwork_candidates(&apple_hit);
        assert!(
            apple_candidates
                .iter()
                .all(|(candidate, _)| candidate.path.starts_with(media.join("apple2")))
        );
    }

    #[test]
    fn wrapper_rule_same_file_name_on_two_platforms_claims_only_its_own_platform() {
        let records = records(vec![
            record("1", "/g/a/Game.zip", "Sharp X68000", true),
            record("2", "/g/b/Game.zip", "MegaDrive", true),
        ]);
        let rows = vec![
            row(1, "Sharp X68000", "/g/a/Wrap/Game.zip"),
            row(2, "MegaDrive", "/g/b/Wrap/Game.zip"),
            // Right directory, wrong platform for that record.
            row(3, "MegaDrive", "/g/a/Wrap2/Game.zip"),
        ];
        let forward = wrapper_record_matches(&library(rows.clone()), &records);
        let mut reversed_rows = rows;
        reversed_rows.reverse();
        let reversed = wrapper_record_matches(&library(reversed_rows), &records);
        // Row order never changes the outcome.
        assert_eq!(forward, reversed);
        assert_eq!(forward.len(), 2);
        assert_eq!(
            forward[Path::new("/g/a/Wrap/Game.zip")],
            PathBuf::from("/g/a/Game.zip")
        );
        assert_eq!(
            forward[Path::new("/g/b/Wrap/Game.zip")],
            PathBuf::from("/g/b/Game.zip")
        );
        assert!(!forward.contains_key(Path::new("/g/a/Wrap2/Game.zip")));
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
    fn manual_fixture_index(game_path: &Path, root: &Path) -> MediaIndex {
        let mut record = serde_json::to_value(
            record("manual-7", game_path.to_str().unwrap(), "Saturn", false).as_ref(),
        )
        .unwrap();
        record["artwork"] = serde_json::json!({
            "reference": "", "small_reference": null,
            "manual": { "hosted_reference": "/manuals/guide.cbz", "public_reference": null }
        });
        let record = Arc::new(serde_json::from_value(record).unwrap());
        let mut index = MediaIndex::default();
        index.settings.source.media_mapping = Some(
            archivefs_core::identity_source::romm::media_mapping::RommMediaMapping {
                provider_prefix: "/manuals".into(),
                local_root: root.to_path_buf(),
            },
        );
        index.providers = Some(Arc::new(ProviderData {
            esde_collections: Vec::new(),
            esde_root_count: 0,
            records: records(vec![record]),
            wrapper_matches: HashMap::new(),
            ambiguous: HashSet::new(),
        }));
        index
    }

    #[test]
    fn selected_game_romm_manual_uses_existing_mapping_model_and_provenance() {
        use super::super::{
            documents::{DocumentPreferences, GameDocumentAssociation, GameDocumentSource},
            game_documents::discover_game_documents,
            library::Game,
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("guide.cbz");
        super::super::manual_viewer::tests::cbz(&path);
        let before = std::fs::read(&path).unwrap();
        let game_path = root.path().join("game.bin");
        let game = Game::from_archive(row(7, "Saturn", game_path.to_str().unwrap()));
        let index = manual_fixture_index(&game_path, root.path());
        let mut prefs = DocumentPreferences::default();
        let (docs, issue) = discover_game_documents(&game, &prefs, Some(&index));
        assert!(issue.is_none());
        assert_eq!(docs.len(), 1, "nearby and RomM paths deduplicate");
        assert_eq!(docs[0].association, GameDocumentAssociation::RommRecord);
        assert!(matches!(docs[0].source, GameDocumentSource::Romm(_)));
        assert!(docs[0].association_reason.contains("manual-7"));
        assert!(
            docs[0]
                .association_reason
                .contains("not document-content verification")
        );
        assert_eq!(docs[0].page_count, Some(3));
        prefs.associations.insert(path.clone(), 7);
        let (docs, _) = discover_game_documents(&game, &prefs, Some(&index));
        assert_eq!(docs[0].association, GameDocumentAssociation::Explicit);
        prefs.associations.insert(path.clone(), 99);
        assert!(
            discover_game_documents(&game, &prefs, Some(&index))
                .0
                .is_empty()
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn romm_manual_requires_exact_path_platform_and_unambiguous_current_record() {
        use super::super::library::Game;
        use archivefs_core::identity_source::model::ExternalVerification;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.bin");
        let game = Game::from_archive(row(7, "Saturn", path.to_str().unwrap()));
        for state in [
            ExternalVerification::Stale,
            ExternalVerification::Unmatched,
            ExternalVerification::Ambiguous,
        ] {
            let mut index = manual_fixture_index(&path, root.path());
            let data = Arc::get_mut(index.providers.as_mut().unwrap()).unwrap();
            Arc::get_mut(data.records.get_mut(&path).unwrap())
                .unwrap()
                .verification = state;
            assert!(index.manual_reference(&game).is_none());
        }
        let mut index = manual_fixture_index(&path, root.path());
        let data = Arc::get_mut(index.providers.as_mut().unwrap()).unwrap();
        data.ambiguous.insert(path.clone());
        assert!(index.manual_reference(&game).is_none());
        let wrong_platform = Game::from_archive(row(7, "NES", path.to_str().unwrap()));
        let mut index = manual_fixture_index(&path, root.path());
        assert!(index.manual_reference(&wrong_platform).is_none());
        let wrapper = root.path().join("wrapper/game.bin");
        Arc::get_mut(index.providers.as_mut().unwrap())
            .unwrap()
            .wrapper_matches
            .insert(wrapper.clone(), path.clone());
        let wrapper_game = Game::from_archive(row(8, "Saturn", wrapper.to_str().unwrap()));
        assert!(index.manual_reference(&wrapper_game).is_none());
    }

    #[test]
    fn romm_manual_mapping_and_missing_file_explain_unavailability() {
        use super::super::{
            documents::DocumentPreferences, game_documents::discover_game_documents, library::Game,
        };
        let root = tempfile::tempdir().unwrap();
        let games = root.path().join("games");
        std::fs::create_dir(&games).unwrap();
        let path = games.join("game.bin");
        let game = Game::from_archive(row(7, "Saturn", path.to_str().unwrap()));
        let mut index = manual_fixture_index(&path, root.path());
        let prefs = DocumentPreferences::default();
        let (docs, issue) = discover_game_documents(&game, &prefs, Some(&index));
        assert!(docs.is_empty());
        assert!(issue.unwrap().contains("no longer available"));
        index.settings.source.media_mapping = None;
        let (_, issue) = discover_game_documents(&game, &prefs, Some(&index));
        assert!(issue.unwrap().contains("safely map"));
    }

    #[test]
    fn romm_manual_withheld_states_give_clear_refusals() {
        use super::super::{
            documents::DocumentPreferences, game_documents::discover_game_documents, library::Game,
        };
        use archivefs_core::identity_source::model::ExternalVerification;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.bin");
        let game = Game::from_archive(row(7, "Saturn", path.to_str().unwrap()));
        let prefs = DocumentPreferences::default();
        for (state, expected) in [
            (ExternalVerification::Stale, "out of date"),
            (ExternalVerification::Unmatched, "not matched"),
            (ExternalVerification::Ambiguous, "more than one"),
        ] {
            let mut index = manual_fixture_index(&path, root.path());
            let data = Arc::get_mut(index.providers.as_mut().unwrap()).unwrap();
            Arc::get_mut(data.records.get_mut(&path).unwrap())
                .unwrap()
                .verification = state;
            let (docs, issue) = discover_game_documents(&game, &prefs, Some(&index));
            assert!(docs.is_empty());
            assert!(issue.unwrap().contains(expected));
        }
        let mut index = manual_fixture_index(&path, root.path());
        Arc::get_mut(index.providers.as_mut().unwrap())
            .unwrap()
            .ambiguous
            .insert(path.clone());
        assert!(
            discover_game_documents(&game, &prefs, Some(&index))
                .1
                .unwrap()
                .contains("more than one")
        );
        let wrong = Game::from_archive(row(7, "NES", path.to_str().unwrap()));
        let index = manual_fixture_index(&path, root.path());
        assert!(
            discover_game_documents(&wrong, &prefs, Some(&index))
                .1
                .unwrap()
                .contains("platform")
        );
    }

    #[test]
    fn game_without_romm_manual_is_not_an_error() {
        use super::super::{
            documents::DocumentPreferences, game_documents::discover_game_documents, library::Game,
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.bin");
        let game = Game::from_archive(row(7, "Saturn", path.to_str().unwrap()));
        let mut index = manual_fixture_index(&path, root.path());
        let data = Arc::get_mut(index.providers.as_mut().unwrap()).unwrap();
        Arc::get_mut(data.records.get_mut(&path).unwrap())
            .unwrap()
            .artwork = None;
        let (docs, issue) =
            discover_game_documents(&game, &DocumentPreferences::default(), Some(&index));
        assert!(docs.is_empty() && issue.is_none());
    }

    #[test]
    fn romm_manual_remote_only_and_unsupported_are_refused_locally() {
        use super::super::{
            documents::DocumentPreferences, game_documents::discover_game_documents, library::Game,
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.bin");
        let game = Game::from_archive(row(7, "Saturn", path.to_str().unwrap()));
        let prefs = DocumentPreferences::default();
        // Remote-only reference: never fetched, refused as unmappable.
        let mut index = manual_fixture_index(&path, root.path());
        let data = Arc::get_mut(index.providers.as_mut().unwrap()).unwrap();
        let record = Arc::get_mut(data.records.get_mut(&path).unwrap()).unwrap();
        let manual = record.artwork.as_mut().unwrap().manual.as_mut().unwrap();
        manual.hosted_reference = None;
        manual.public_reference = Some("https://example.invalid/guide.pdf".into());
        let (docs, issue) = discover_game_documents(&game, &prefs, Some(&index));
        assert!(docs.is_empty());
        assert!(issue.unwrap().contains("safely map"));
        // Unsupported content behind an allowed name is listed but not openable.
        std::fs::write(root.path().join("guide.cbz"), b"not an archive").unwrap();
        let index = manual_fixture_index(&path, root.path());
        let (docs, _) = discover_game_documents(&game, &prefs, Some(&index));
        assert_eq!(docs.len(), 1);
        assert_ne!(
            docs[0].viewer,
            super::super::documents::DocumentOpenCapability::Supported
        );
        assert!(docs[0].refusal_reason.is_some());
    }

    #[test]
    fn romm_manual_with_identity_conflict_is_refused_distinctly() {
        use super::super::{
            documents::DocumentPreferences, game_documents::discover_game_documents, library::Game,
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.bin");
        let game = Game::from_archive(row(7, "Saturn", path.to_str().unwrap()));
        let mut index = manual_fixture_index(&path, root.path());
        let data = Arc::get_mut(index.providers.as_mut().unwrap()).unwrap();
        let record = Arc::get_mut(data.records.get_mut(&path).unwrap()).unwrap();
        record
            .conflicts
            .push(archivefs_core::identity_source::model::IdentityConflict {
                field: archivefs_core::identity_source::model::ConflictField::Platform,
                external: "a".into(),
                local: "b".into(),
                detail: "d".into(),
            });
        let (docs, issue) =
            discover_game_documents(&game, &DocumentPreferences::default(), Some(&index));
        assert!(docs.is_empty());
        assert!(issue.unwrap().contains("conflicts"));
    }
}
