use std::path::PathBuf;

use crate::{
    ArchiveFsApp, DuplicateGroupIdentity, DuplicateReviewFilters, DuplicateSortField,
    HealthDashboardFilters, HealthIssue, HealthReportCache, HealthReportCacheKey, HealthSortField,
    LoadState, RefreshGeneration, build_health_issues,
};

/// UI/session state for health and duplicate review surfaces.
///
/// Health and duplicate computation remains in the existing page/core helpers;
/// this bundle only owns the filters, selections, sort state, and cached report
/// retained by the GUI between frames.
pub(crate) struct HealthDuplicateUiState {
    pub(crate) duplicate_filters: DuplicateReviewFilters,
    pub(crate) duplicate_sort_field: DuplicateSortField,
    pub(crate) duplicate_sort_ascending: bool,
    pub(crate) selected_duplicate_group: Option<DuplicateGroupIdentity>,
    pub(crate) selected_duplicate_archive: Option<PathBuf>,
    pub(crate) health_filters: HealthDashboardFilters,
    pub(crate) health_sort_field: HealthSortField,
    pub(crate) health_sort_ascending: bool,
    pub(crate) selected_health_issue: Option<PathBuf>,
    pub(crate) diagnostics_refresh_generation: RefreshGeneration,
    pub(crate) health_report_cache: Option<HealthReportCache>,
}

impl Default for HealthDuplicateUiState {
    fn default() -> Self {
        Self {
            duplicate_filters: DuplicateReviewFilters::initial(),
            duplicate_sort_field: DuplicateSortField::Title,
            duplicate_sort_ascending: true,
            selected_duplicate_group: None,
            selected_duplicate_archive: None,
            health_filters: HealthDashboardFilters::default(),
            health_sort_field: HealthSortField::default(),
            health_sort_ascending: true,
            selected_health_issue: None,
            diagnostics_refresh_generation: RefreshGeneration::INITIAL,
            health_report_cache: None,
        }
    }
}

impl ArchiveFsApp {
    /// The Health Dashboard's report, rebuilt only when the underlying
    /// live snapshot, database snapshot, diagnostics refresh, or recovery
    /// offers have actually changed since the last call - see
    /// `HealthReportCacheKey`'s doc comment for why pointer identity
    /// rather than a raw generation comparison. Never called unless the
    /// dashboard is actually open (see its one call site), so this adds
    /// no cost to the ordinary library view.
    pub(crate) fn cached_health_issues(&mut self) -> &[HealthIssue] {
        let key = HealthReportCacheKey {
            live_data_ptr: match &self.state {
                LoadState::Ready(data) => Some(std::ptr::from_ref(data.as_ref()) as usize),
                LoadState::Loading { .. } | LoadState::Error(_) => None,
            },
            database_snapshot_ptr: self
                .database_state
                .snapshot()
                .map(|snapshot| std::ptr::from_ref(snapshot) as usize),
            diagnostics_generation: self.health_duplicate_ui.diagnostics_refresh_generation,
        };

        let cache_is_fresh = self
            .health_duplicate_ui
            .health_report_cache
            .as_ref()
            .is_some_and(|cache| {
                cache.key == key
                    && cache.lazy_unmount_offers == self.mount_ui.lazy_unmount_offers
                    && cache.remount_offers == self.mount_ui.remount_offers
            });

        if !cache_is_fresh {
            let live_records = match &self.state {
                LoadState::Ready(data) => Some(&data.records),
                LoadState::Loading { .. } | LoadState::Error(_) => None,
            };
            let issues = match (live_records, self.database_state.snapshot()) {
                (Some(records), Some(snapshot)) => build_health_issues(
                    records,
                    snapshot,
                    &self.mount_ui.lazy_unmount_offers,
                    &self.mount_ui.remount_offers,
                ),
                _ => Vec::new(),
            };
            self.health_duplicate_ui.health_report_cache = Some(HealthReportCache {
                key,
                lazy_unmount_offers: self.mount_ui.lazy_unmount_offers.clone(),
                remount_offers: self.mount_ui.remount_offers.clone(),
                issues,
            });
        }

        &self
            .health_duplicate_ui
            .health_report_cache
            .as_ref()
            .unwrap()
            .issues
    }
}
