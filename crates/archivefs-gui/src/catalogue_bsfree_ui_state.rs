use std::sync::mpsc::Receiver;

use archivefs_core::patch_manager::{
    CheatSourceError, CheatSourceFetchResult, DolphinCatalogueError, DolphinCatalogueFetchResult,
    DolphinCatalogueUpdateCheck,
};

use crate::sources_page::{
    CatalogueManagerState, CatalogueReview, DolphinCatalogueManagerState,
    DolphinCatalogueRetrievalKind, RunningCatalogueRetrieval, RunningDolphinCatalogueRetrieval,
};

use crate::{BsFreeGuiState, BsFreeManagerState, RunningBsFreeOperation};

/// UI/session state for BSFree and catalogue surfaces.
///
/// Retrieval workers and provider/domain implementations remain in their
/// existing controllers and core APIs. This bundle only consolidates the
/// feature state that the GUI retains between frames.
pub(crate) struct CatalogueBsFreeUiState {
    pub(crate) bsfree_manager: BsFreeManagerState,
    pub(crate) bsfree_operation: Option<RunningBsFreeOperation>,
    pub(crate) bsfree_ui: BsFreeGuiState,
    pub(crate) catalogue_manager: CatalogueManagerState,
    pub(crate) catalogue_review: Option<CatalogueReview>,
    pub(crate) catalogue_retrieval: Option<RunningCatalogueRetrieval>,
    pub(crate) catalogue_generation: u64,
    pub(crate) catalogue_last_result: Option<Result<CheatSourceFetchResult, CheatSourceError>>,
    pub(crate) dolphin_catalogue_manager: DolphinCatalogueManagerState,
    pub(crate) dolphin_catalogue_review: Option<DolphinCatalogueRetrievalKind>,
    pub(crate) dolphin_catalogue_retrieval: Option<RunningDolphinCatalogueRetrieval>,
    pub(crate) dolphin_catalogue_generation: u64,
    pub(crate) dolphin_catalogue_last_result:
        Option<Result<DolphinCatalogueFetchResult, DolphinCatalogueError>>,
    pub(crate) dolphin_catalogue_remove_confirm: bool,
    pub(crate) dolphin_catalogue_update_available: Option<bool>,
    pub(crate) dolphin_catalogue_update_check:
        Option<Receiver<Result<DolphinCatalogueUpdateCheck, DolphinCatalogueError>>>,
}

impl Default for CatalogueBsFreeUiState {
    fn default() -> Self {
        Self {
            bsfree_manager: BsFreeManagerState::NotLoaded,
            bsfree_operation: None,
            bsfree_ui: BsFreeGuiState::default(),
            catalogue_manager: CatalogueManagerState::NotLoaded,
            catalogue_review: None,
            catalogue_retrieval: None,
            catalogue_generation: 0,
            catalogue_last_result: None,
            dolphin_catalogue_manager: DolphinCatalogueManagerState::NotLoaded,
            dolphin_catalogue_review: None,
            dolphin_catalogue_retrieval: None,
            dolphin_catalogue_generation: 0,
            dolphin_catalogue_last_result: None,
            dolphin_catalogue_remove_confirm: false,
            dolphin_catalogue_update_available: None,
            dolphin_catalogue_update_check: None,
        }
    }
}
