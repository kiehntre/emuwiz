//! Low-noise, game-first presentation over the canonical GUI-v2 library.
//!
//! This page owns no catalogue, artwork, selection or launch state.  A
//! selected game is represented by the route and all actions hand off to the
//! existing GUI-v2 workflows.

use super::{
    App,
    library::Game,
    media_sources::Kind,
    routes::{Route, Section},
};
use crate::ui::theme;
use eframe::egui::{self, RichText};

fn browse_primary(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).strong())
            .fill(theme::PRIMARY_ACTION)
            .min_size(egui::vec2(160.0, 42.0)),
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
        ("Saves & States", Route::Section(Section::Saves)),
        (
            "Manuals / Guides",
            Route::Task {
                section: Section::Artwork,
                game,
            },
        ),
        ("Problems & Repair", Route::Section(Section::Problems)),
    ]
}

impl App {
    pub(super) fn browse_play(&mut self, ui: &mut egui::Ui, selected_id: Option<i64>) {
        let library = self.library.clone();
        ui.label("Games first: choose a platform, find a title, and start playing.");
        ui.add_space(theme::SPACE_SM);

        ui.horizontal_wrapped(|ui| {
            ui.label("Search");
            ui.add_sized(
                [260.0, 36.0],
                egui::TextEdit::singleline(&mut self.filter.search).hint_text("Title or platform"),
            );
            if ui.button("All platforms").clicked() {
                self.filter.select_platform(String::new());
            }
            if !self.filter.platform.is_empty() {
                ui.label(format!("Platform: {}", self.filter.platform));
            }
        });

        ui.separator();
        ui.strong("Platforms");
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
                    ui.push_id(("v2_browse_play_platform", platform.as_str()), |ui| {
                        if ui
                            .add(
                                egui::Button::new(format!("{platform} · {count}"))
                                    .selected(self.filter.platform == platform),
                            )
                            .clicked()
                        {
                            self.filter.select_platform(platform.clone());
                        }
                    });
                }
            }
        });

        if let Some(id) = selected_id {
            if let Some(game) = library.game(id) {
                ui.add_space(theme::SPACE_MD);
                self.browse_play_selected(ui, game);
            }
        }

        let indices = filtered_game_indices(self);
        ui.add_space(theme::SPACE_SM);
        ui.horizontal(|ui| {
            ui.strong("Games");
            ui.label(format!("{} in the current scope", indices.len()));
        });
        if indices.is_empty() {
            self.browse_play_empty(ui, library.games.is_empty());
        } else {
            egui::ScrollArea::vertical()
                .id_salt(("v2_browse_play_games", self.filter.platform.as_str()))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for index in indices {
                        let Some(game) = library.games.get(index) else {
                            continue;
                        };
                        self.browse_play_game_card(ui, game, selected_id == Some(game.archive.id));
                    }
                });
        }
    }

    fn browse_play_game_card(&mut self, ui: &mut egui::Ui, game: &Game, selected: bool) {
        let id = game.archive.id;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.push_id(("v2_browse_play_game", id), |ui| {
                ui.horizontal(|ui| {
                    self.picture(ui, game, Kind::Cover, egui::vec2(76.0, 100.0));
                    ui.vertical(|ui| {
                        ui.strong(&game.title);
                        ui.label(&game.platform);
                        ui.label(if game.identified {
                            "Identity verified"
                        } else {
                            "Identity needs review"
                        });
                        if ui
                            .button(if selected { "Selected" } else { "Select game" })
                            .clicked()
                        {
                            self.go(Route::BrowsePlayGame(id));
                        }
                    });
                });
            });
        });
    }

    fn browse_play_selected(&mut self, ui: &mut egui::Ui, game: &Game) {
        let id = game.archive.id;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.push_id(("v2_browse_play_selected", id), |ui| {
                ui.heading("Selected game");
                ui.horizontal_top(|ui| {
                    self.picture(ui, game, Kind::Cover, egui::vec2(150.0, 198.0));
                    ui.vertical(|ui| {
                        ui.heading(&game.title);
                        ui.label(format!("Platform: {}", game.platform));
                        ui.label(if game.attention {
                            "Readiness needs attention"
                        } else if game.identified {
                            "Identity verified · launch checks available"
                        } else {
                            "Identity needs review before launch"
                        });
                        ui.add_space(theme::SPACE_SM);
                        ui.horizontal_wrapped(|ui| {
                            if browse_primary(ui, "Play") {
                                self.go(Route::Task {
                                    section: Section::Launch,
                                    game: id,
                                });
                            }
                            if ui.button("Game Details").clicked() {
                                self.go(Route::Game(id));
                            }
                        });
                        ui.label("Play opens the existing readiness and launch planner.");
                    });
                });
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

    fn browse_play_empty(&mut self, ui: &mut egui::Ui, library_empty: bool) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading(if library_empty { "No games found" } else { "No games match this scope" });
            if library_empty {
                ui.label("Add a game folder in Sources & Providers, then scan it into the existing library.");
                if browse_primary(ui, "Open Sources") {
                    self.go(Route::Section(Section::Sources));
                }
            } else {
                ui.label("Try another search or platform. Your library has not been changed.");
                if ui.button("Clear search and platform").clicked() {
                    self.filter = Default::default();
                }
            }
        });
    }
}
