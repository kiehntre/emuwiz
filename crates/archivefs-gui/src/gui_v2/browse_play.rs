//! Low-noise, game-first presentation over the canonical GUI-v2 library.
//!
//! Browse & Play owns no catalogue, artwork, selection or launch state. A
//! selected game is represented by the route and actions hand off to the
//! existing GUI-v2 workflows.

use super::{
    App,
    library::Game,
    media_sources::Kind,
    routes::{Route, Section},
};
use crate::ui::theme;
use eframe::egui::{self, RichText};

const PLATFORM_ICON_SIZE: f32 = 32.0;
const GRID_CARD_WIDTH: f32 = 178.0;
const GRID_COVER_SIZE: egui::Vec2 = egui::vec2(158.0, 158.0);
const LIST_COVER_SIZE: egui::Vec2 = egui::vec2(56.0, 74.0);
const SELECTED_COVER_SIZE: egui::Vec2 = egui::vec2(148.0, 190.0);

fn browse_primary(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).strong())
            .fill(theme::PRIMARY_ACTION)
            .min_size(egui::vec2(160.0, 44.0)),
    )
    .clicked()
}

pub(super) fn filtered_game_indices(app: &App) -> Vec<usize> {
    app.library.filter(&app.filter)
}

pub(super) fn browse_play_contextual_routes(game: i64) -> [(&'static str, Route); 4] {
    [
        (
            "Cheats & Mods",
            Route::Task {
                section: Section::Mods,
                game,
            },
        ),
        (
            "Saves & States",
            Route::Task {
                section: Section::Saves,
                game,
            },
        ),
        ("Manuals / Guides", Route::Game(game)),
        (
            "Problems & Repair",
            Route::Task {
                section: Section::Problems,
                game,
            },
        ),
    ]
}

impl App {
    pub(super) fn browse_play(&mut self, ui: &mut egui::Ui, selected_id: Option<i64>) {
        let library = self.library.clone();
        let selected_game = selected_id.and_then(|id| library.game(id).cloned());

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Browse & Play").strong());
            ui.label("Games first · management tools remain in the sidebar.");
        });
        ui.add_space(theme::SPACE_SM);
        self.browse_play_search(ui);
        ui.separator();
        self.browse_play_platforms(ui, &library);
        ui.add_space(theme::SPACE_SM);

        let wide = ui.available_width() >= 980.0;
        if wide {
            ui.columns(2, |columns| {
                self.browse_play_games(&mut columns[0], &library, selected_id);
                if let Some(game) = selected_game.as_ref() {
                    self.browse_play_selected(&mut columns[1], game);
                } else {
                    Self::selected_game_prompt(&mut columns[1]);
                }
            });
        } else {
            if let Some(game) = selected_game.as_ref() {
                self.browse_play_selected(ui, game);
                ui.add_space(theme::SPACE_MD);
            }
            self.browse_play_games(ui, &library, selected_id);
        }
    }

    fn browse_play_search(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Search games");
            ui.add_sized(
                [280.0, 36.0],
                egui::TextEdit::singleline(&mut self.filter.search)
                    .hint_text("Title or platform")
                    .desired_width(280.0),
            );
            if !self.filter.search.is_empty() && ui.button("Clear search").clicked() {
                self.filter.search.clear();
            }
            if ui.button("All platforms").clicked() {
                self.filter.select_platform(String::new());
            }
            if !self.filter.platform.is_empty() {
                ui.label(format!("Platform: {}", self.filter.platform));
            }
            if !self.filter.search.is_empty() {
                ui.label(format!("Search: \"{}\"", self.filter.search));
            }
            ui.separator();
            ui.selectable_value(&mut self.filter.list, false, "Grid");
            ui.selectable_value(&mut self.filter.list, true, "Compact list");
        });
    }

    fn browse_play_platforms(&mut self, ui: &mut egui::Ui, library: &super::library::Library) {
        ui.horizontal(|ui| {
            ui.strong("Platforms");
            ui.label("Choose a system to narrow the shelf.");
        });
        ui.horizontal_wrapped(|ui| {
            let platforms: Vec<_> = library.platforms.keys().cloned().collect();
            if platforms.is_empty() {
                ui.label("No platforms are available yet.");
            } else {
                for platform in platforms {
                    let count = library
                        .platforms
                        .get(&platform)
                        .copied()
                        .unwrap_or_default();
                    let selected = self.filter.platform == platform;
                    ui.push_id(("v2_browse_play_platform", platform.as_str()), |ui| {
                        let response = egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_min_width(148.0);
                            ui.horizontal(|ui| {
                                self.imagery
                                    .platform_icon(ui, &platform, PLATFORM_ICON_SIZE);
                                ui.vertical(|ui| {
                                    ui.strong(&platform);
                                    ui.label(format!("{count} games"));
                                });
                            });
                        });
                        let response = response.response.interact(egui::Sense::click());
                        if selected {
                            ui.painter().rect_stroke(
                                response.rect,
                                6.0,
                                egui::Stroke::new(2.0_f32, theme::PRIMARY_ACTION),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if response.clicked() {
                            self.filter.select_platform(platform.clone());
                        }
                    });
                }
            }
        });
    }

    fn browse_play_games(
        &mut self,
        ui: &mut egui::Ui,
        library: &super::library::Library,
        selected_id: Option<i64>,
    ) {
        let indices = filtered_game_indices(self);
        ui.horizontal(|ui| {
            ui.strong("Games");
            ui.label(format!("{} in the current scope", indices.len()));
        });
        if indices.is_empty() {
            self.browse_play_empty(ui, library.games.is_empty());
            return;
        }

        if self.filter.list {
            for index in indices {
                let Some(game) = library.games.get(index) else {
                    continue;
                };
                self.browse_play_game_card(ui, game, selected_id == Some(game.archive.id), true);
            }
        } else {
            let columns = ((ui.available_width() / GRID_CARD_WIDTH).floor() as usize).clamp(1, 5);
            for row in indices.chunks(columns) {
                ui.horizontal_top(|ui| {
                    for index in row {
                        let Some(game) = library.games.get(*index) else {
                            continue;
                        };
                        self.browse_play_game_card(
                            ui,
                            game,
                            selected_id == Some(game.archive.id),
                            false,
                        );
                    }
                });
            }
        }
    }

    fn browse_play_game_card(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        selected: bool,
        list: bool,
    ) {
        let id = game.archive.id;
        let size = if list {
            LIST_COVER_SIZE
        } else {
            GRID_COVER_SIZE
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.push_id(("v2_browse_play_game", id), |ui| {
                ui.set_min_width(if list {
                    ui.available_width()
                } else {
                    GRID_CARD_WIDTH
                });
                ui.set_min_height(if list { 88.0 } else { 250.0 });
                if list {
                    ui.horizontal(|ui| {
                        self.picture(ui, game, Kind::Cover, size);
                        self.browse_play_card_text(ui, game, selected, id);
                    });
                } else {
                    self.picture(ui, game, Kind::Cover, size);
                    self.browse_play_card_text(ui, game, selected, id);
                }
            });
        });
    }

    fn browse_play_card_text(&mut self, ui: &mut egui::Ui, game: &Game, selected: bool, id: i64) {
        ui.vertical(|ui| {
            ui.set_min_width(if selected { 120.0 } else { 100.0 });
            ui.add_space(4.0);
            ui.strong(&game.title);
            ui.label(&game.platform);
            ui.label(if game.attention {
                "Needs attention"
            } else {
                game.identity_summary()
            });
            if ui
                .add(
                    egui::Button::new(if selected { "Selected" } else { "Select" })
                        .selected(selected),
                )
                .clicked()
            {
                self.go(Route::BrowsePlayGame(id));
            }
        });
    }

    fn browse_play_selected(&mut self, ui: &mut egui::Ui, game: &Game) {
        let id = game.archive.id;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.push_id(("v2_browse_play_selected", id), |ui| {
                ui.heading("Selected game");
                ui.heading(&game.title);
                ui.label(format!("Platform: {}", game.platform));
                ui.label(format!("Identity: {}", game.identity_summary()));
                ui.label(if game.attention {
                    "Launch issue needs attention"
                } else if game.identified {
                    "Launch checks available"
                } else {
                    "Needs verification before launch"
                });
                ui.add_space(theme::SPACE_SM);
                if browse_primary(ui, "Play") {
                    self.go(Route::Task {
                        section: Section::Launch,
                        game: id,
                    });
                }
                if ui.button("Game Details").clicked() {
                    self.go(Route::Game(id));
                }
                ui.label("Play opens the existing readiness and launch planner.");
                self.picture(ui, game, Kind::Cover, SELECTED_COVER_SIZE);
                ui.separator();
                ui.label("More for this game");
                ui.horizontal_wrapped(|ui| {
                    for (label, route) in browse_play_contextual_routes(id) {
                        ui.push_id(("v2_browse_play_action", id, label), |ui| {
                            if ui.button(label).clicked() {
                                self.go(route.clone());
                            }
                        });
                    }
                });
            });
        });
    }

    fn selected_game_prompt(ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Select a game");
            ui.label("Choose a title from the shelf to see artwork, readiness and Play.");
        });
    }

    fn browse_play_empty(&mut self, ui: &mut egui::Ui, library_empty: bool) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading(if library_empty {
                "No games have been added yet"
            } else {
                "No games match the current search/filter"
            });
            if library_empty {
                ui.label("Add a game folder in Sources & Providers, then scan it into the existing library.");
                if browse_primary(ui, "Open Sources") {
                    self.go(Route::Section(Section::Sources));
                }
            } else {
                ui.label("Try another search or platform, or clear the active filters.");
                if ui.button("Clear search and platform").clicked() {
                    self.filter = Default::default();
                }
            }
        });
    }
}
