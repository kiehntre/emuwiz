//! Artwork-led presentation for Home and Platforms.
//!
//! Paint-only: every action here routes to a destination the page already
//! offered (a platform's games, a game's details, Setup & Doctor). Pictures
//! come from [`super::imagery`] (bundled/managed platform hardware) and the
//! existing bounded cover pipeline ([`super::artwork`]); nothing is fetched.
use super::{
    App,
    artwork::Picture,
    imagery::{EmptyArt, empty_state},
    media_sources::Kind,
    routes::{Route, Section},
};
use crate::ui::{components::hero_card, theme};
use eframe::egui::{self, RichText};

const SYSTEM_TILE: egui::Vec2 = egui::vec2(148.0, 150.0);
const SHOWCASE_TILE: egui::Vec2 = egui::vec2(132.0, 176.0);
const TILE_GAP: f32 = 10.0;
pub(super) const PLATFORM_CARD_HEIGHT: f32 = 168.0;
const PLATFORM_CARD_MIN_WIDTH: f32 = 440.0;

fn section_title(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.add_space(theme::SPACE_SM);
    ui.label(
        RichText::new(title)
            .size(theme::SECTION_TITLE_SIZE)
            .strong(),
    );
    ui.label(RichText::new(detail).color(theme::muted(ui)));
    ui.add_space(theme::SPACE_XS);
}

fn tiles_that_fit(available: f32, tile: f32) -> usize {
    ((available + TILE_GAP) / (tile + TILE_GAP))
        .floor()
        .max(1.0) as usize
}

impl App {
    /// Real systems (never "Unknown system"), largest collection first.
    fn largest_systems(&self, limit: usize) -> Vec<(String, usize)> {
        let mut systems: Vec<_> = self
            .library
            .platforms
            .iter()
            .filter(|(platform, _)| {
                crate::ui::platform_artwork::canonical_platform_for_artwork(platform).is_some()
            })
            .map(|(platform, count)| (platform.clone(), *count))
            .collect();
        systems.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        systems.truncate(limit);
        systems
    }

    fn open_platform_games(&mut self, platform: String) {
        self.filter.select_platform(platform);
        self.filter.attention_only = false;
        self.change_filter();
        self.go(Route::Section(Section::Games));
    }

    /// The library hero: the EmuWiz mascot beside the library totals and
    /// setup state. Hardware imagery follows in "Your systems".
    pub(super) fn home_hero(&mut self, ui: &mut egui::Ui) {
        let wide = ui.available_width() >= 760.0;
        let mut open_setup = false;
        hero_card(ui, |ui| {
            ui.horizontal(|ui| {
                if wide {
                    let size = egui::vec2(104.0, 104.0);
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    if let Some(texture) = self.imagery.mascot(ui.ctx()) {
                        ui.painter().image(
                            texture.id(),
                            rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    ui.add_space(theme::SPACE_MD);
                }
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Your game library")
                            .size(theme::PAGE_TITLE_SIZE)
                            .strong(),
                    );
                    if self.loaded {
                        ui.label(format!(
                            "{} games · {} systems · {} need attention",
                            self.library.games.len(),
                            self.library.platforms.len(),
                            self.library.attention
                        ));
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading your existing game list. You can already explore the tasks below.");
                        });
                    }
                    if let Some(environment) = self.environment.as_ref() {
                        let ready_count = environment.setup_ready_count(&self.library);
                        let attention_count = environment.setup_attention_count(&self.library);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "System setup: {ready_count} ready · {attention_count} need attention"
                                ))
                                .color(theme::muted(ui)),
                            );
                            open_setup = ui.button("Open Setup & Doctor").clicked();
                        });
                    }
                });
            });
        });
        if open_setup {
            self.go(Route::Section(Section::Setup));
        }
    }

    /// "Your systems": hardware tiles for the largest collections. Each tile
    /// opens that system's games, exactly like its Platforms card.
    pub(super) fn home_systems(&mut self, ui: &mut egui::Ui) {
        let fit = tiles_that_fit(ui.available_width(), SYSTEM_TILE.x).min(8);
        let systems = self.largest_systems(fit);
        if systems.is_empty() {
            return;
        }
        section_title(
            ui,
            "Your systems",
            "Your largest collections. Choose one to browse its games.",
        );
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;
            for (platform, count) in &systems {
                let (rect, response) = ui.allocate_exact_size(SYSTEM_TILE, egui::Sense::click());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        true,
                        format!("{platform}, {count} games"),
                    )
                });
                let hovered = response.hovered() || response.has_focus();
                ui.painter().rect_filled(rect, 10.0, theme::CARD_SURFACE);
                ui.painter().rect_stroke(
                    rect,
                    10.0,
                    if hovered {
                        egui::Stroke::new(1.5_f32, theme::TEAL)
                    } else {
                        theme::border(ui)
                    },
                    egui::StrokeKind::Inside,
                );
                let plate = egui::Rect::from_min_size(
                    rect.min + egui::vec2(6.0, 6.0),
                    egui::vec2(rect.width() - 12.0, 96.0),
                );
                ui.painter().rect_filled(plate, 8.0, theme::DEEP_BACKGROUND);
                self.imagery
                    .paint_platform(ui, plate.shrink(6.0), platform, egui::Color32::WHITE);
                let name = ui.painter().layout(
                    platform.clone(),
                    egui::FontId::proportional(theme::BODY_SIZE),
                    theme::PRIMARY_TEXT,
                    rect.width() - 12.0,
                );
                let name_pos = egui::pos2(rect.min.x + 8.0, plate.max.y + 4.0);
                ui.painter().with_clip_rect(rect.shrink(4.0)).galley(
                    name_pos,
                    name,
                    theme::PRIMARY_TEXT,
                );
                ui.painter().text(
                    egui::pos2(rect.min.x + 8.0, rect.max.y - 8.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{count} games"),
                    egui::FontId::proportional(theme::METADATA_SIZE),
                    theme::SECONDARY_TEXT,
                );
                if response
                    .on_hover_text(format!("Browse {platform} games"))
                    .clicked()
                {
                    chosen = Some(platform.clone());
                }
            }
        });
        if let Some(platform) = chosen {
            self.open_platform_games(platform);
        }
    }

    /// A row of game covers on Home: games opened this session, or else the
    /// newest games whose cover is a local file. Both reuse pictures that are
    /// already on this computer. The automatic showcase skips covers that turn
    /// out to be unavailable, and the row is hidden when it has nothing.
    pub(super) fn home_showcase(&mut self, ui: &mut egui::Ui) {
        let library = self.library.clone();
        let recent = self.imagery.recently_opened().to_vec();
        let showcase = recent.is_empty();
        let (title, detail, candidates) = if showcase {
            (
                "Newest in your library",
                "Games with a cover picture on this computer, newest files first.",
                self.imagery
                    .showcase(&library, self.artwork.index.as_ref())
                    .to_vec(),
            )
        } else {
            (
                "Recently opened",
                "Games you looked at in this session, most recent first.",
                recent,
            )
        };
        let games: Vec<_> = candidates
            .iter()
            .filter(|id| {
                // Opened games stay listed (with their platform artwork if the
                // cover is unavailable); the automatic showcase skips them.
                !showcase
                    || !matches!(
                        self.artwork
                            .pictures
                            .get(&self.artwork.key(**id, Kind::Cover)),
                        Some(Picture::Missing | Picture::Failed(_))
                    )
            })
            .filter_map(|id| library.game(*id))
            .take(tiles_that_fit(ui.available_width(), SHOWCASE_TILE.x).min(8))
            .collect();
        if games.is_empty() {
            return;
        }
        section_title(ui, title, detail);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;
            for game in games {
                ui.push_id(("v2_showcase", game.archive.id), |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(SHOWCASE_TILE.x, SHOWCASE_TILE.y + 48.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(SHOWCASE_TILE.x);
                            self.picture(ui, game, Kind::Cover, SHOWCASE_TILE);
                            self.game_link(ui, game);
                        },
                    );
                });
            }
        });
    }

    /// Hardware-led Platforms grid. Rows are virtualised; each card asks for
    /// its (cached, thumbnail-sized) artwork only while visible.
    pub(super) fn platforms_grid(&mut self, ui: &mut egui::Ui) {
        if self.library.platforms.is_empty() {
            if empty_state(
                ui,
                &mut self.imagery,
                EmptyArt::Glyph("console"),
                "No systems are listed yet",
                "Systems appear here once EmuWiz has found games in your game folders. Discover your game folders to get started.",
                Some("Add my games"),
            ) {
                self.go(Route::Section(Section::Sources));
            }
            return;
        }
        let library = self.library.clone();
        let platforms: Vec<_> = library.platforms.iter().collect();
        let available = ui.available_width();
        let columns = ((available + 12.0) / (PLATFORM_CARD_MIN_WIDTH + 12.0))
            .floor()
            .clamp(1.0, 4.0) as usize;
        let width = (available - 12.0 * (columns - 1) as f32) / columns as f32;
        let rows = platforms.len().div_ceil(columns);
        let mut check = None;
        let mut view = None;
        egui::ScrollArea::vertical()
            .id_salt("v2_platforms")
            .auto_shrink([false, false])
            .show_rows(ui, PLATFORM_CARD_HEIGHT + 12.0, rows, |ui, visible| {
                for row in visible {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        for (platform, count) in platforms.iter().skip(row * columns).take(columns)
                        {
                            ui.push_id(("v2_platform_card", platform.as_str()), |ui| {
                                match self.platform_card(ui, platform, **count, width) {
                                    Some(true) => view = Some((*platform).clone()),
                                    Some(false) => check = Some((*platform).clone()),
                                    None => {}
                                }
                            });
                        }
                    });
                    ui.add_space(12.0 - ui.spacing().item_spacing.y);
                }
            });
        if let Some(platform) = check {
            self.check_platform(platform);
        } else if let Some(platform) = view {
            self.open_platform_games(platform);
        }
    }

    /// One platform card: `Some(true)` = view its games, `Some(false)` = check it.
    fn platform_card(
        &mut self,
        ui: &mut egui::Ui,
        platform: &str,
        count: usize,
        width: f32,
    ) -> Option<bool> {
        let mut choice = None;
        let ready = platform.eq_ignore_ascii_case("Arcade") || count > 0;
        egui::Frame::new()
            .fill(theme::CARD_SURFACE)
            .stroke(theme::border(ui))
            .corner_radius(10)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(width - 22.0);
                ui.set_height(PLATFORM_CARD_HEIGHT - 22.0);
                ui.horizontal_top(|ui| {
                    let side = PLATFORM_CARD_HEIGHT - 22.0;
                    let (plate, _) =
                        ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
                    ui.painter().rect_filled(plate, 8.0, theme::DEEP_BACKGROUND);
                    self.imagery.paint_platform(
                        ui,
                        plate.shrink(8.0),
                        platform,
                        egui::Color32::WHITE,
                    );
                    ui.add_space(theme::SPACE_SM);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(platform)
                                    .size(theme::SECTION_TITLE_SIZE)
                                    .strong(),
                            )
                            .truncate(),
                        );
                        ui.add(
                            egui::Label::new(format!("{count} games available to browse"))
                                .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(if ready {
                                    "Verification data ready · game folder configured"
                                } else {
                                    "Needs setup"
                                })
                                .color(theme::muted(ui)),
                            )
                            .truncate(),
                        );
                        ui.add_space(theme::SPACE_XS);
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("View games").strong())
                                        .fill(theme::PRIMARY_ACTION)
                                        .min_size(egui::vec2(120.0, 38.0)),
                                )
                                .on_hover_text(format!("View {platform} games"))
                                .clicked()
                            {
                                choice = Some(true);
                            }
                            if ui
                                .add(
                                    egui::Button::new(if ready { "Check" } else { "Set up" })
                                        .min_size(egui::vec2(80.0, 38.0)),
                                )
                                .on_hover_text(if ready {
                                    format!("Check {platform}")
                                } else {
                                    format!("Set up {platform}")
                                })
                                .clicked()
                            {
                                choice = Some(false);
                            }
                        });
                    });
                });
            });
        choice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_fit_the_available_width_and_never_drop_to_zero() {
        assert_eq!(tiles_that_fit(10.0, 148.0), 1);
        assert_eq!(tiles_that_fit(148.0 * 4.0 + TILE_GAP * 3.0, 148.0), 4);
    }
}
