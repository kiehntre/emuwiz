//! EmuWiz Blue Game Details pieces (boards `D6/D7-Game-*`): the hero card,
//! the right-hand cards and Game information (decision D4).
//!
//! Presentation only. Game information shows saved ScreenScraper values the
//! person already accepted (`AcceptedScreenScraperMetadata`), one row per
//! value that exists; nothing is looked up, guessed or filled in.

use eframe::egui::{self, RichText};

use super::{library::Game, simple_shell::palette};
use crate::ui::theme::{self, blue};

/// Below this page width Blue Game Details keeps the single column.
pub(super) const SIDE_BY_SIDE_MIN_WIDTH: f32 = 1100.0;
/// The hero cover: 3:4, about 230 wide at 1920.
pub(super) const HERO_COVER: egui::Vec2 = egui::vec2(232.0, 309.0);

/// The hero panel around the cover, title and actions.
pub(super) fn hero_card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(blue::CARD_SURFACE_TOP)
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    ui.add_space(theme::SPACE_MD);
}

/// One card in the right-hand column, with an optional title.
pub(super) fn side_card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(blue::CARD_SURFACE_TOP)
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(theme::card_title(title));
                ui.add_space(6.0);
            }
            add(ui);
        });
    ui.add_space(theme::SPACE_MD);
}

/// The information rows that have a saved value, in the board's order.
pub(super) fn information_rows(game: &Game) -> Vec<(&'static str, String)> {
    let Some(saved) = game.screenscraper.as_ref() else {
        return Vec::new();
    };
    let values = &saved.values;
    let mut rows = Vec::new();
    let mut push = |label: &'static str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            rows.push((label, value));
        }
    };
    push("Developer", values.developer.clone());
    push("Publisher", values.publisher.clone());
    push("Year", values.release_year.map(|year| year.to_string()));
    push("Genre", values.genre.clone());
    push("Players", values.players.clone());
    push("Region", values.region.clone());
    push(
        "Languages",
        values
            .languages
            .as_ref()
            .filter(|languages| !languages.is_empty())
            .map(|languages| languages.join(", ")),
    );
    rows
}

/// Game information (D4): saved values only, collapsed by default as on the
/// board. With nothing saved it says so instead of showing empty rows.
pub(super) fn information_card(ui: &mut egui::Ui, game: &Game) {
    let rows = information_rows(game);
    side_card(ui, "", |ui| {
        egui::CollapsingHeader::new(RichText::new("Game information").strong())
            .id_salt(("v2_detail_information", game.archive.id))
            .default_open(false)
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(
                        RichText::new(
                            "No game information is saved for this game yet. Artwork, Manuals & Extras can look it up from your providers.",
                        )
                        .color(palette::SECONDARY_TEXT),
                    );
                    return;
                }
                egui::Grid::new(("v2_detail_information_rows", game.archive.id))
                    .num_columns(2)
                    .spacing(egui::vec2(16.0, 6.0))
                    .show(ui, |ui| {
                        for (label, value) in &rows {
                            ui.label(RichText::new(*label).color(palette::DIM_TEXT));
                            ui.label(value);
                            ui.end_row();
                        }
                    });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("From your saved ScreenScraper record.")
                        .size(13.0)
                        .color(palette::DIM_TEXT),
                );
            });
    });
}
