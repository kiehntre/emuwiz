//! Museum: a first-class, curated view of "what does EmuWiz know about my
//! collection", organised by platform and (from a platform) by game.
//!
//! This is a discovery/presentation layer only. Every fact shown here is
//! read from state the app already computed elsewhere this session:
//!
//! - platform counts come from [`crate::home_library_snapshot`]'s existing
//!   projection of the already-loaded [`archivefs_core::CachedLibrarySnapshot`]
//!   (the same numbers Home's own collection summary shows) - never a second
//!   scan, and never recomputed on every frame;
//! - platform identity (display name, what evidence distinguishes it, which
//!   emulator it usually implies) comes from the existing
//!   [`archivefs_core::platform::PLATFORMS`] registry;
//! - a selected game's identity/artwork/evidence is not re-derived here at
//!   all - Museum's game view is a curated summary plus direct links to the
//!   existing Selected Evidence / Cheats & Mods / Emulator Setup / Repair
//!   destinations, never a duplicate of them (see [`MuseumAction`]).
//!
//! No historical/manufacturer/release-period prose is invented: the current
//! platform registry does not store that data, so this page shows
//! collection-centric facts (what you actually have) rather than fabricated
//! encyclopaedia text.

use super::*;

#[derive(Debug, Default)]
pub(crate) struct MuseumPageState {
    /// `None` is the platform grid; `Some(name)` is one platform's detail
    /// view. `name` matches [`home_page::HomePlatformSummary::name`]
    /// exactly (the same string the rest of the app already uses to filter
    /// by platform), so this never needs its own platform-identity concept.
    pub(crate) selected_platform: Option<String>,
}

#[derive(Default)]
pub(crate) struct MuseumHeroState {
    poster_texture: Option<egui::TextureHandle>,
    poster_load_attempted: bool,
}

const MUSEUM_HERO_PNG: &[u8] = include_bytes!("../assets/emuwiz_hero_museum.png");

/// Where a Museum action should navigate. Museum never performs the
/// destination's own work (launching, editing cheats, ...) - it only picks
/// where to send the user, exactly like Home's `HomeCard` action channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MuseumAction {
    /// Show this platform's games in the ordinary Library view.
    BrowseLibraryForPlatform(String),
    /// Open Emulator Setup - offered on a platform's detail view because it
    /// names a `preferred_emulator` and the user may want to check it.
    OpenEmulatorSetup,
    OpenSelectedEvidence,
    OpenCheats(PathBuf),
    OpenRomm,
    OpenDiscConversion,
}

/// One platform card's collection-centric facts, projected once by
/// [`build_platform_view`] rather than recomputed inside rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MuseumPlatformView {
    pub(crate) name: String,
    pub(crate) games_total: usize,
    pub(crate) games_identified: usize,
    pub(crate) games_missing: usize,
    pub(crate) media_coverage: Option<archivefs_core::identity_source::model::MediaCoverage>,
    /// From the platform registry, when this exact display name is a
    /// recognised platform - `None` for a platform string the registry does
    /// not (yet) know, which is reported honestly rather than hidden.
    pub(crate) registry: Option<MuseumPlatformRegistryFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MuseumPlatformRegistryFacts {
    pub(crate) display_name: &'static str,
    pub(crate) preferred_emulator: Option<&'static str>,
    /// The registry's own "what evidence exists for this platform" prose -
    /// technical detection explanation, not history. Shown only in an
    /// expandable technical-details section, never presented as
    /// encyclopaedia text.
    pub(crate) explanation: &'static str,
}

/// A thin view of the currently selected game. It reuses the same selected
/// record/evidence/artwork caches as Library and Selected Evidence; Museum
/// does not perform its own lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MuseumSelectedGameView {
    pub(crate) archive_path: PathBuf,
    pub(crate) title: String,
    pub(crate) platform: String,
    pub(crate) facts: Vec<(String, String)>,
    pub(crate) evidence_highlights: Vec<String>,
    pub(crate) feature_view: Option<crate::feature_discovery::FeatureDiscoveryView>,
    pub(crate) screenshot_count: Option<usize>,
    pub(crate) video_available: Option<bool>,
    pub(crate) dat_verified: bool,
}

/// Pure projection: `library`'s already-computed per-platform counts, sorted
/// by name (matching [`home_page::HomeLibrarySnapshot::platforms`]'s own
/// deterministic order) and joined against the platform registry. No I/O,
/// no rescanning.
pub(crate) fn build_platform_views(
    library: &home_page::HomeLibrarySnapshot,
) -> Vec<MuseumPlatformView> {
    library
        .platforms
        .iter()
        .map(|summary| MuseumPlatformView {
            name: summary.name.clone(),
            games_total: summary.total,
            games_identified: summary.identified,
            games_missing: summary.missing,
            media_coverage: summary.romm_media_coverage,
            registry: archivefs_core::platform::PLATFORMS
                .iter()
                .find(|platform| platform.display_name == summary.name)
                .map(|platform| MuseumPlatformRegistryFacts {
                    display_name: platform.display_name,
                    preferred_emulator: platform.preferred_emulator,
                    explanation: platform.explanation,
                }),
        })
        .collect()
}

const MUSEUM_CARD_MIN_WIDTH: f32 = 220.0;
const MUSEUM_CARD_MAX_WIDTH: f32 = 300.0;
const MUSEUM_CARD_GAP: f32 = 14.0;
const MUSEUM_CARD_MAX_COLUMNS: usize = 4;

struct MuseumGridLayout {
    columns: usize,
    card_width: f32,
}

fn museum_grid_layout(available_width: f32, card_count: usize) -> MuseumGridLayout {
    let available_width = available_width.max(0.0);
    let columns_that_fit = ((available_width + MUSEUM_CARD_GAP)
        / (MUSEUM_CARD_MIN_WIDTH + MUSEUM_CARD_GAP))
        .floor() as usize;
    let columns = columns_that_fit
        .clamp(1, MUSEUM_CARD_MAX_COLUMNS)
        .min(card_count.max(1));
    let card_width = ((available_width - MUSEUM_CARD_GAP * columns.saturating_sub(1) as f32)
        / columns as f32)
        .min(MUSEUM_CARD_MAX_WIDTH)
        .max(0.0);
    MuseumGridLayout {
        columns,
        card_width,
    }
}

/// Renders the whole Museum destination: the platform grid, or one
/// platform's detail view when `state.selected_platform` is set.
pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut MuseumPageState,
    library: Option<&home_page::HomeLibrarySnapshot>,
) -> Option<MuseumAction> {
    let mut screenshot_requests = Vec::new();
    show_with_selected_game(
        ui,
        state,
        library,
        None,
        None,
        None,
        &mut screenshot_requests,
    )
}

pub(crate) fn show_with_selected_game(
    ui: &mut egui::Ui,
    state: &mut MuseumPageState,
    library: Option<&home_page::HomeLibrarySnapshot>,
    selected_game: Option<&MuseumSelectedGameView>,
    covers: Option<&crate::gamer_artwork::GamerCoverCache>,
    screenshots: Option<&mut crate::gamer_artwork::GamerScreenshotCache>,
    screenshot_requests: &mut Vec<crate::gamer_artwork::CoverJob>,
) -> Option<MuseumAction> {
    show_with_selected_game_and_artwork(
        ui,
        state,
        library,
        selected_game,
        covers,
        screenshots,
        None,
        screenshot_requests,
    )
}

/// `screenshot_requests` mirrors `GamerViewViewState::screenshot_requests`
/// exactly: this function never sends anything itself - it reports what the
/// selected game's screenshot section needs (via
/// `GamerScreenshotCache::visible`, the same scheduling primitive the Gamer
/// View Details screen already uses) and the caller hands the result to the
/// existing cover worker. Before this, Museum only ever *read*
/// `GamerScreenshotCache` - it held no `&mut` access and so could never ask
/// for anything not already requested by some other screen this session,
/// which is why a game never opened through Gamer View's Details screen
/// showed no screenshots here even when the provider genuinely had them.
pub(crate) fn show_with_selected_game_and_artwork(
    ui: &mut egui::Ui,
    state: &mut MuseumPageState,
    library: Option<&home_page::HomeLibrarySnapshot>,
    selected_game: Option<&MuseumSelectedGameView>,
    covers: Option<&crate::gamer_artwork::GamerCoverCache>,
    screenshots: Option<&mut crate::gamer_artwork::GamerScreenshotCache>,
    artwork: Option<&mut crate::platform_artwork_manager::ArtworkRenderAssets<'_>>,
    screenshot_requests: &mut Vec<crate::gamer_artwork::CoverJob>,
) -> Option<MuseumAction> {
    show_with_selected_game_and_artwork_inner(
        ui,
        None,
        state,
        library,
        selected_game,
        covers,
        screenshots,
        artwork,
        screenshot_requests,
    )
}

pub(crate) fn show_with_selected_game_and_artwork_with_hero(
    ui: &mut egui::Ui,
    hero_state: &mut MuseumHeroState,
    state: &mut MuseumPageState,
    library: Option<&home_page::HomeLibrarySnapshot>,
    selected_game: Option<&MuseumSelectedGameView>,
    covers: Option<&crate::gamer_artwork::GamerCoverCache>,
    screenshots: Option<&mut crate::gamer_artwork::GamerScreenshotCache>,
    artwork: Option<&mut crate::platform_artwork_manager::ArtworkRenderAssets<'_>>,
    screenshot_requests: &mut Vec<crate::gamer_artwork::CoverJob>,
) -> Option<MuseumAction> {
    show_with_selected_game_and_artwork_inner(
        ui,
        Some(hero_state),
        state,
        library,
        selected_game,
        covers,
        screenshots,
        artwork,
        screenshot_requests,
    )
}

fn show_with_selected_game_and_artwork_inner(
    ui: &mut egui::Ui,
    mut hero_state: Option<&mut MuseumHeroState>,
    state: &mut MuseumPageState,
    library: Option<&home_page::HomeLibrarySnapshot>,
    selected_game: Option<&MuseumSelectedGameView>,
    covers: Option<&crate::gamer_artwork::GamerCoverCache>,
    screenshots: Option<&mut crate::gamer_artwork::GamerScreenshotCache>,
    mut artwork: Option<&mut crate::platform_artwork_manager::ArtworkRenderAssets<'_>>,
    screenshot_requests: &mut Vec<crate::gamer_artwork::CoverJob>,
) -> Option<MuseumAction> {
    if let Some(hero_state) = hero_state.as_deref_mut() {
        let hero_rendered = show_museum_hero(
            ui,
            hero_state,
            library.is_some(),
            state.selected_platform.is_some(),
            selected_game.is_some(),
        );
        if !hero_rendered {
            show_museum_native_header(ui);
        }
    } else {
        show_museum_native_header(ui);
    }
    ui.add_space(theme::SECTION_GAP);

    let Some(library) = library else {
        widgets::empty_state(
            ui,
            "No library loaded yet",
            "Scan a source folder to see your collection here.",
            None,
        );
        return None;
    };

    let platforms = build_platform_views(library);
    if platforms.is_empty() {
        widgets::empty_state(
            ui,
            "No platforms identified yet",
            "Once games in your library are identified by platform, they will appear here.",
            None,
        );
        return None;
    }

    if let Some(selected) = state.selected_platform.clone() {
        match platforms.iter().find(|view| view.name == selected) {
            Some(view) => {
                let view = view.clone();
                if consume_museum_scroll(ui, museum_platform_scroll_id()) {
                    ui.scroll_to_cursor(Some(egui::Align::TOP));
                }
                show_platform_detail(
                    ui,
                    state,
                    &view,
                    selected_game,
                    covers,
                    screenshots,
                    artwork.as_deref_mut(),
                    screenshot_requests,
                )
            }
            None => {
                // The platform named by stale state no longer appears in the
                // current snapshot (e.g. the library reloaded) - return to
                // the grid rather than show a broken detail view.
                state.selected_platform = None;
                show_platform_grid(ui, state, &platforms, artwork.as_deref_mut())
            }
        }
    } else {
        if consume_museum_scroll(ui, museum_platform_scroll_id()) {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        show_platform_grid(ui, state, &platforms, artwork.as_deref_mut())
    }
}

fn show_museum_native_header(ui: &mut egui::Ui) {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::CHECK,
        "Museum",
        "Browse your collection by platform: what EmuWiz knows about each system, and how \
         complete your library is. Nothing here is rescanned - it reflects your most recent \
         library load.",
    );
}

fn cached_museum_hero(ui: &egui::Ui, state: &mut MuseumHeroState) -> bool {
    if !state.poster_load_attempted {
        state.poster_load_attempted = true;
        if let Ok(decoded) = image::load_from_memory(MUSEUM_HERO_PNG) {
            let rgba = decoded.to_rgba8();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            state.poster_texture = Some(ui.ctx().load_texture(
                "emuwiz-museum-hero",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }
    state.poster_texture.is_some()
}

fn museum_hero_height(width: f32) -> f32 {
    width * 821.0 / 1916.0
}

fn show_museum_hero(
    ui: &mut egui::Ui,
    state: &mut MuseumHeroState,
    has_library: bool,
    has_platform: bool,
    has_selected_game: bool,
) -> bool {
    if !cached_museum_hero(ui, state) {
        return false;
    }
    let width = ui.available_width().max(1.0);
    let height = museum_hero_height(width);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let texture = state.poster_texture.as_ref().expect("cached hero texture");
    ui.painter().image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    // The poster's illustrated panels are real egui hit regions, while the
    // artwork remains the only visible button treatment. Only expose a hit
    // target when the corresponding existing Museum flow can act on it.
    let panel_top = rect.top() + height * 0.648;
    let panel_height = height * 0.177;
    let panel = |left: f32, right: f32| {
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + width * left, panel_top),
            egui::pos2(rect.left() + width * right, panel_top + panel_height),
        )
    };
    let transparent = || {
        egui::Button::new("")
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
    };
    if has_library
        && ui
            .put(panel(0.025, 0.219), transparent())
            .on_hover_text("Browse the platforms in your Museum")
            .clicked()
    {
        ui.data_mut(|data| data.insert_temp(museum_platform_scroll_id(), true));
    }
    if has_platform
        && has_selected_game
        && ui
            .put(panel(0.226, 0.435), transparent())
            .on_hover_text("View the selected game's Museum showcase")
            .clicked()
    {
        ui.data_mut(|data| data.insert_temp(museum_game_scroll_id(), true));
    }
    if has_platform
        && has_selected_game
        && ui
            .put(panel(0.442, 0.642), transparent())
            .on_hover_text("Discover features available for the selected game")
            .clicked()
    {
        ui.data_mut(|data| data.insert_temp(museum_feature_scroll_id(), true));
    }
    true
}

fn museum_platform_scroll_id() -> egui::Id {
    egui::Id::new("museum-hero-platform-scroll")
}

fn museum_game_scroll_id() -> egui::Id {
    egui::Id::new("museum-hero-game-scroll")
}

fn museum_feature_scroll_id() -> egui::Id {
    egui::Id::new("museum-hero-feature-scroll")
}

fn consume_museum_scroll(ui: &mut egui::Ui, id: egui::Id) -> bool {
    if ui.data_mut(|data| data.get_temp::<bool>(id).unwrap_or(false)) {
        ui.data_mut(|data| data.remove::<bool>(id));
        true
    } else {
        false
    }
}

fn show_platform_grid(
    ui: &mut egui::Ui,
    state: &mut MuseumPageState,
    platforms: &[MuseumPlatformView],
    mut artwork: Option<&mut crate::platform_artwork_manager::ArtworkRenderAssets<'_>>,
) -> Option<MuseumAction> {
    let layout = museum_grid_layout(ui.available_width(), platforms.len());
    for row in platforms.chunks(layout.columns) {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
            for view in row {
                ui.allocate_ui_with_layout(
                    egui::vec2(layout.card_width, 224.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        widgets::card(ui, |ui| {
                            let art_size = egui::vec2(layout.card_width - 24.0, 86.0);
                            let art_rect = ui.allocate_space(art_size).1;
                            if let Some(artwork) = artwork.as_deref_mut() {
                                let asset_id = crate::ui::platform_artwork::platform_asset_id(
                                    &view.name,
                                    view.registry.is_none(),
                                );
                                let fallback =
                                    crate::ui::platform_artwork::platform_fallback_asset_id(
                                        &view.name,
                                        view.registry.is_none(),
                                    );
                                crate::ui::platform_artwork::paint_platform_artwork_at(
                                    ui,
                                    artwork.cache,
                                    artwork.directory,
                                    crate::ui::platform_artwork::PlatformArtworkPaint {
                                        center: art_rect.center(),
                                        size: art_rect.width().min(art_rect.height()),
                                        color: ui.visuals().text_color().gamma_multiply(0.8),
                                        asset_id: &asset_id,
                                        fallback_asset_id: fallback,
                                    },
                                );
                            }
                            ui.label(egui::RichText::new(&view.name).strong());
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} game{}",
                                    view.games_total,
                                    if view.games_total == 1 { "" } else { "s" }
                                ))
                                .color(theme::muted(ui)),
                            );
                            if view.games_missing > 0 {
                                widgets::status_badge(
                                    ui,
                                    format!("{} missing", view.games_missing),
                                    widgets::StatusTone::Warning,
                                );
                            }
                            if widgets::action_button(
                                ui,
                                "View",
                                widgets::ActionStyle::Secondary,
                                true,
                            )
                            .clicked()
                            {
                                state.selected_platform = Some(view.name.clone());
                            }
                        });
                    },
                );
                ui.add_space(MUSEUM_CARD_GAP);
            }
        });
        ui.add_space(MUSEUM_CARD_GAP);
    }
    None
}

fn show_platform_detail(
    ui: &mut egui::Ui,
    state: &mut MuseumPageState,
    view: &MuseumPlatformView,
    selected_game: Option<&MuseumSelectedGameView>,
    covers: Option<&crate::gamer_artwork::GamerCoverCache>,
    screenshots: Option<&mut crate::gamer_artwork::GamerScreenshotCache>,
    mut artwork: Option<&mut crate::platform_artwork_manager::ArtworkRenderAssets<'_>>,
    screenshot_requests: &mut Vec<crate::gamer_artwork::CoverJob>,
) -> Option<MuseumAction> {
    let mut action = None;
    if widgets::action_button(ui, "< All platforms", widgets::ActionStyle::Secondary, true)
        .clicked()
    {
        state.selected_platform = None;
    }
    ui.add_space(theme::SPACE_MD);

    widgets::section_header(ui, &view.name, None);
    widgets::card(ui, |ui| {
        if let Some(artwork) = artwork.as_deref_mut() {
            let art_rect = ui.allocate_space(egui::vec2(ui.available_width(), 96.0)).1;
            let asset_id =
                crate::ui::platform_artwork::platform_asset_id(&view.name, view.registry.is_none());
            let fallback = crate::ui::platform_artwork::platform_fallback_asset_id(
                &view.name,
                view.registry.is_none(),
            );
            crate::ui::platform_artwork::paint_platform_artwork_at(
                ui,
                artwork.cache,
                artwork.directory,
                crate::ui::platform_artwork::PlatformArtworkPaint {
                    center: art_rect.center(),
                    size: art_rect.width().min(art_rect.height()),
                    color: ui.visuals().text_color().gamma_multiply(0.8),
                    asset_id: &asset_id,
                    fallback_asset_id: fallback,
                },
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("Games: {}", view.games_total)).strong());
            ui.label(format!("Identified: {}", view.games_identified));
            if view.games_missing > 0 {
                widgets::status_badge(
                    ui,
                    format!("{} missing from disk", view.games_missing),
                    widgets::StatusTone::Warning,
                );
            }
        });
        if let Some(coverage) = view.media_coverage {
            ui.add_space(theme::SPACE_SM);
            ui.strong("Media in your collection");
            ui.label(format!(
                "Covers: {} of {}",
                coverage.covers_available, coverage.total_games
            ));
            ui.label(format!(
                "Screenshots: {} of {}",
                coverage.screenshots_available, coverage.total_games
            ));
            ui.label(match coverage.videos_available {
                Some(videos) => format!("Videos: {} of {}", videos, coverage.total_games),
                None => "Videos: Not indexed".to_string(),
            });
        }
        if let Some(game) = selected_game.filter(|game| game.platform == view.name) {
            if consume_museum_scroll(ui, museum_game_scroll_id())
                || consume_museum_scroll(ui, museum_feature_scroll_id())
            {
                ui.scroll_to_cursor(Some(egui::Align::TOP));
            }
            action =
                show_selected_game_showcase(ui, game, covers, screenshots, screenshot_requests)
                    .or(action.clone());
        }
        if let Some(registry) = &view.registry {
            ui.add_space(theme::SPACE_XS);
            if let Some(emulator) = registry.preferred_emulator {
                ui.label(format!("Usually launched with: {emulator}"));
            }
            widgets::technical_details(ui, ("museum-platform", view.name.as_str()), |ui| {
                ui.label(registry.explanation);
            });
        } else {
            ui.add_space(theme::SPACE_XS);
            ui.label(
                egui::RichText::new(
                    "This platform is not in EmuWiz's built-in registry yet, so no extra \
                     detection detail is available here.",
                )
                .small()
                .color(theme::muted(ui)),
            );
        }
        ui.add_space(theme::SPACE_MD);
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(ui, "Browse in Library", widgets::ActionStyle::Primary, true)
                .clicked()
            {
                action = Some(MuseumAction::BrowseLibraryForPlatform(view.name.clone()));
            }
            if view
                .registry
                .as_ref()
                .and_then(|r| r.preferred_emulator)
                .is_some()
                && widgets::action_button(
                    ui,
                    "Emulator Setup",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
            {
                action = Some(MuseumAction::OpenEmulatorSetup);
            }
        });
    });
    action
}

fn show_selected_game_showcase(
    ui: &mut egui::Ui,
    game: &MuseumSelectedGameView,
    covers: Option<&crate::gamer_artwork::GamerCoverCache>,
    screenshots: Option<&mut crate::gamer_artwork::GamerScreenshotCache>,
    screenshot_requests: &mut Vec<crate::gamer_artwork::CoverJob>,
) -> Option<MuseumAction> {
    let mut action = None;
    ui.add_space(theme::SPACE_MD);
    widgets::section_header(ui, "Selected game", None);
    widgets::hero_card(ui, |ui| {
        ui.horizontal_top(|ui| {
            let cover = covers.and_then(|cache| cache.slot_for(&game.archive_path, None));
            let cover_size = egui::vec2(150.0, 210.0);
            match cover {
                Some(crate::gamer_artwork::CoverSlot::Ready { texture, .. }) => {
                    widgets::media_frame(ui, cover_size, None, |ui, rect| {
                        let drawn =
                            crate::gamer_artwork::fit_within(cover_size, texture.size_vec2());
                        ui.painter().image(
                            texture.id(),
                            egui::Rect::from_center_size(rect.center(), drawn),
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    });
                }
                Some(crate::gamer_artwork::CoverSlot::Loading)
                | Some(crate::gamer_artwork::CoverSlot::Revalidating { .. }) => {
                    widgets::media_frame(ui, cover_size, None, |ui, _| {
                        ui.centered_and_justified(|ui| ui.label("Loading cover…"));
                    });
                }
                Some(crate::gamer_artwork::CoverSlot::None(reason)) => {
                    ui.allocate_ui(cover_size, |ui| {
                        ui.centered_and_justified(|ui| {
                            ui.label(match reason {
                                crate::gamer_artwork::NoCover::Failed => "Cover load failed",
                                _ => "No cover art available",
                            });
                        });
                    });
                }
                None => {
                    ui.allocate_ui(cover_size, |ui| {
                        ui.centered_and_justified(|ui| ui.label("No cover art available"));
                    });
                }
            }
            ui.add_space(theme::SPACE_MD);
            ui.vertical(|ui| {
                ui.heading(&game.title);
                widgets::status_badge(ui, &game.platform, widgets::StatusTone::Info);
                if game.dat_verified {
                    widgets::status_badge(ui, "DAT verified", widgets::StatusTone::Success);
                }
                ui.add_space(theme::SPACE_SM);
                ui.horizontal_wrapped(|ui| {
                    if widgets::action_button(
                        ui,
                        "View full evidence",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                    {
                        action = Some(MuseumAction::OpenSelectedEvidence);
                    }
                });
            });
        });
    });

    ui.add_space(theme::SPACE_MD);
    widgets::section_header(ui, "Collection facts", None);
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Platform: ").strong());
            ui.label(&game.platform);
        });
        for (label, value) in &game.facts {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("{label}: ")).strong());
                ui.label(value);
            });
        }
    });

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(ui, "Screenshots", None);
    let screenshot_count = game
        .screenshot_count
        .unwrap_or_default()
        .min(crate::gamer_artwork::MAX_DETAILS_SCREENSHOTS);
    if let Some(cache) = screenshots {
        // Schedule the same request Gamer View's Details screen would make
        // for this exact path - the one thing this section was missing
        // before: it could read `cache` but never had `&mut` access to ask
        // it for anything. Safe to call every frame this section renders:
        // `visible` is a no-op for any index that already has a slot.
        screenshot_requests.extend(cache.visible(&game.archive_path));
        let screenshot_pending = cache.delivery(&game.archive_path)
            == Some(archivefs_core::identity_source::media_resolver::MediaDelivery::RemotePending);
        if game.screenshot_count.is_none() || screenshot_pending {
            ui.label("Loading screenshots…");
        } else if game.screenshot_count == Some(0) {
            ui.label(egui::RichText::new("No screenshots available").color(theme::muted(ui)));
        } else {
            let slots: Vec<_> = (0..screenshot_count)
                .filter_map(|index| cache.slot_for(&game.archive_path, index))
                .collect();
            let ready: Vec<_> = slots
                .iter()
                .filter_map(|slot| match slot {
                    crate::gamer_artwork::CoverSlot::Ready { texture, .. } => Some(texture),
                    _ => None,
                })
                .collect();
            if !ready.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for texture in ready.iter().take(4) {
                        let size = egui::vec2(180.0, 105.0);
                        widgets::media_frame(ui, size, None, |ui, rect| {
                            let drawn = crate::gamer_artwork::fit_within(size, texture.size_vec2());
                            ui.painter().image(
                                texture.id(),
                                egui::Rect::from_center_size(rect.center(), drawn),
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        });
                    }
                });
                if cache.has_loading(&game.archive_path) {
                    ui.label("Loading more…");
                }
            } else if slots.iter().any(|slot| {
                matches!(
                    slot,
                    crate::gamer_artwork::CoverSlot::Loading
                        | crate::gamer_artwork::CoverSlot::Revalidating { .. }
                )
            }) {
                ui.label("Loading screenshots…");
            } else if slots.iter().any(|slot| {
                matches!(
                    slot,
                    crate::gamer_artwork::CoverSlot::None(crate::gamer_artwork::NoCover::Failed)
                )
            }) {
                ui.label("Could not load screenshots");
            } else {
                ui.label("No screenshots available");
            }
        }
    } else {
        ui.label("Loading screenshots…");
    }

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(ui, "Video", None);
    ui.label(match game.video_available {
        Some(true) => "Video available",
        Some(false) => "Video not available",
        None => "Video coverage not indexed",
    });

    if !game.evidence_highlights.is_empty() {
        ui.add_space(theme::SECTION_GAP);
        widgets::section_header(ui, "Evidence highlights", None);
        widgets::card(ui, |ui| {
            for highlight in &game.evidence_highlights {
                ui.label(format!("✓ {highlight}"));
            }
            if widgets::action_button(
                ui,
                "View full evidence",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                action = Some(MuseumAction::OpenSelectedEvidence);
            }
        });
    }

    if let Some(feature_view) = &game.feature_view {
        if let Some(feature_action) = crate::feature_discovery::show(ui, feature_view) {
            action = Some(match feature_action {
                crate::feature_discovery::FeatureDiscoveryAction::OpenCheats => {
                    MuseumAction::OpenCheats(game.archive_path.clone())
                }
                crate::feature_discovery::FeatureDiscoveryAction::OpenRomm => {
                    MuseumAction::OpenRomm
                }
                crate::feature_discovery::FeatureDiscoveryAction::OpenEmulatorSetup => {
                    MuseumAction::OpenEmulatorSetup
                }
                crate::feature_discovery::FeatureDiscoveryAction::OpenDiscConversion => {
                    MuseumAction::OpenDiscConversion
                }
            });
        }
    }
    action
}

#[cfg(test)]
mod tests;
