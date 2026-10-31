//! Lightweight catalogue projection. Never turns filename hints into identity.
use archivefs_core::{
    ArchiveKind, PersistedArchive,
    launch::{CanonicalIdentityStatus, EvidenceFreshness, canonical_identity_for_saved_report},
};

pub(super) fn media_kind_label(storage_name: &str) -> &'static str {
    match ArchiveKind::from_storage(storage_name) {
        Some(ArchiveKind::Zip) => "ZIP",
        Some(ArchiveKind::SevenZip) => "7z",
        Some(ArchiveKind::Rar) => "RAR",
        Some(ArchiveKind::Lha) => "LHA",
        Some(ArchiveKind::MegaDriveRom) => "Mega Drive ROM",
        Some(ArchiveKind::DirectGameImage) => "Game image",
        Some(ArchiveKind::ArcadeSetDirectory) => "Arcade set",
        Some(ArchiveKind::ScummVmDirectory) => "ScummVM game folder",
        Some(ArchiveKind::PreservationRecording) => "Spectrum gameplay recording",
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
/// Status for preservation-only media; Play is never offered for it.
pub(crate) const PRESERVATION_ONLY_STATUS: &str = "Preservation-only · not offered for play";

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
    /// The last scan could not finish reading this game's file, so its saved
    /// identity report is history, not current evidence, until a rescan.
    pub inspection_incomplete: bool,
    /// A saved DAT audit found exactly one exact hash match for this game.
    /// The inner label is the DAT's ecosystem when the saved row records one.
    pub dat_exact: Option<Option<&'static str>>,
    /// The audited hashes behind that exact match, from the same current
    /// evidence. Only set alongside `dat_exact`.
    pub dat_exact_hashes: Option<archivefs_core::dat::library_identity_summary::LibraryItemHashes>,
    /// The strongest other saved DAT evidence, when any applies (a negative
    /// only from this game's own platform's catalogue). `None`: not checked.
    pub dat_state: Option<archivefs_core::LibraryDatState>,
    pub attention: bool,
    pub screenscraper:
        Option<archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment>,
    pub(super) search: String,
}

impl Game {
    pub fn from_archive(archive: PersistedArchive) -> Self {
        let title =
            crate::media_sets_page::title_without_cue_suffix(&archive.display_name).to_string();
        let platform = canonical_platform_name(archive.platform.as_deref());
        let freshness = EvidenceFreshness::from_stored_health(&archive.last_known_health);
        let identified = archive.identity_report.as_ref().is_some_and(|report| {
            matches!(
                canonical_identity_for_saved_report(report, freshness).0,
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
            inspection_incomplete: freshness.is_incomplete(),
            dat_exact: None,
            dat_exact_hashes: None,
            dat_state: None,
            attention,
            screenscraper: None,
            search,
        }
    }
    /// A preservation-only record (an RZX gameplay recording): shown and
    /// kept, never offered for play.
    pub fn is_preservation_only(&self) -> bool {
        ArchiveKind::from_storage(&self.archive.archive_kind)
            .is_some_and(ArchiveKind::is_preservation_only)
    }

    pub fn status(&self) -> &'static str {
        if self.is_preservation_only() {
            PRESERVATION_ONLY_STATUS
        } else if self.attention {
            "Needs attention"
        } else if self.inspection_incomplete && self.dat_exact.is_none() {
            "Rescan to finish checking"
        } else if self.identity_summary() == "Mismatch" {
            "Identity conflict · review evidence"
        } else if self.identified {
            "Identified · not yet checked for play"
        } else if self.platform != UNKNOWN_PLATFORM {
            self.identity_summary()
        } else {
            "Unknown platform · More evidence needed"
        }
    }

    /// Records a saved exact DAT match. It makes the game identified with no
    /// confirmation step, whatever a scan-time report said.
    pub fn mark_dat_exact(&mut self, ecosystem: Option<&'static str>) {
        self.identified = true;
        self.dat_exact = Some(ecosystem);
    }

    /// "Exact No-Intro match" (or "Exact DAT match" when the saved row does
    /// not record which catalogue it came from).
    pub fn dat_exact_label(&self) -> Option<String> {
        self.dat_exact
            .map(|ecosystem| format!("Exact {} match", ecosystem.unwrap_or("DAT")))
    }

    /// DAT verification is independent of scan-time identification and launch readiness.
    pub fn dat_identity_summary(&self) -> &'static str {
        if self.dat_exact.is_some() {
            return "Verified";
        }
        use archivefs_core::LibraryDatState;
        match self.dat_state {
            Some(LibraryDatState::Verified) | Some(LibraryDatState::Probable) => "Probable match",
            Some(LibraryDatState::Ambiguous) => "Ambiguous match",
            Some(LibraryDatState::NoMatch) => "No catalogue match",
            None => "Not checked",
        }
    }

    pub fn identity_summary(&self) -> &'static str {
        if !self.inspection_incomplete
            && self.archive.identity_report.as_ref().is_some_and(|report| {
                matches!(
                    canonical_identity_for_saved_report(report, EvidenceFreshness::Current).0,
                    CanonicalIdentityStatus::Conflicting
                )
            })
        {
            return "Mismatch";
        }
        if self.dat_exact.is_some() || self.dat_state.is_some() {
            if self.dat_exact.is_none()
                && self.dat_state == Some(archivefs_core::LibraryDatState::NoMatch)
                && self.platform != UNKNOWN_PLATFORM
            {
                return "Platform recognised · Catalogue match unavailable";
            }
            return self.dat_identity_summary();
        }
        let Some(report) = self.archive.identity_report.as_ref() else {
            return if self.platform == UNKNOWN_PLATFORM {
                "Unknown platform"
            } else {
                "Platform recognised · Game not yet identified"
            };
        };
        let freshness = if self.inspection_incomplete {
            EvidenceFreshness::InspectionIncomplete
        } else {
            EvidenceFreshness::Current
        };
        if freshness.is_incomplete() {
            return "Rescan needed";
        }
        match canonical_identity_for_saved_report(report, freshness).0 {
            CanonicalIdentityStatus::Resolved(_) => "Verified",
            CanonicalIdentityStatus::Conflicting => "Mismatch",
            CanonicalIdentityStatus::Unknown if self.platform != UNKNOWN_PLATFORM => {
                "Platform recognised · Game not yet identified"
            }
            CanonicalIdentityStatus::Unknown => "Unknown platform",
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
    pub(super) by_platform: BTreeMap<String, Vec<usize>>,
    pub(super) title_twins: std::collections::HashSet<i64>,
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
        let mut first_title = HashMap::new();
        for (index, game) in library.games.iter().enumerate() {
            library
                .by_platform
                .entry(game.platform.clone())
                .or_default()
                .push(index);
            if let Some(other) = first_title.insert(game.title.as_str(), game.archive.id)
                && other != game.archive.id
            {
                library.title_twins.insert(other);
                library.title_twins.insert(game.archive.id);
            }
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
    pub(super) fn has_title_twin(&self, id: i64) -> bool {
        self.title_twins.contains(&id)
    }
    pub fn filter(&self, filter: &Filter) -> Vec<usize> {
        let needle = filter.search.trim().to_lowercase();
        let matches = |game: &Game| {
            filter.admits_before_check(game)
                && (!filter.unverified_only || !game.identified)
                && (needle.is_empty() || game.search.contains(&needle))
        };
        if filter.platform.is_empty() {
            self.games
                .iter()
                .enumerate()
                .filter(|(_, game)| matches(game))
                .map(|(index, _)| index)
                .collect()
        } else {
            self.by_platform
                .get(&filter.platform)
                .into_iter()
                .flatten()
                .copied()
                .filter(|index| matches(&self.games[*index]))
                .collect()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Filter {
    pub search: String,
    pub platform: String,
    pub readiness: ReadinessFilter,
    /// Blue's Browse & Play also folds finished launch checks into the
    /// readiness views; the classic Library does not.
    #[serde(skip)]
    pub cache_aware: bool,
    pub unverified_only: bool,
    pub list: bool,
}

/// The Library's readiness views. Ready is answered only from checks that
/// have already finished; nothing is planned in bulk to fill it in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum ReadinessFilter {
    #[default]
    All,
    Ready,
    NeedsAttention,
    NotChecked,
}

impl ReadinessFilter {
    pub const ALL: [Self; 4] = [
        Self::All,
        Self::Ready,
        Self::NeedsAttention,
        Self::NotChecked,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Ready => "Ready",
            Self::NeedsAttention => "Needs attention",
            Self::NotChecked => "Not checked",
        }
    }
}

impl Filter {
    /// Entering a platform is a fresh browse intent, not an old problem/search view.
    pub fn select_platform(&mut self, platform: String) {
        self.platform = platform;
        self.search.clear();
        self.readiness = ReadinessFilter::All;
        self.unverified_only = false;
    }

    /// Whether the catalogue-only part of the filter keeps `game`. Views that
    /// read the readiness cache are a superset here; `matches` is final.
    fn admits_before_check(&self, game: &Game) -> bool {
        match self.readiness {
            ReadinessFilter::All => true,
            ReadinessFilter::NeedsAttention => game.attention || self.cache_aware,
            ReadinessFilter::Ready | ReadinessFilter::NotChecked => !game.attention,
        }
    }

    /// True when the final list also depends on the readiness cache.
    pub fn uses_cache(&self) -> bool {
        match self.readiness {
            ReadinessFilter::All => false,
            ReadinessFilter::NeedsAttention => self.cache_aware,
            ReadinessFilter::Ready | ReadinessFilter::NotChecked => true,
        }
    }
}

/// What a finished launch check said. Unknown is never stored: a game with no
/// entry is Not checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckedReadiness {
    Ready,
    /// A finished negative result, with the card's own label for the reason.
    NotReady(&'static str),
}

impl CheckedReadiness {
    /// The finished answer on a compact readiness card, or `None` when the
    /// card is still checking, stale or partial (unknown is never a result).
    pub fn from_compact(view: &super::launch_readiness_summary::CompactReadiness) -> Option<Self> {
        use super::launch_readiness_summary::ReadinessPresentationState as State;
        if view.pending || view.spinner {
            return None;
        }
        Some(match view.tone {
            State::Ready | State::ReadyWithWarnings => Self::Ready,
            _ => Self::NotReady(view.badge),
        })
    }
}

/// The file facts a result was taken against. A result whose facts no longer
/// match the catalogue row describes an older file and is ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    size: Option<u64>,
    modified: Option<i64>,
    health: String,
}

impl Fingerprint {
    fn of(game: &Game) -> Self {
        Self {
            size: game.archive.size_bytes,
            modified: game.archive.modified_time_unix_seconds,
            health: game.archive.last_known_health.clone(),
        }
    }
}

/// Finished per-game launch-check results, kept while browsing.
#[derive(Clone, Debug, Default)]
pub(super) struct ReadinessCache {
    entries: std::collections::HashMap<i64, (CheckedReadiness, Fingerprint)>,
    version: u64,
}

impl ReadinessCache {
    /// Record a finished answer for `game`, or forget it with `None` when the
    /// latest answer is unknown (checking, stale, partial).
    pub fn record(&mut self, game: &Game, result: Option<CheckedReadiness>) {
        let id = game.archive.id;
        let entry = result.map(|result| (result, Fingerprint::of(game)));
        if self.entries.get(&id) == entry.as_ref() {
            return;
        }
        match entry {
            Some(entry) => self.entries.insert(id, entry),
            None => self.entries.remove(&id),
        };
        self.version += 1;
    }
    /// Record a background answer only where no current answer exists, so a
    /// background check never replaces a foreground one.
    pub fn record_background(&mut self, game: &Game, result: CheckedReadiness) {
        if self.get(game).is_none() {
            self.record(game, Some(result));
        }
    }
    pub fn clear(&mut self) {
        if !self.entries.is_empty() {
            self.entries.clear();
            self.version += 1;
        }
    }
    /// The stored answer, only while it still matches the catalogue row.
    pub fn get(&self, game: &Game) -> Option<CheckedReadiness> {
        self.entries
            .get(&game.archive.id)
            .filter(|(_, print)| *print == Fingerprint::of(game))
            .map(|(result, _)| *result)
    }
    /// The reason label of a current Not ready result for a game the catalogue
    /// does not already flag.
    pub fn not_ready_reason(&self, game: &Game) -> Option<&'static str> {
        match self.get(game) {
            Some(CheckedReadiness::NotReady(reason)) if !game.attention => Some(reason),
            _ => None,
        }
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The four-way readiness view. A catalogue problem is always Needs
/// attention. Otherwise a cached Ready is Ready, a cached Not ready is Needs
/// attention, and a game with no current answer is Not checked.
pub(super) fn matches(game: &Game, filter: &Filter, cache: &ReadinessCache) -> bool {
    let checked = if game.attention {
        None
    } else {
        cache.get(game)
    };
    match filter.readiness {
        ReadinessFilter::All => true,
        ReadinessFilter::NeedsAttention => {
            game.attention
                || (filter.cache_aware && matches!(checked, Some(CheckedReadiness::NotReady(_))))
        }
        ReadinessFilter::Ready => checked == Some(CheckedReadiness::Ready),
        ReadinessFilter::NotChecked => !game.attention && checked.is_none(),
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
        let game = Game::from_archive(archive);
        assert_eq!(
            game.identity_summary(),
            "Platform recognised · Game not yet identified"
        );
        assert_eq!(
            game.status(),
            "Platform recognised · Game not yet identified"
        );
    }

    fn game(id: i64, health: &str) -> Game {
        Game::from_archive(PersistedArchive {
            id,
            source_folder_id: 1,
            relative_path: format!("{id}.zip").into(),
            absolute_path: format!("/fixture/{id}.zip").into(),
            archive_kind: "zip".into(),
            display_name: format!("Game {id}"),
            normalized_name: format!("game {id}"),
            size_bytes: Some(1),
            modified_time_unix_seconds: Some(1),
            platform: Some("SNES".into()),
            platform_source: Some("fixture".into()),
            last_known_health: health.into(),
            last_seen_at: "fixture".into(),
            last_verified_missing_at: None,
            identity_report: None,
        })
    }

    fn filter(readiness: super::ReadinessFilter) -> super::Filter {
        super::Filter {
            readiness,
            cache_aware: true,
            ..Default::default()
        }
    }

    #[test]
    fn readiness_views_partition_games_by_finished_results() {
        use super::{CheckedReadiness as C, ReadinessCache, ReadinessFilter as F, matches};
        let games = [
            game(1, "ok"),
            game(2, "ok"),
            game(3, "missing"),
            game(4, "ok"),
            game(5, "ok"),
        ];
        let mut cache = ReadinessCache::default();
        cache.record(&games[0], Some(C::Ready));
        // A cached Ready never hides a catalogue problem.
        cache.record(&games[2], Some(C::Ready));
        cache.record(&games[3], Some(C::NotReady("Needs emulator")));
        let hits = |readiness| -> Vec<i64> {
            games
                .iter()
                .filter(|g| matches(g, &filter(readiness), &cache))
                .map(|g| g.archive.id)
                .collect()
        };
        assert_eq!(hits(F::All), [1, 2, 3, 4, 5]);
        assert_eq!(hits(F::Ready), [1]);
        assert_eq!(hits(F::NeedsAttention), [3, 4]);
        assert_eq!(hits(F::NotChecked), [2, 5]);
        for g in &games {
            let n = [F::Ready, F::NeedsAttention, F::NotChecked]
                .into_iter()
                .filter(|f| matches(g, &filter(*f), &cache))
                .count();
            assert_eq!(n, 1, "game {}", g.archive.id);
        }
    }

    #[test]
    fn selection_switching_keeps_earlier_results_and_unknown_forgets() {
        use super::{CheckedReadiness as C, ReadinessCache};
        let (a, b) = (game(1, "ok"), game(2, "ok"));
        let mut cache = ReadinessCache::default();
        cache.record(&a, Some(C::Ready));
        cache.record(&b, Some(C::NotReady("Needs emulator")));
        assert_eq!(cache.get(&a), Some(C::Ready));
        assert_eq!(cache.get(&b), Some(C::NotReady("Needs emulator")));
        let version = cache.version();
        cache.record(&a, Some(C::Ready));
        assert_eq!(cache.version(), version, "same answer is not a change");
        // The latest answer is unknown (checking/stale): never a stale success.
        cache.record(&a, None);
        assert_eq!(cache.get(&a), None);
        assert_eq!(cache.get(&b), Some(C::NotReady("Needs emulator")));
    }

    #[test]
    fn a_changed_file_invalidates_its_cached_result() {
        use super::{CheckedReadiness as C, ReadinessCache};
        let mut g = game(1, "ok");
        let mut cache = ReadinessCache::default();
        cache.record(&g, Some(C::Ready));
        assert_eq!(cache.get(&g), Some(C::Ready));
        g.archive.modified_time_unix_seconds = Some(2);
        assert_eq!(cache.get(&g), None, "newer file");
        g.archive.modified_time_unix_seconds = Some(1);
        g.archive.last_known_health = "corrupt".into();
        assert_eq!(cache.get(&g), None, "health changed");
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn library_filter_is_a_superset_for_cache_views_and_exact_for_classic() {
        use super::{Filter, Library, ReadinessFilter as F};
        let library = Library::new(vec![game(1, "ok").archive, game(2, "missing").archive]);
        let n = |readiness, cache_aware| {
            library
                .filter(&Filter {
                    readiness,
                    cache_aware,
                    ..Default::default()
                })
                .len()
        };
        assert_eq!(n(F::All, false), 2);
        assert_eq!(n(F::NeedsAttention, false), 1, "classic stays exact");
        assert_eq!(n(F::NeedsAttention, true), 2);
        assert_eq!(n(F::Ready, true), 1);
        assert_eq!(n(F::NotChecked, true), 1);
    }

    #[test]
    fn a_large_catalogue_filters_without_per_game_work_beyond_a_lookup() {
        use super::{CheckedReadiness as C, ReadinessCache, ReadinessFilter as F, matches};
        let games: Vec<Game> = (0..20_000).map(|i| game(i, "ok")).collect();
        let mut cache = ReadinessCache::default();
        for g in games.iter().step_by(100) {
            cache.record(g, Some(C::Ready));
        }
        let ready = games
            .iter()
            .filter(|g| matches(g, &filter(F::Ready), &cache))
            .count();
        let unchecked = games
            .iter()
            .filter(|g| matches(g, &filter(F::NotChecked), &cache))
            .count();
        assert_eq!((ready, unchecked), (200, 19_800));
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
