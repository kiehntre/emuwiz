//! Read-only discovery over existing providers. No title/filename matching.
use super::library::Library;
use archivefs_core::identity_source::{
    model::{ExternalIdentityRecord, IdentityProvider},
    settings::{ProviderSettings, SettingsLocation, default_identity_root},
    status::IdentitySourceApi,
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
        for game in &library.games {
            let id = game.archive.id;
            let path = &game.archive.absolute_path;
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
                    index.covers.insert(id, Source::Local(path));
                }
                if let Some(path) = entry.entry.media.screenshot {
                    index
                        .screenshots
                        .entry(id)
                        .or_default()
                        .push(Source::Local(path));
                }
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
                    index
                        .covers
                        .entry(id)
                        .or_insert_with(|| Source::Local(path.into()));
                }
                for reference in snapshot.screenshots.into_iter().take(8) {
                    if let Some(path) = reference.hosted_reference {
                        index
                            .screenshots
                            .entry(id)
                            .or_default()
                            .push(Source::Local(path.into()));
                    }
                }
            }
            if let Some(record) = records.get(path) {
                let description = [
                    record.synopsis.clone().unwrap_or_default(),
                    record.genres.join(" · "),
                    record
                        .players
                        .as_ref()
                        .map(|players| format!("Players: {players}"))
                        .unwrap_or_default(),
                    record
                        .release_year
                        .map(|year| format!("Released: {year}"))
                        .unwrap_or_default(),
                ]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
                if !description.is_empty() {
                    index.descriptions.insert(id, description);
                }
            }
            let local_count = index.screenshots.get(&id).map_or(0, Vec::len);
            if let Some(record) = records.get(path)
                && let Some(artwork) = &record.artwork
            {
                index.covers.entry(id).or_insert_with(|| Source::Remote {
                    record: record.clone(),
                    kind: Kind::Cover,
                });
                for ordinal in 0..artwork.screenshots.len().min(8) {
                    index
                        .screenshots
                        .entry(id)
                        .or_default()
                        .push(Source::Remote {
                            record: record.clone(),
                            kind: Kind::Screenshot(ordinal),
                        });
                }
            }
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
