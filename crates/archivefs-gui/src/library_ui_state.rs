use crate::{
    LibraryColumnWidths, LibraryRowFilters, RunningAliasAction, RunningBulkPlatformAction,
    RunningMissingRemoval, RunningPlatformAction, SortField,
};

/// UI state owned by the Library surface.
///
/// Database loading, archive selection identity, navigation, mount execution,
/// and readiness state deliberately remain outside this bundle. Those systems
/// provide inputs or effects to the Library surface but do not belong to its
/// presentation state.
pub(crate) struct LibraryUiState {
    pub(crate) filter: String,
    pub(crate) filtered_rows: Option<Vec<usize>>,
    pub(crate) library_filters: LibraryRowFilters,
    pub(crate) library_platform_query: String,
    pub(crate) platform_action: Option<RunningPlatformAction>,
    pub(crate) platform_choice: Option<String>,
    pub(crate) platform_custom_text: String,
    pub(crate) alias_action: Option<RunningAliasAction>,
    pub(crate) missing_removal: Option<RunningMissingRemoval>,
    pub(crate) confirm_remove_missing: Option<Vec<std::path::PathBuf>>,
    pub(crate) new_alias_text: String,
    pub(crate) new_alias_platform_choice: Option<String>,
    pub(crate) bulk_platform_action: Option<RunningBulkPlatformAction>,
    pub(crate) bulk_platform_choice: Option<String>,
    pub(crate) sort_field: Option<SortField>,
    pub(crate) sort_ascending: bool,
    pub(crate) library_scroll_offset: f32,
    pub(crate) library_source_filter: Option<Option<std::path::PathBuf>>,
    pub(crate) library_column_widths: LibraryColumnWidths,
}

impl Default for LibraryUiState {
    fn default() -> Self {
        Self {
            filter: String::new(),
            filtered_rows: None,
            library_filters: LibraryRowFilters::default(),
            library_platform_query: String::new(),
            platform_action: None,
            platform_choice: None,
            platform_custom_text: String::new(),
            alias_action: None,
            missing_removal: None,
            confirm_remove_missing: None,
            new_alias_text: String::new(),
            new_alias_platform_choice: None,
            bulk_platform_action: None,
            bulk_platform_choice: None,
            sort_field: None,
            sort_ascending: true,
            library_scroll_offset: 0.0,
            library_source_filter: None,
            library_column_widths: LibraryColumnWidths::default(),
        }
    }
}
