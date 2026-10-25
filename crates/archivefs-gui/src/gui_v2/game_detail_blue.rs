//! EmuWiz Blue Game Details pieces (boards `D6/D7-Game-*`): the hero card,
//! the right-hand cards and Game information (decision D4).
//!
//! Presentation only. Game information uses the shared resolved evidence,
//! with accepted ScreenScraper values while indexing is pending. Rendering
//! never looks up providers, changes precedence, or fills in unknown values.

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

/// Saved values that the shared resolver selected, with a source for each row.
/// Languages are not part of the neutral model, so retain their accepted record.
fn resolved_information_rows(
    game: &Game,
    resolved: Option<&archivefs_core::metadata_aggregation::ResolvedMetadata>,
) -> Vec<(&'static str, String, String)> {
    use archivefs_core::metadata_aggregation::MetadataField as Field;
    let saved = information_rows(game);
    let mut rows = Vec::new();
    for (label, field) in [
        ("Developer", Field::Developer),
        ("Publisher", Field::Publisher),
        ("Year", Field::ReleaseDate),
        ("Genre", Field::Genre),
        ("Players", Field::Players),
        ("Rating", Field::Rating),
        ("Region", Field::Region),
    ] {
        if let Some(candidate) = resolved.and_then(|evidence| evidence.fields.get(&field)) {
            if !candidate.value.trim().is_empty() {
                let label = if field == Field::ReleaseDate
                    && !(candidate.value.len() == 4
                        && candidate.value.bytes().all(|byte| byte.is_ascii_digit()))
                {
                    "Release date"
                } else {
                    label
                };
                rows.push((
                    label,
                    candidate.value.clone(),
                    candidate.provenance.provider.label().into(),
                ));
            }
        } else if let Some((_, value)) = saved.iter().find(|(name, _)| *name == label) {
            rows.push((label, value.clone(), "ScreenScraper (saved record)".into()));
        }
    }
    if let Some((label, value)) = saved.iter().find(|(label, _)| *label == "Languages") {
        rows.push((*label, value.clone(), "ScreenScraper (saved record)".into()));
    }
    rows
}

/// Shared explanation used by Game Details and Artwork, Manuals & Extras.
pub(super) fn metadata_sources(
    ui: &mut egui::Ui,
    evidence: &archivefs_core::metadata_aggregation::ResolvedMetadata,
) {
    ui.collapsing("Metadata sources & alternatives", |ui| {
        ui.label("Source priority is applied per field. Equal-priority saved metadata follows catalogue, ES-DE, RomM, then ScreenScraper order. Selected values are shown first; different saved values are retained as alternatives.");
        for (field, selected) in &evidence.fields {
            ui.label(format!("{field:?}: {} · {} · {}", selected.value, selected.provenance.provider.label(), selected.provenance.detail));
            for alternative in evidence.conflicts.iter().filter(|candidate| candidate.field == *field) {
                ui.label(format!("Alternative {field:?}: {} · {} · {}", alternative.value, alternative.provenance.provider.label(), alternative.provenance.detail));
            }
        }
        if evidence.conflicts.is_empty() {
            ui.label("No conflicting metadata values were found in the checked sources.");
        }
    });
}

/// Collapsed information card backed by the same evidence as artwork delivery.
pub(super) fn information_card(
    ui: &mut egui::Ui,
    game: &Game,
    index: Option<&super::media_sources::MediaIndex>,
) {
    let resolved = index.map(|index| index.resolved_for(game));
    let rows = resolved_information_rows(game, resolved.as_deref());
    side_card(ui, "", |ui| {
        egui::CollapsingHeader::new(RichText::new("Game information").strong())
            .id_salt(("v2_detail_information", game.archive.id))
            .default_open(false)
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(RichText::new(if index.is_none() {
                        "Game information sources are still being checked."
                    } else {
                        "No game information was found in the checked sources. Artwork, Manuals & Extras can recheck your providers."
                    }).color(palette::SECONDARY_TEXT));
                } else {
                    egui::Grid::new(("v2_detail_information_rows", game.archive.id))
                        .num_columns(2)
                        .spacing(egui::vec2(16.0, 6.0))
                        .show(ui, |ui| {
                            for (label, value, source) in &rows {
                                ui.label(RichText::new(*label).color(palette::DIM_TEXT));
                                ui.vertical(|ui| {
                                    ui.label(value);
                                    ui.label(RichText::new(format!("From {source}")).size(13.0).color(palette::DIM_TEXT));
                                });
                                ui.end_row();
                            }
                        });
                }
                if let Some(resolved) = resolved.as_deref() {
                    metadata_sources(ui, resolved);
                }
            });
    });
}
