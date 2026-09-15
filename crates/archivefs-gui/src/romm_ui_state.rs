use crate::romm_browse::{BrowseState, StaleProgress};
use crate::romm_config::{RommConfigDraft, RommPreviewSummary};
use crate::romm_game::{GamePanelState, HashProgressView};
use crate::romm_source::{RommCardState, RommSnapshot, VerifyRommSummary};
use crate::RunningRommOperation;

/// UI/session state for the RomM surface.
///
/// Operation execution remains in the RomM controller and worker modules; this
/// bundle only groups the state those layers expose to the GUI. Shared GUI
/// configuration remains on `ArchiveFsApp` because it is also consumed by
/// non-RomM surfaces.
pub(crate) struct RommUiState {
    pub(crate) snapshot: Option<Box<RommSnapshot>>,
    pub(crate) verify_summary: Option<VerifyRommSummary>,
    pub(crate) operation: Option<RunningRommOperation>,
    pub(crate) generation: u64,
    pub(crate) card: RommCardState,
    pub(crate) config_draft: Option<Box<RommConfigDraft>>,
    pub(crate) preview: Option<Box<RommPreviewSummary>>,
    pub(crate) browse: Option<Box<BrowseState>>,
    pub(crate) stale_progress: Option<StaleProgress>,
    pub(crate) game: GamePanelState,
    pub(crate) hash_progress: Option<HashProgressView>,
}

impl Default for RommUiState {
    fn default() -> Self {
        Self {
            snapshot: None,
            verify_summary: None,
            operation: None,
            generation: 0,
            card: RommCardState::default(),
            config_draft: None,
            preview: None,
            browse: None,
            stale_progress: None,
            game: GamePanelState::default(),
            hash_progress: None,
        }
    }
}
