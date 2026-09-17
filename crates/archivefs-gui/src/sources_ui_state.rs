use std::path::PathBuf;

use crate::dat_authority_dashboard::DashboardState;
use crate::dat_sources_page::{DatSourcesPageState, DatSourcesPageUi};
use crate::media_sets_page::MediaSetsPageState;
use crate::platform_source_actions::{RunningSourceAction, SourcesLastScan};
use crate::source_controller::{SourcesAddDialogState, SourcesRemoveDialogState};
use crate::sources_page::MountRootFeedback;
use crate::{ScanPersistSummary, cheat_sources_page};

/// UI/session state for the Sources, DAT, and source-adjacent media surfaces.
/// Source persistence and scan execution remain owned by their existing
/// controllers; this bundle only owns state retained by the application.
#[derive(Default)]
pub(crate) struct SourcesUiState {
    pub(crate) cheat_sources_page: Option<cheat_sources_page::CheatSourcesPageState>,
    pub(crate) cheat_sources_ui: cheat_sources_page::CheatSourcesPageUi,
    pub(crate) dat_sources_page: Option<DatSourcesPageState>,
    pub(crate) dat_sources_ui: DatSourcesPageUi,
    pub(crate) media_sets_page: MediaSetsPageState,
    pub(crate) quick_rename_mode: bool,
    pub(crate) source_action: Option<RunningSourceAction>,
    pub(crate) mount_root_draft: Option<PathBuf>,
    pub(crate) mount_root_feedback: Option<MountRootFeedback>,
    pub(crate) sources_add_dialog: Option<SourcesAddDialogState>,
    pub(crate) sources_remove_dialog: Option<SourcesRemoveDialogState>,
    pub(crate) dat_authority: DashboardState,
    pub(crate) pending_source_scan_summary: Option<ScanPersistSummary>,
    pub(crate) sources_last_scan: Option<SourcesLastScan>,
    pub(crate) gamer_view_pending_first_scan: Option<PathBuf>,
}
