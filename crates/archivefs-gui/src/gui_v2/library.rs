//! Lightweight catalogue projection. Never turns filename hints into identity.
use archivefs_core::{
    PersistedArchive,
    launch::{CanonicalIdentityStatus, canonical_identity_from_game_report},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DuplicateMember {
    pub path: std::path::PathBuf,
    pub title: String,
    pub platform: String,
    pub size_bytes: u64,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DuplicateGroup {
    pub kind: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub members: Vec<DuplicateMember>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct DuplicateReport {
    pub groups: Vec<DuplicateGroup>,
    pub files_examined: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Game {
    pub archive: PersistedArchive,
    pub title: String,
    pub platform: String,
    pub identified: bool,
    pub attention: bool,
    search: String,
}

impl Game {
    pub fn from_archive(archive: PersistedArchive) -> Self {
        let title = archive.display_name.clone();
        let platform = archive
            .platform
            .clone()
            .unwrap_or_else(|| "Unknown system".into());
        let identified = archive.identity_report.as_ref().is_some_and(|report| {
            matches!(
                canonical_identity_from_game_report(report).0,
                CanonicalIdentityStatus::Resolved(_)
            )
        });
        let attention = archive.last_verified_missing_at.is_some()
            || matches!(
                archive.last_known_health.as_str(),
                "missing" | "corrupt" | "damaged" | "error"
            );
        let search = format!("{title} {platform}").to_lowercase();
        Self {
            archive,
            title,
            platform,
            identified,
            attention,
            search,
        }
    }
    pub fn status(&self) -> &'static str {
        if self.attention {
            "Needs attention"
        } else if self.identified {
            "Identified · not yet checked for play"
        } else {
            "Not checked yet"
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct Library {
    pub games: Vec<Game>,
    pub by_id: HashMap<i64, usize>,
    pub platforms: BTreeMap<String, usize>,
    pub attention: usize,
    pub sources: usize,
    pub load_ms: u128,
    pub scan_warning: Option<String>,
}

impl Library {
    pub fn new(archives: Vec<PersistedArchive>) -> Self {
        let mut games: Vec<_> = archives.into_iter().map(Game::from_archive).collect();
        games.sort_by_cached_key(|game| (game.title.to_lowercase(), game.archive.id));
        let mut library = Self {
            games,
            ..Self::default()
        };
        for (index, game) in library.games.iter().enumerate() {
            library.by_id.insert(game.archive.id, index);
            *library.platforms.entry(game.platform.clone()).or_default() += 1;
            library.attention += usize::from(game.attention);
        }
        library
    }
    pub fn game(&self, id: i64) -> Option<&Game> {
        self.by_id.get(&id).and_then(|index| self.games.get(*index))
    }
    pub fn filter(&self, filter: &Filter) -> Vec<usize> {
        let needle = filter.search.trim().to_lowercase();
        self.games
            .iter()
            .enumerate()
            .filter(|(_, game)| {
                (filter.platform.is_empty() || game.platform == filter.platform)
                    && (!filter.attention_only || game.attention)
                    && (needle.is_empty() || game.search.contains(&needle))
            })
            .map(|(index, _)| index)
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Filter {
    pub search: String,
    pub platform: String,
    pub attention_only: bool,
    pub list: bool,
}

pub(super) type SharedLibrary = Arc<Library>;

#[derive(Clone, Debug, Default)]
pub(super) struct Detail {
    pub game: i64,
    pub file_present: bool,
    pub unchanged: bool,
    pub saved_checks: usize,
    pub installed: Vec<String>,
    pub technical: String,
}

impl Detail {
    pub fn emulator_status(&self) -> String {
        if self.installed.is_empty() {
            "Needs setup · no matching emulator found by discovery".into()
        } else {
            format!(
                "Found: {} · Play checks game and firmware readiness",
                self.installed.join(", ")
            )
        }
    }
}
