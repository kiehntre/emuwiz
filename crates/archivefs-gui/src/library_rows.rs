//! The Library row model.
//!
//! What a Library row is (`ArchiveRow`), where it came from (`RowOrigin`),
//! how the live scan and the persisted catalogue merge into one display list
//! (`build_display_rows`), and which of those rows a search and the platform
//! and status filters select (`LibraryRowFilters`, `matching_row_indices`).
//!
//! Rendering lives in `library_view`, session state in `library_ui_state`;
//! this module is only the data model both of them agree on, and the one the
//! Gamer View rail, the Cheats & Mods picker and the identity pages reuse.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use archivefs_core::{
    ArchiveRecord, ArchiveSnapshot, ArchiveStats, ArchiveStatus, ConfigIdentity, DoctorReport,
    PersistedArchive,
};
use eframe::egui;

use crate::{
    CachedLibrarySnapshot, DatabaseGeneration, RefreshGeneration,
    persisted_archive_has_unknown_platform,
};

pub(crate) struct LoadedData {
    pub(crate) mount_root: PathBuf,
    pub(crate) records: Vec<ArchiveRecord>,
    pub(crate) rows: Vec<ArchiveRow>,
    pub(crate) stats: ArchiveStats,
    pub(crate) doctor: DoctorReport,
    pub(crate) config_identity: ConfigIdentity,
}

impl LoadedData {
    pub(crate) fn from_snapshot(snapshot: ArchiveSnapshot) -> Self {
        let rows = snapshot
            .records
            .iter()
            .zip(&snapshot.statuses)
            .map(|(record, status)| ArchiveRow::new(record, status))
            .collect();

        Self {
            mount_root: snapshot.mount_root,
            records: snapshot.records,
            rows,
            stats: snapshot.stats,
            doctor: snapshot.doctor,
            config_identity: snapshot.config_identity,
        }
    }
}

/// Where a displayed row's data came from - see requirement 4. Only `Live`
/// rows carry a path that `selected_record`/`selected_record_index` can
/// ever match against `LoadedData.records`, since those come from the
/// cache's `PersistedArchive.absolute_path`, never from a live
/// `ArchiveRecord` - this is what guarantees a cache-only selection can
/// never resolve to a live record and so can never expose an action
/// button (see `show_selected_archive`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowOrigin {
    /// Backed by the latest coherent live snapshot. Actions are available,
    /// subject to `latest_generation_actions_safe`.
    Live,
    /// Known to the persisted catalogue, not (yet) confirmed by the live
    /// snapshot, and not marked missing by the last scan.
    CachedAwaitingValidation,
    /// Known to the persisted catalogue and marked missing
    /// (`last_verified_missing_at` set) as of the last completed scan.
    CachedMissing,
    /// Known to the persisted catalogue, not marked missing by the last
    /// scan, but its path is not reachable right now (a cheap existence
    /// check at merge time, for display only - never used to authorize
    /// mount/unmount).
    CachedUnavailable,
}

impl RowOrigin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::CachedAwaitingValidation => "Cached: awaiting validation",
            Self::CachedMissing => "Cached: missing",
            Self::CachedUnavailable => "Cached: source unavailable",
        }
    }

    /// Gamer View's own wording for the same states `label()` describes
    /// precisely for Advanced View - "Cached" is internal snapshot/database
    /// vocabulary (docs/GUI_NAVIGATION_RESET_DESIGN.md §2.6's banned-word
    /// spirit, even though "Cached" itself isn't verbatim on that list).
    /// `Live` never reaches this - the one call site only shows a line at
    /// all when `origin != Live`.
    pub(crate) fn gamer_view_label(self) -> &'static str {
        match self {
            Self::Live => "",
            Self::CachedAwaitingValidation => "Checking this game...",
            Self::CachedMissing => "We couldn't find this game's file just now.",
            Self::CachedUnavailable => "We can't reach this game's location right now.",
        }
    }
}

#[derive(Clone)]
pub(crate) struct ArchiveRow {
    /// Exact-byte identity used for selection and reconciliation - never
    /// rendered directly, and never compared via `.display()` (see
    /// requirement 5). For a live row this is
    /// `ArchiveRecord.mount_plan.archive.path`; for a cache-only row it is
    /// `PersistedArchive.absolute_path` - the same pairing the database's
    /// own `(source_folder_id, relative_path)` uniqueness constraint
    /// already encodes.
    pub(crate) path: PathBuf,
    pub(crate) archive_path: String,
    pub(crate) mount_path: String,
    pub(crate) platform: String,
    pub(crate) state: String,
    pub(crate) search_text: String,
    pub(crate) origin: RowOrigin,
    pub(crate) unknown_platform: bool,
    pub(crate) source_path: Option<PathBuf>,
}

impl ArchiveRow {
    pub(crate) fn new(record: &ArchiveRecord, status: &ArchiveStatus) -> Self {
        let archive_path = status.archive_path.display().to_string();
        let mount_path = status.mount_path.display().to_string();
        let raw_platform = record
            .metadata
            .platform
            .as_deref()
            .or(record.identity.platform.as_deref());
        let unknown_platform = raw_platform.is_none();
        let platform = raw_platform.unwrap_or("Unknown").to_string();
        let state = status.state.to_string();
        let search_text =
            format!("{archive_path}\n{mount_path}\n{platform}\n{state}").to_lowercase();

        Self {
            path: record.mount_plan.archive.path.clone(),
            archive_path,
            mount_path,
            platform,
            state,
            search_text,
            origin: RowOrigin::Live,
            unknown_platform,
            source_path: None,
        }
    }

    /// Synthesizes a display-only row for a cache-only archive: one the
    /// persisted catalogue knows about but the latest live snapshot does
    /// not confirm. `path_exists` is a cheap, display-only existence
    /// check (never a substitute for live validation) that distinguishes
    /// "unreachable right now" from "awaiting the next live refresh".
    pub(crate) fn from_cached(persisted: &PersistedArchive, path_exists: bool) -> Self {
        let archive_path = persisted.absolute_path.display().to_string();
        let unknown_platform = persisted_archive_has_unknown_platform(persisted);
        let platform = persisted
            .platform
            .as_deref()
            .unwrap_or("Unknown")
            .to_string();
        let origin = if persisted.last_verified_missing_at.is_some() {
            RowOrigin::CachedMissing
        } else if !path_exists {
            RowOrigin::CachedUnavailable
        } else {
            RowOrigin::CachedAwaitingValidation
        };
        let state = origin.label().to_string();
        let mount_path = String::new();
        let search_text =
            format!("{archive_path}\n{mount_path}\n{platform}\n{state}").to_lowercase();

        Self {
            path: persisted.absolute_path.clone(),
            archive_path,
            mount_path,
            platform,
            state,
            search_text,
            origin,
            unknown_platform,
            source_path: None,
        }
    }

    /// Overrides this row's platform-derived fields (`platform`,
    /// `unknown_platform`, and the platform portion of `search_text`)
    /// with the library database's effective (manual-aware) platform for
    /// this archive.
    ///
    /// A live row built from `ArchiveRecord` alone only ever sees the
    /// live scan's own automatic detection
    /// (`record.metadata.platform`/`record.identity.platform`), which
    /// disagrees with the persisted effective platform exactly when a
    /// manual assignment is active and automatic detection found
    /// nothing. Without this override, such a row would be wrongly
    /// classified (and counted/filtered) as unknown. Only ever applied
    /// when the database already has a persisted row for this exact path
    /// (see `build_display_rows`); a live row with no persisted
    /// counterpart yet keeps its live-only classification, the only
    /// signal available for it.
    pub(crate) fn with_persisted_platform(mut self, persisted: &PersistedArchive) -> Self {
        self.unknown_platform = persisted_archive_has_unknown_platform(persisted);
        self.platform = persisted
            .platform
            .as_deref()
            .unwrap_or("Unknown")
            .to_string();
        self.search_text = format!(
            "{}\n{}\n{}\n{}",
            self.archive_path, self.mount_path, self.platform, self.state
        )
        .to_lowercase();
        self
    }
    pub(crate) fn with_source_path(mut self, source_path: Option<PathBuf>) -> Self {
        self.source_path = source_path;
        self
    }

    pub(crate) fn matches(&self, normalized_filter: &str) -> bool {
        self.search_text.contains(normalized_filter)
    }

    pub(crate) fn row_text_color(&self, visuals: &egui::Visuals) -> Option<egui::Color32> {
        match self.origin {
            RowOrigin::Live => None,
            RowOrigin::CachedAwaitingValidation => Some(egui::Color32::from_rgb(150, 150, 150)),
            RowOrigin::CachedMissing => Some(visuals.error_fg_color),
            RowOrigin::CachedUnavailable => Some(egui::Color32::from_rgb(210, 140, 40)),
        }
    }
}

/// Merges live rows with cache-only rows for display - see requirement 4
/// and 5. Live rows always win: a cached archive whose exact path already
/// appears among `records` is represented only by its live row, never
/// duplicated - but with its platform/unknown-platform classification
/// overridden from the persisted effective value when the database
/// already has an entry for it (see `ArchiveRow::with_persisted_platform`
/// and requirement 6). Recomputed fresh whenever the underlying live or
/// cached data changes (see `ArchiveFsApp::recompute_filtered_rows`), not
/// on every frame, so it stays cheap without risking a stale merge.
pub(crate) fn build_display_rows(
    records: &[ArchiveRecord],
    live_rows: &[ArchiveRow],
    cached: Option<&CachedLibrarySnapshot>,
) -> Vec<ArchiveRow> {
    let persisted_by_path: HashMap<&Path, &PersistedArchive> = cached
        .map(|cached| {
            cached
                .archives
                .iter()
                .map(|persisted| (persisted.absolute_path.as_path(), persisted))
                .collect()
        })
        .unwrap_or_default();

    // Resolves one row's owning source: by exact database id when a
    // persisted counterpart names one (the reliable case - see
    // `PersistedArchive::source_folder_id`), otherwise by the longest
    // configured source path that is a prefix of the archive's absolute
    // path (the only signal available for a brand new live-only row never
    // yet persisted). Longest-first so a source nested inside another
    // configured source's path never wrongly claims ownership of the
    // outer source's own direct children.
    let source_path_by_id: HashMap<i64, &Path> = cached
        .map(|cached| {
            cached
                .source_views
                .iter()
                .filter_map(|view| view.id.map(|id| (id, view.path.as_path())))
                .collect()
        })
        .unwrap_or_default();
    let mut source_paths_longest_first: Vec<&Path> = cached
        .map(|cached| cached.source_views.iter().map(|view| view.path.as_path()))
        .into_iter()
        .flatten()
        .collect();
    source_paths_longest_first.sort_by_key(|path| std::cmp::Reverse(path.as_os_str().len()));
    let resolve_source_path = |row_path: &Path, persisted: Option<&PersistedArchive>| {
        if let Some(path) = persisted
            .and_then(|persisted| source_path_by_id.get(&persisted.source_folder_id).copied())
        {
            return Some(path.to_path_buf());
        }
        source_paths_longest_first
            .iter()
            .find(|source_path| row_path.starts_with(source_path))
            .map(|path| path.to_path_buf())
    };

    let mut merged: Vec<ArchiveRow> = live_rows
        .iter()
        .cloned()
        .map(|row| {
            let persisted = persisted_by_path.get(row.path.as_path()).copied();
            let source_path = resolve_source_path(&row.path, persisted);
            let row = match persisted {
                Some(persisted) => row.with_persisted_platform(persisted),
                None => row,
            };
            row.with_source_path(source_path)
        })
        .collect();

    if let Some(cached) = cached {
        let live_paths: HashSet<&Path> = records
            .iter()
            .map(|record| record.mount_plan.archive.path.as_path())
            .collect();
        for persisted in &cached.archives {
            if live_paths.contains(persisted.absolute_path.as_path()) {
                continue;
            }
            let path_exists = persisted.absolute_path.exists();
            let source_path = resolve_source_path(&persisted.absolute_path, Some(persisted));
            merged.push(
                ArchiveRow::from_cached(persisted, path_exists).with_source_path(source_path),
            );
        }
    }

    merged
}

/// Optional search filters over the merged row list (requirement 6). Two
/// independent groups - state and platform - each AND'd together; within
/// a group, an unchecked filter set imposes no restriction (defaults to
/// "show everything") and multiple checked filters within the same group
/// are OR'd, so checking both `present` and `missing` shows both rather
/// than nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LibraryRowFilters {
    pub(crate) present: bool,
    pub(crate) missing: bool,
    pub(crate) awaiting_validation: bool,
    pub(crate) known_platform: bool,
    pub(crate) unknown_platform: bool,
    /// Session-wide platform-first selection shared with Mount and the
    /// Cheats & Mods chooser. `Some("Unknown")` is diagnostic, not empty.
    pub(crate) platform: Option<String>,
}

impl LibraryRowFilters {
    pub(crate) fn is_active(&self) -> bool {
        self.present
            || self.missing
            || self.awaiting_validation
            || self.known_platform
            || self.unknown_platform
            || self.platform.is_some()
    }

    pub(crate) fn matches(&self, row: &ArchiveRow) -> bool {
        let state_group_active = self.present || self.missing || self.awaiting_validation;
        let state_match = !state_group_active || {
            let is_present = matches!(row.origin, RowOrigin::Live);
            let is_missing = matches!(row.origin, RowOrigin::CachedMissing);
            let is_awaiting = matches!(
                row.origin,
                RowOrigin::CachedAwaitingValidation | RowOrigin::CachedUnavailable
            );
            (self.present && is_present)
                || (self.missing && is_missing)
                || (self.awaiting_validation && is_awaiting)
        };

        let platform_group_active = self.known_platform || self.unknown_platform;
        let platform_match = !platform_group_active
            || (self.known_platform && !row.unknown_platform)
            || (self.unknown_platform && row.unknown_platform);

        let selected_platform_match = self.platform.as_deref().is_none_or(|wanted| {
            if wanted == "Unknown" {
                row.unknown_platform
            } else {
                !row.unknown_platform && row.platform == wanted
            }
        });

        state_match && platform_match && selected_platform_match
    }
}

pub(crate) fn matching_row_indices(rows: &[ArchiveRow], filter: &str) -> Option<Vec<usize>> {
    let normalized_filter = filter.trim().to_lowercase();
    if normalized_filter.is_empty() {
        return None;
    }

    Some(
        rows.iter()
            .enumerate()
            .filter_map(|(index, row)| row.matches(&normalized_filter).then_some(index))
            .collect(),
    )
}

/// Identifies the exact inputs [`build_display_rows`] reads, so the
/// merged projection can be reused across frames instead of rebuilt for
/// every repaint of the Library / Recently Found / Home renderer.
///
/// Pointer identity plus generations, following the precedent
/// `HealthReportCacheKey` already sets in this crate. `LoadedData` and
/// `CachedLibrarySnapshot` are only ever *replaced* wholesale
/// (`live_library_controller::poll_load` and
/// `database_load::poll_database_load` both assign a freshly boxed
/// value; nothing mutates either in place), so a pointer change is a
/// reliable "this was reloaded" signal. The generations are carried as
/// well because a new box can land on the address the previous one just
/// freed: every reload that could do that bumps `refresh_generation`,
/// `snapshot_generation` or `database_generation` first, so the pair can
/// never both repeat.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct MergedDisplayRowsKey {
    pub(crate) live_data_ptr: usize,
    pub(crate) database_snapshot_ptr: Option<usize>,
    pub(crate) refresh_generation: RefreshGeneration,
    pub(crate) snapshot_generation: Option<RefreshGeneration>,
    pub(crate) database_generation: DatabaseGeneration,
}

/// The merged live+cache row list [`build_display_rows`] produced for
/// [`MergedDisplayRowsKey`].
pub(crate) struct MergedDisplayRowsCache {
    key: MergedDisplayRowsKey,
    rows: Vec<ArchiveRow>,
}

impl MergedDisplayRowsCache {
    #[cfg(test)]
    pub(crate) fn rows(&self) -> &[ArchiveRow] {
        &self.rows
    }
}

/// Returns the merged display rows for `key`, rebuilding them only when
/// the live snapshot or the database snapshot has actually been replaced
/// since the last call.
///
/// Filters, sorting and selection are deliberately *not* part of the key:
/// they are applied downstream, per frame, to the indices this list is
/// addressed by (`matching_row_indices`, `LibraryRowFilters::matches`,
/// `sort_visible_indices`), so a filter or sort change still takes effect
/// on the very next frame without invalidating anything here.
///
/// One consequence is deliberate: the `path_exists` probe
/// `ArchiveRow::from_cached` uses to tell `CachedUnavailable` from
/// `CachedAwaitingValidation` is now sampled once per reload rather than
/// once per repaint. Both are "the live scan has not confirmed this file"
/// states - the authoritative missing flag comes from the database's
/// `last_verified_missing_at`, not from this probe - and the live rows
/// beside them are already a snapshot, so sampling it with them makes the
/// page internally consistent instead of mixing two freshnesses.
pub(crate) fn cached_display_rows<'cache>(
    cache: &'cache mut Option<MergedDisplayRowsCache>,
    key: MergedDisplayRowsKey,
    records: &[ArchiveRecord],
    live_rows: &[ArchiveRow],
    cached: Option<&CachedLibrarySnapshot>,
) -> &'cache [ArchiveRow] {
    if !cache.as_ref().is_some_and(|entry| entry.key == key) {
        *cache = Some(MergedDisplayRowsCache {
            key,
            rows: build_display_rows(records, live_rows, cached),
        });
    }
    &cache
        .as_ref()
        .expect("the cache was just populated for this key")
        .rows
}
