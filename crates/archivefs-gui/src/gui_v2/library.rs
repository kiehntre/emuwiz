//! Lightweight catalogue projection. Never turns filename hints into identity.
use archivefs_core::{
    ArchiveKind, PersistedArchive,
    launch::{CanonicalIdentityStatus, canonical_identity_from_game_report},
};

pub(super) fn media_kind_label(storage_name: &str) -> &'static str {
    match ArchiveKind::from_storage(storage_name) {
        Some(ArchiveKind::Zip) => "ZIP",
        Some(ArchiveKind::SevenZip) => "7z",
        Some(ArchiveKind::Rar) => "RAR",
        Some(ArchiveKind::MegaDriveRom) => "Mega Drive ROM",
        Some(ArchiveKind::DirectGameImage) => "Game image",
        Some(ArchiveKind::ArcadeSetDirectory) => "Arcade set",
        None if storage_name.eq_ignore_ascii_case("iso") => "Game image",
        None => "Media",
    }
}
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

pub(crate) const UNKNOWN_PLATFORM: &str = "Unknown system";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlatformProjection {
    pub(crate) total: usize,
    pub(crate) assigned: usize,
    pub(crate) missing: usize,
}

/// Projects the persisted catalogue rows into the one platform-count view
/// shared by GUI v2 and the legacy Museum handoff. Every row is counted,
/// including unassigned and missing rows; health affects the badges shown for
/// a row, never whether the row exists in a platform projection.
pub(crate) fn project_platforms(
    archives: &[PersistedArchive],
) -> BTreeMap<String, PlatformProjection> {
    let mut platforms = BTreeMap::new();
    for archive in archives {
        let platform = canonical_platform_name(archive.platform.as_deref());
        let entry = platforms
            .entry(platform)
            .or_insert_with(PlatformProjection::default);
        entry.total += 1;
        entry.assigned += usize::from(
            archive
                .platform
                .as_deref()
                .is_some_and(|platform| !platform.trim().is_empty()),
        );
        entry.missing += usize::from(archive.last_verified_missing_at.is_some());
    }
    platforms
}

/// Resolves a persisted platform ID or alias without rewriting the stored
/// catalogue. Unknown assigned IDs remain visible verbatim; only an absent
/// assignment becomes the explicit unknown bucket.
pub(crate) fn canonical_platform_name(platform: Option<&str>) -> String {
    let Some(raw) = platform.filter(|raw| !raw.trim().is_empty()) else {
        return UNKNOWN_PLATFORM.to_string();
    };
    archivefs_core::canonical_platform_for_alias(raw)
        .unwrap_or(raw)
        .to_string()
}

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
    pub exact_index: usize,
    pub kind: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub members: Vec<DuplicateMember>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct DuplicateReport {
    pub groups: Vec<DuplicateGroup>,
    pub files_examined: usize,
    pub exact_groups: Vec<archivefs_core::repair::ExactDuplicateGroup>,
}

#[derive(Clone, Debug)]
pub(super) struct Game {
    pub archive: PersistedArchive,
    pub title: String,
    pub platform: String,
    pub identified: bool,
    pub attention: bool,
    pub screenscraper:
        Option<archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment>,
    pub(super) search: String,
}

impl Game {
    pub fn from_archive(archive: PersistedArchive) -> Self {
        let title = archive.display_name.clone();
        let platform = canonical_platform_name(archive.platform.as_deref());
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
            screenscraper: None,
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

    pub fn identity_summary(&self) -> &'static str {
        let Some(report) = self.archive.identity_report.as_ref() else {
            return "Unknown";
        };
        match canonical_identity_from_game_report(report).0 {
            CanonicalIdentityStatus::Resolved(_) => "Verified",
            CanonicalIdentityStatus::Conflicting => "Mismatch",
            CanonicalIdentityStatus::Unknown => "Needs verification",
        }
    }
}

/// A catalogue row from an older library location that a current row replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HistoricalLink {
    pub current_id: i64,
    pub evidence: archivefs_core::catalogue_supersession::SupersessionEvidence,
}

#[derive(Clone, Default)]
pub(super) struct Library {
    /// Current catalogue rows only. A row that a current row confidently
    /// replaces is kept in [`Self::historical`], never listed as a second game.
    pub games: Vec<Game>,
    pub by_id: HashMap<i64, usize>,
    pub platforms: BTreeMap<String, usize>,
    pub platform_sources: BTreeMap<String, BTreeMap<(i64, std::path::PathBuf), usize>>,
    pub attention: usize,
    pub sources: usize,
    pub load_ms: u128,
    pub scan_warning: Option<String>,
    /// Superseded rows, preserved for provenance and Advanced details.
    pub historical: Vec<Game>,
    pub historical_links: HashMap<i64, HistoricalLink>,
    pub(super) historical_of: HashMap<i64, Vec<usize>>,
    /// What the identity classification needs beyond the rows themselves.
    pub identity_context: IdentityContext,
}

/// Reference-data facts used to explain unconfirmed identities. Filled when the
/// library loads; empty (so nothing is claimed) otherwise.
#[derive(Clone, Debug, Default)]
pub(super) struct IdentityContext {
    /// Installed identification catalogues, when they could be listed.
    pub inventory: Option<archivefs_core::identity_attention::ReferenceInventory>,
    /// Games already matched against reference data by an audit.
    pub matched: std::collections::HashSet<i64>,
}

impl Library {
    pub fn new(archives: Vec<PersistedArchive>) -> Self {
        Self::with_history(archives, &[], None)
    }

    /// Builds the library from every persisted row. `renames` are EmuWiz's own
    /// applied rename moves and `configured_sources` the sources still in the
    /// configuration (`None`: all of them); both are evidence that an old row has
    /// been replaced. Nothing is deleted: replaced rows move to
    /// [`Self::historical`].
    pub fn with_history(
        archives: Vec<PersistedArchive>,
        renames: &[(std::path::PathBuf, std::path::PathBuf)],
        configured_sources: Option<&std::collections::HashSet<i64>>,
    ) -> Self {
        let links = archivefs_core::catalogue_supersession::derive_supersessions(
            &archives,
            renames,
            configured_sources,
        );
        let (old, current): (Vec<_>, Vec<_>) = archives
            .into_iter()
            .partition(|archive| links.contains_key(&archive.id));
        let platform_projection = project_platforms(&current);
        let mut games: Vec<_> = current.into_iter().map(Game::from_archive).collect();
        games.sort_by_cached_key(|game| (game.title.to_lowercase(), game.archive.id));
        let mut historical: Vec<_> = old.into_iter().map(Game::from_archive).collect();
        historical.sort_by_cached_key(|game| game.archive.id);
        let mut library = Self {
            games,
            historical,
            ..Self::default()
        };
        library.platforms = platform_projection
            .into_iter()
            .map(|(platform, projection)| (platform, projection.total))
            .collect();
        for (index, game) in library.games.iter().enumerate() {
            library.by_id.insert(game.archive.id, index);
            let root = game
                .archive
                .absolute_path
                .ancestors()
                .nth(game.archive.relative_path.components().count())
                .unwrap_or(&game.archive.absolute_path)
                .to_path_buf();
            *library
                .platform_sources
                .entry(game.platform.clone())
                .or_default()
                .entry((game.archive.source_folder_id, root))
                .or_default() += 1;
            library.attention += usize::from(game.attention);
        }
        for (index, game) in library.historical.iter().enumerate() {
            if let Some(link) = links.get(&game.archive.id) {
                library.historical_links.insert(
                    game.archive.id,
                    HistoricalLink {
                        current_id: link.current_id,
                        evidence: link.evidence,
                    },
                );
                library
                    .historical_of
                    .entry(link.current_id)
                    .or_default()
                    .push(index);
            }
        }
        library
    }

    /// The id to show for a catalogue id: an old row's id becomes its current
    /// row's id, every other id is unchanged.
    pub fn resolve(&self, id: i64) -> i64 {
        self.historical_links
            .get(&id)
            .map_or(id, |link| link.current_id)
    }

    /// Older-location rows that a current game replaces, for Advanced details.
    pub fn historical_for(&self, current_id: i64) -> impl Iterator<Item = &Game> {
        self.historical_of
            .get(&current_id)
            .into_iter()
            .flatten()
            .filter_map(|index| self.historical.get(*index))
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
                    && (!filter.unverified_only || !game.identified)
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
    pub unverified_only: bool,
    pub list: bool,
}

impl Filter {
    /// Entering a platform is a fresh browse intent, not an old problem/search view.
    pub fn select_platform(&mut self, platform: String) {
        self.platform = platform;
        self.search.clear();
        self.attention_only = false;
        self.unverified_only = false;
    }
}

pub(super) type SharedLibrary = Arc<Library>;

#[cfg(test)]
mod tests {
    use super::{Game, canonical_platform_name};
    use archivefs_core::PersistedArchive;

    #[test]
    fn scummvm_aliases_project_to_the_canonical_selector_identity() {
        for alias in ["scumm", "scummvm", "sci", "sierrasci"] {
            assert_eq!(canonical_platform_name(Some(alias)), "ScummVM");
        }
    }

    #[test]
    fn dos_does_not_project_as_scummvm() {
        assert_eq!(canonical_platform_name(Some("DOS")), "DOS");
    }

    #[test]
    fn identity_summary_does_not_promote_missing_evidence_to_verified() {
        let archive = PersistedArchive {
            id: 7,
            source_folder_id: 1,
            relative_path: "unknown.zip".into(),
            absolute_path: "/fixture/unknown.zip".into(),
            archive_kind: "zip".into(),
            display_name: "Unknown".into(),
            normalized_name: "unknown".into(),
            size_bytes: Some(1),
            modified_time_unix_seconds: Some(1),
            platform: Some("SNES".into()),
            platform_source: Some("fixture".into()),
            last_known_health: "pending".into(),
            last_seen_at: "fixture".into(),
            last_verified_missing_at: None,
            identity_report: None,
        };
        assert_eq!(Game::from_archive(archive).identity_summary(), "Unknown");
    }
}

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
