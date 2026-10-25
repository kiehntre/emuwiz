use std::path::PathBuf;

use archivefs_core::ConfigIdentity;

use crate::game_metadata::{GameMetadataResult, GameMetadataWorker};
use crate::gamer_artwork::{CoverWorker, GamerCoverCache, GamerScreenshotCache};
use crate::gamer_view::AlphaJumpIndex;
use crate::platform_artwork_manager::PlatformArtworkManager;

/// Session-owned artwork, media, Museum, and Gamer View state. Artwork and
/// metadata workers remain the existing feature workers; this type only
/// groups their UI-facing state on the application.
pub(crate) struct ArtworkMediaState {
    /// The single platform-artwork session: managed directory, texture
    /// cache, Settings drafts and the background worker. Museum, Gamer View
    /// and Settings all read and drive this one owner.
    pub(crate) platform_artwork: PlatformArtworkManager,
    pub(crate) gamer_covers: GamerCoverCache,
    pub(crate) gamer_screenshots: GamerScreenshotCache,
    pub(crate) museum_page: crate::museum_page::MuseumPageState,
    pub(crate) museum_hero: crate::museum_page::MuseumHeroState,
    pub(crate) gamer_cover_worker: Option<CoverWorker>,
    pub(crate) gamer_cover_worker_allowed: bool,
    pub(crate) gamer_cover_library: Option<ConfigIdentity>,
    pub(crate) selected_game_metadata: Option<(PathBuf, GameMetadataResult)>,
    pub(crate) game_metadata_worker: Option<GameMetadataWorker>,
    pub(crate) game_metadata_worker_allowed: bool,
    pub(crate) gamer_alpha_jump: AlphaJumpIndex,
    pub(crate) es_de_media: crate::es_de_media_state::EsDeMediaState,
    pub(crate) launchbox_local_media: crate::launchbox_local_state::LaunchBoxLocalMediaState,
}

impl ArtworkMediaState {
    pub(crate) fn new() -> Self {
        Self {
            platform_artwork: PlatformArtworkManager::new(
                archivefs_core::platform_artwork::default_platform_artwork_root().ok(),
                crate::open_folder_in_file_manager,
            ),
            gamer_covers: GamerCoverCache::default(),
            gamer_screenshots: GamerScreenshotCache::default(),
            museum_page: crate::museum_page::MuseumPageState::default(),
            museum_hero: crate::museum_page::MuseumHeroState::default(),
            gamer_cover_worker: None,
            gamer_cover_worker_allowed: true,
            gamer_cover_library: None,
            selected_game_metadata: None,
            game_metadata_worker: None,
            game_metadata_worker_allowed: true,
            gamer_alpha_jump: AlphaJumpIndex::default(),
            es_de_media: crate::es_de_media_state::EsDeMediaState::default(),
            launchbox_local_media: crate::launchbox_local_state::LaunchBoxLocalMediaState::default(
            ),
        }
    }
}
