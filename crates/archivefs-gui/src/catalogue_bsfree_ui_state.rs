use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use archivefs_core::patch_manager::{
    BsFreeCheat, BsFreeGame, BsFreeGameSearchRequest, BsFreeGameSearchResult, BsFreeSourceStatus,
    BsFreeSystem,
};
use archivefs_core::patch_manager::{
    CheatSourceError, CheatSourceFetchResult, DolphinCatalogueError, DolphinCatalogueFetchResult,
    DolphinCatalogueUpdateCheck,
};

use crate::sources_page::{
    CatalogueManagerState, CatalogueReview, DolphinCatalogueManagerState,
    DolphinCatalogueRetrievalKind, RunningCatalogueRetrieval, RunningDolphinCatalogueRetrieval,
};

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

#[derive(Debug)]
pub(crate) enum BsFreeManagerState {
    NotLoaded,
    Ready(Box<BsFreeSourceStatus>),
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) enum BsFreeOperation {
    LoadStatus,
    Download,
    Import(PathBuf),
    Validate,
    SetEnabled(bool),
    Remove,
    LoadSystems,
    Search(BsFreeGameSearchRequest),
    LoadGame { upstream_uid: i64, offset: u32 },
}

#[derive(Debug)]
pub(crate) enum BsFreeOperationResult {
    Status(Box<BsFreeSourceStatus>),
    Removed,
    Search(BsFreeGameSearchResult),
    Systems(archivefs_core::patch_manager::ProviderPage<BsFreeSystem>),
    Game(
        BsFreeGame,
        archivefs_core::patch_manager::ProviderPage<BsFreeCheat>,
    ),
}

pub(crate) struct RunningBsFreeOperation {
    pub(crate) operation: BsFreeOperation,
    pub(crate) receiver: Receiver<Result<BsFreeOperationResult, String>>,
}

#[derive(Debug, Default)]
pub(crate) struct BsFreeGuiState {
    pub(crate) import_path: String,
    pub(crate) download_confirm: bool,
    pub(crate) remove_confirm: bool,
    pub(crate) search_context: Option<PathBuf>,
    pub(crate) search_title: String,
    pub(crate) search_platform: String,
    pub(crate) search_system_id: Option<i64>,
    pub(crate) platforms: Option<Result<Vec<BsFreeSystem>, String>>,
    pub(crate) platform_query: String,
    pub(crate) search_result: Option<Result<BsFreeGameSearchResult, String>>,
    pub(crate) selected_game: Option<BsFreeGame>,
    pub(crate) cheats:
        Option<Result<archivefs_core::patch_manager::ProviderPage<BsFreeCheat>, String>>,
}
