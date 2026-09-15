use std::path::PathBuf;

use crate::{
    DuplicateGroupIdentity, DuplicateReviewFilters, DuplicateSortField, HealthDashboardFilters,
    HealthReportCache, HealthSortField, RefreshGeneration,
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
