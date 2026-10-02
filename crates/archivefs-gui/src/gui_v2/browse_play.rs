//! Low-noise, game-first presentation over the canonical GUI-v2 library.
//!
//! Browse & Play owns no catalogue, artwork, selection or launch state. A
//! selected game is represented by the route and actions hand off to the
//! existing GUI-v2 workflows.
//!
//! Layout: one toolbar, one compact systems row, then the shelf and the
//! selected-game panel side by side. The page itself never scrolls; only the
//! shelf does (virtualised, so a 132,000-game library is fully reachable), and
//! the selected-game panel stays in view beside it.

use std::sync::Arc;

use super::{
    App,
    artwork::Picture,
    library::{Filter, Game},
    media_sources::Kind,
    routes::{Route, Section},
};
use crate::ui::theme;
use eframe::egui::{self, Color32, RichText};

const PLATFORM_ICON_SIZE: f32 = 22.0;
/// Largest systems shown as chips; the rest live in the dropdown.
const TOP_SYSTEMS: usize = 7;
const CARD_WIDTH: f32 = 176.0;
const CARD_MAX_WIDTH: f32 = 230.0;
const CARD_PAD: f32 = 8.0;
const COVER_HEIGHT: f32 = 150.0;
const CARD_HEIGHT: f32 = 246.0;
const LIST_ROW_HEIGHT: f32 = 84.0;
const LIST_COVER_SIZE: egui::Vec2 = egui::vec2(52.0, 68.0);
const SELECTED_COVER_SIZE: egui::Vec2 = egui::vec2(150.0, 196.0);
const COMPACT_COVER_SIZE: egui::Vec2 = egui::vec2(70.0, 92.0);
/// Below this width the selected-game panel becomes a strip above the shelf.
const SIDE_BY_SIDE_MIN_WIDTH: f32 = 900.0;
const ATTENTION_TEXT: Color32 = Color32::from_rgb(232, 170, 62);

/// Cached filter result so typing or scrolling never rescans the catalogue.
#[derive(Default)]
pub(super) struct BrowsePlayState {
    key: Option<(String, String, bool, bool, usize)>,
    indices: Arc<Vec<usize>>,
    /// The shelf starts from the top again when the search or system changes.
    reset_scroll: bool,
    /// Plain-language reason some covers are missing, taken from the covers
    /// drawn last frame, so it is said once instead of on every card.
    cover_note: Option<String>,
}

#[cfg(test)]
impl BrowsePlayState {
    pub(super) fn reset_scroll_pending(&self) -> bool {
        self.reset_scroll
    }
}

fn browse_primary(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).strong())
            .fill(theme::PRIMARY_ACTION)
            .min_size(egui::vec2(120.0, 40.0)),
    )
    .clicked()
}

#[cfg_attr(not(test), allow(dead_code))]
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

/// `132064` -> `132,064`.
fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn status_text(game: &Game) -> (&'static str, Color32) {
    if game.attention {
        ("Needs attention", ATTENTION_TEXT)
    } else {
        match game.identity_summary() {
            "Verified" => ("Verified", theme::SUCCESS),
            other => (other, theme::SECONDARY_TEXT),
        }
    }
}

/// One sentence that says what Play will do for this game.
fn readiness_sentence(game: &Game) -> &'static str {
    if game.attention {
        "Something needs attention before this can start. Game Details shows what."
    } else if game.identified {
        "Play checks that everything is ready first. Nothing starts until you confirm."
    } else {
        "This game is not verified yet. Play will show what is missing."
    }
}

/// "Verified · SNES" on one line. The status comes first so a long system name
/// is what gets shortened, never the status.
fn status_line(game: &Game, width: f32) -> egui::text::LayoutJob {
    let (status, colour) = status_text(game);
    let font = egui::FontId::proportional(theme::METADATA_SIZE);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        status,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: colour,
            ..Default::default()
        },
    );
    job.append(
        &format!("  ·  {}", game.platform),
        0.0,
        egui::TextFormat {
            font_id: font,
            color: theme::SECONDARY_TEXT,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    job
}

/// A title clamped to two lines so every card is the same height.
fn two_line_title(text: &str, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: egui::FontId::proportional(theme::BODY_SIZE),
            color: theme::PRIMARY_TEXT,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 2,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job
}

impl App {
    pub(super) fn browse_play(&mut self, ui: &mut egui::Ui, selected_id: Option<i64>) {
        let library = self.library.clone();
        let selected_game = selected_id.and_then(|id| library.game(id).cloned());
        let indices = self.browse_play_indices();
        // Decided before anything is drawn: a widget that overflows widens the
        // parent, and the layout below must not be sized from that.
        let page_width = ui.available_width();

        self.browse_play_toolbar(ui);
        ui.add_space(theme::SPACE_XS);
        self.browse_play_systems(ui, &library);
        ui.add_space(theme::SPACE_XS);
        ui.separator();

        let size = egui::vec2(page_width, ui.available_height());
        let height = size.y.max(260.0);
        if size.x >= SIDE_BY_SIDE_MIN_WIDTH {
            let panel_width = (size.x * 0.32).clamp(340.0, 420.0);
            let gap = ui.spacing().item_spacing.x;
            let shelf_width = size.x - panel_width - gap;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(shelf_width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_size(egui::vec2(shelf_width, height));
                        self.browse_play_games(ui, &library, &indices, selected_id);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(panel_width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_size(egui::vec2(panel_width, height));
                        match selected_game.as_ref() {
                            Some(game) => self.browse_play_selected(ui, game, false),
                            None => Self::selected_game_prompt(ui),
                        }
                    },
                );
            });
        } else {
            if let Some(game) = selected_game.as_ref() {
                self.browse_play_selected(ui, game, true);
                ui.add_space(theme::SPACE_SM);
            }
            self.browse_play_games(ui, &library, &indices, selected_id);
        }
    }

    /// The games matching the current search and system. Recomputed only when
    /// the filter or the library changes.
    fn browse_play_indices(&mut self) -> Arc<Vec<usize>> {
        let key = (
            self.filter.platform.clone(),
            self.filter.search.trim().to_lowercase(),
            self.filter.attention_only,
            self.filter.unverified_only,
            Arc::as_ptr(&self.library) as usize,
        );
        if self.browse_play.key.as_ref() != Some(&key) {
            self.browse_play.indices = Arc::new(self.library.filter(&self.filter));
            self.browse_play.reset_scroll = true;
            self.browse_play.key = Some(key);
        }
        self.browse_play.indices.clone()
    }

    fn browse_play_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_sized(
                [300.0, 34.0],
                egui::TextEdit::singleline(&mut self.filter.search)
                    .hint_text("Search by title or system")
                    .desired_width(300.0),
            );
            if !self.filter.search.is_empty() && ui.button("Clear").clicked() {
                self.filter.search.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.selectable_value(&mut self.filter.list, true, "List");
                ui.selectable_value(&mut self.filter.list, false, "Grid");
            });
        });
    }

    fn browse_play_systems(&mut self, ui: &mut egui::Ui, library: &super::library::Library) {
        let mut ordered: Vec<(String, usize)> = library
            .platforms
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect();
        ordered.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let total: usize = ordered.iter().map(|(_, count)| count).sum();
        let mut chosen: Option<String> = None;

        ui.horizontal_wrapped(|ui| {
            if self.system_chip(
                ui,
                None,
                "All systems",
                total,
                self.filter.platform.is_empty(),
            ) {
                chosen = Some(String::new());
            }
            let mut shown: Vec<&(String, usize)> = ordered.iter().take(TOP_SYSTEMS).collect();
            if !self.filter.platform.is_empty()
                && !shown.iter().any(|(name, _)| *name == self.filter.platform)
                && let Some(entry) = ordered
                    .iter()
                    .find(|(name, _)| *name == self.filter.platform)
            {
                shown.push(entry);
            }
            for (name, count) in shown {
                let selected = self.filter.platform == *name;
                if self.system_chip(ui, Some(name), name, *count, selected) {
                    chosen = Some(name.clone());
                }
            }
            let mut rest: Vec<&(String, usize)> = ordered
                .iter()
                .filter(|(name, _)| {
                    !ordered.iter().take(TOP_SYSTEMS).any(|(top, _)| top == name)
                        && *name != self.filter.platform
                })
                .collect();
            if !rest.is_empty() {
                rest.sort_by(|a, b| a.0.cmp(&b.0));
                egui::ComboBox::from_id_salt("v2_browse_play_more_systems")
                    .selected_text(format!("More systems ({})", rest.len()))
                    // A known width lets the wrapping row place it instead of overflowing.
                    .width(190.0)
                    .height(360.0)
                    .show_ui(ui, |ui| {
                        for (name, count) in rest {
                            if ui
                                .selectable_label(
                                    false,
                                    format!("{name}  ·  {}", thousands(*count)),
                                )
                                .clicked()
                            {
                                chosen = Some(name.clone());
                            }
                        }
                    });
            }
        });
        if let Some(platform) = chosen {
            self.filter.select_platform(platform);
        }
    }

    /// One compact, clickable system chip, sized from its text so a row of them
    /// wraps cleanly (nested frames do not wrap, and their overflow widens the
    /// whole page). Returns whether it was clicked.
    fn system_chip(
        &mut self,
        ui: &mut egui::Ui,
        icon_for: Option<&str>,
        label: &str,
        count: usize,
        selected: bool,
    ) -> bool {
        let name = ui.painter().layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(theme::BODY_SIZE),
            theme::PRIMARY_TEXT,
        );
        let number = ui.painter().layout_no_wrap(
            thousands(count),
            egui::FontId::proportional(theme::METADATA_SIZE),
            theme::SECONDARY_TEXT,
        );
        let icon = if icon_for.is_some() {
            PLATFORM_ICON_SIZE + 6.0
        } else {
            0.0
        };
        let width = 12.0 + icon + name.size().x + 8.0 + number.size().x + 12.0;
        // Allocated directly in the wrapping row (a child scope would not wrap).
        let (_, rect) = ui.allocate_space(egui::vec2(width, 32.0));
        let response = ui.interact(
            rect,
            ui.id().with(("v2_browse_play_platform", label)),
            egui::Sense::click(),
        );
        let hovered = response.hovered();
        let fill = if selected {
            theme::PRIMARY_ACTION
        } else if hovered {
            theme::RAISED_SURFACE
        } else {
            theme::CARD_SURFACE
        };
        ui.painter().rect_filled(rect, 16.0, fill);
        ui.painter().rect_stroke(
            rect,
            16.0,
            egui::Stroke::new(
                1.0_f32,
                if selected {
                    theme::PRIMARY_ACTION_HOVER
                } else {
                    theme::BORDER_SUBTLE
                },
            ),
            egui::StrokeKind::Inside,
        );
        theme::paint_focus_ring(ui, &response, rect, 16.0);
        let mut x = rect.left() + 12.0;
        if let Some(platform) = icon_for {
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - PLATFORM_ICON_SIZE / 2.0),
                egui::vec2(PLATFORM_ICON_SIZE, PLATFORM_ICON_SIZE),
            );
            self.imagery
                .paint_platform(ui, icon_rect, platform, Color32::WHITE);
            x += icon;
        }
        let name_pos = egui::pos2(x, rect.center().y - name.size().y / 2.0);
        let name_width = name.size().x;
        ui.painter().galley(name_pos, name, theme::PRIMARY_TEXT);
        let number_pos = egui::pos2(
            x + name_width + 8.0,
            rect.center().y - number.size().y / 2.0,
        );
        ui.painter()
            .galley(number_pos, number, theme::SECONDARY_TEXT);
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    }

    fn browse_play_games(
        &mut self,
        ui: &mut egui::Ui,
        library: &super::library::Library,
        indices: &Arc<Vec<usize>>,
        selected_id: Option<i64>,
    ) {
        self.browse_play_summary(ui, indices.len());
        if indices.is_empty() {
            self.browse_play_empty(ui, library.games.is_empty());
            return;
        }

        let reset = std::mem::take(&mut self.browse_play.reset_scroll);
        let list = self.filter.list;
        let scroll_height = ui.available_height().max(160.0);
        let spacing = ui.spacing().item_spacing;
        let usable = (ui.available_width() - 14.0).max(CARD_WIDTH);
        let columns = if list {
            1
        } else {
            (((usable + spacing.x) / (CARD_WIDTH + spacing.x)).floor() as usize).max(1)
        };
        // Cards share the row evenly, so the shelf has no ragged gap on its right.
        let card_width = if list {
            usable
        } else {
            ((usable - spacing.x * (columns as f32 - 1.0)) / columns as f32)
                .clamp(CARD_WIDTH, CARD_MAX_WIDTH)
        };
        let rows = indices.len().div_ceil(columns);
        let row_height = if list { LIST_ROW_HEIGHT } else { CARD_HEIGHT };
        let mut cover_note: Option<String> = None;
        let mut area = egui::ScrollArea::vertical()
            .id_salt("v2_browse_play_shelf")
            .auto_shrink([false, false])
            .max_height(scroll_height);
        if reset {
            area = area.vertical_scroll_offset(0.0);
        }
        area.show_rows(ui, row_height, rows, |ui, range| {
            for row in range {
                let start = row * columns;
                let end = (start + columns).min(indices.len());
                ui.horizontal_top(|ui| {
                    for position in start..end {
                        let Some(game) = library.games.get(indices[position]) else {
                            continue;
                        };
                        let selected = selected_id == Some(game.archive.id);
                        if list {
                            self.browse_play_list_row(ui, game, selected, &mut cover_note);
                        } else {
                            self.browse_play_card(ui, game, selected, card_width, &mut cover_note);
                        }
                    }
                });
            }
        });
        self.browse_play.cover_note = cover_note;
    }

    /// "392 games matching "005" in Arcade" with one-click removal of each part.
    fn browse_play_summary(&mut self, ui: &mut egui::Ui, count: usize) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} {}",
                    thousands(count),
                    if count == 1 { "game" } else { "games" }
                ))
                .strong(),
            );
            let search = self.filter.search.trim().to_owned();
            if !search.is_empty()
                && ui
                    .small_button(format!("matching \"{search}\"  ×"))
                    .on_hover_text("Clear the search")
                    .clicked()
            {
                self.filter.search.clear();
            }
            if !self.filter.platform.is_empty()
                && ui
                    .small_button(format!("in {}  ×", self.filter.platform))
                    .on_hover_text("Show all systems")
                    .clicked()
            {
                self.filter.select_platform(String::new());
            }
            if let Some(note) = self.browse_play.cover_note.as_deref() {
                ui.label(
                    RichText::new(format!("Some covers cannot load right now: {note}"))
                        .size(theme::METADATA_SIZE)
                        .color(theme::SECONDARY_TEXT),
                );
            }
        });
        ui.add_space(theme::SPACE_XS);
    }

    fn card_background(ui: &egui::Ui, rect: egui::Rect, selected: bool, response: &egui::Response) {
        let hovered = response.hovered();
        let fill = if selected || hovered {
            theme::RAISED_SURFACE
        } else {
            theme::CARD_SURFACE
        };
        ui.painter().rect_filled(rect, 10.0, fill);
        let stroke = if selected {
            egui::Stroke::new(2.0_f32, theme::PRIMARY_ACTION_HOVER)
        } else if hovered {
            egui::Stroke::new(1.0_f32, theme::BORDER_FOCUS)
        } else {
            egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE)
        };
        ui.painter()
            .rect_stroke(rect, 10.0, stroke, egui::StrokeKind::Inside);
        theme::paint_focus_ring(ui, response, rect, 10.0);
    }

    fn note_cover_problem(&self, game: &Game, note: &mut Option<String>) {
        if note.is_none() {
            let key = self.artwork.key(game.archive.id, Kind::Cover);
            if let Some(Picture::Unavailable { issue, .. }) = self.artwork.pictures.get(&key) {
                *note = Some(issue.plain_message().to_string());
            }
        }
    }

    /// A whole-card button: picture, two-line title, and "system · status".
    fn browse_play_card(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        selected: bool,
        card_width: f32,
        note: &mut Option<String>,
    ) {
        let id = game.archive.id;
        ui.push_id(("v2_browse_play_game", id), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(card_width, CARD_HEIGHT), egui::Sense::click());
            if !ui.is_rect_visible(rect) {
                return;
            }
            Self::card_background(ui, rect, selected, &response);
            let inner = rect.shrink(CARD_PAD);
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            child.set_clip_rect(rect.intersect(ui.clip_rect()));
            self.picture_tile(
                &mut child,
                game,
                Kind::Cover,
                egui::vec2(inner.width(), COVER_HEIGHT),
            );
            self.note_cover_problem(game, note);
            child.add_space(6.0);
            child.add(
                egui::Label::new(two_line_title(&game.title, inner.width())).selectable(false),
            );
            child.add(egui::Label::new(status_line(game, inner.width())).selectable(false));
            let response = response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("{}\n{}", game.title, game.platform));
            if response.clicked() {
                self.go(Route::BrowsePlayGame(id));
            }
        });
    }

    fn browse_play_list_row(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        selected: bool,
        note: &mut Option<String>,
    ) {
        let id = game.archive.id;
        ui.push_id(("v2_browse_play_game", id), |ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), LIST_ROW_HEIGHT - 8.0),
                egui::Sense::click(),
            );
            if !ui.is_rect_visible(rect) {
                return;
            }
            Self::card_background(ui, rect, selected, &response);
            let inner = rect.shrink(CARD_PAD);
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            self.picture_tile(&mut child, game, Kind::Cover, LIST_COVER_SIZE);
            self.note_cover_problem(game, note);
            let text_width = (inner.width() - LIST_COVER_SIZE.x - 12.0).max(80.0);
            child.vertical(|ui| {
                ui.set_width(text_width);
                ui.add(
                    egui::Label::new(
                        RichText::new(&game.title)
                            .strong()
                            .color(theme::PRIMARY_TEXT),
                    )
                    .truncate()
                    .selectable(false),
                );
                ui.add(egui::Label::new(status_line(game, text_width)).selectable(false));
            });
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                self.go(Route::BrowsePlayGame(id));
            }
        });
    }

    /// The selected game. Full panel beside the shelf, or a compact strip when
    /// the window is narrow. Play is always the first thing to see.
    fn browse_play_selected(&mut self, ui: &mut egui::Ui, game: &Game, compact: bool) {
        let id = game.archive.id;
        egui::Frame::new()
            .fill(theme::CARD_SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.push_id(("v2_browse_play_selected", id), |ui| {
                    // Widths are fixed before anything is drawn: a widget that
                    // does not fit would otherwise widen the panel and every
                    // width computed after it.
                    let inner = ui.available_width();
                    ui.set_width(inner);
                    let gap = ui.spacing().item_spacing.x;
                    let cover = if compact {
                        COMPACT_COVER_SIZE
                    } else {
                        SELECTED_COVER_SIZE
                    };
                    let text_width = (inner - cover.x - gap).max(120.0);
                    let heading = |this: &mut Self, ui: &mut egui::Ui| {
                        ui.horizontal_top(|ui| {
                            this.picture_tile(ui, game, Kind::Cover, cover);
                            ui.vertical(|ui| {
                                ui.set_width(text_width);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&game.title)
                                            .size(theme::SECTION_TITLE_SIZE)
                                            .strong()
                                            .color(theme::PRIMARY_TEXT),
                                    )
                                    .wrap(),
                                );
                                ui.label(
                                    RichText::new(&game.platform).color(theme::SECONDARY_TEXT),
                                );
                                let (status, colour) = status_text(game);
                                ui.label(RichText::new(status).strong().color(colour));
                                if compact {
                                    Self::play_row(this, ui, id, text_width);
                                }
                            });
                        });
                    };
                    if compact {
                        heading(self, ui);
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("v2_browse_play_detail")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(inner - 12.0);
                            heading(self, ui);
                            ui.add_space(theme::SPACE_SM);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(readiness_sentence(game))
                                        .size(theme::METADATA_SIZE)
                                        .color(theme::SECONDARY_TEXT),
                                )
                                .wrap(),
                            );
                            ui.add_space(theme::SPACE_SM);
                            Self::play_row(self, ui, id, inner - 12.0);
                            ui.add_space(theme::SPACE_LG);
                            ui.label(RichText::new("More for this game").strong());
                            for (label, route) in browse_play_contextual_routes(id) {
                                ui.push_id(("v2_browse_play_action", id, label), |ui| {
                                    if ui
                                        .add_sized([inner - 12.0, 34.0], egui::Button::new(label))
                                        .clicked()
                                    {
                                        self.go(route.clone());
                                    }
                                });
                            }
                            ui.add_space(theme::SPACE_SM);
                            ui.collapsing("Advanced details", |ui| {
                                self.artwork_provider_details(ui, id);
                            });
                        });
                });
            });
    }

    /// Play (primary) and Game details, sized to the given width.
    fn play_row(this: &mut Self, ui: &mut egui::Ui, id: i64, width: f32) {
        let gap = ui.spacing().item_spacing.x;
        let play = (width * 0.42).clamp(96.0, 150.0);
        ui.horizontal(|ui| {
            if ui
                .add_sized(
                    [play, 40.0],
                    egui::Button::new(RichText::new("Play").strong()).fill(theme::PRIMARY_ACTION),
                )
                .clicked()
            {
                this.go(Route::Task {
                    section: Section::Launch,
                    game: id,
                });
            }
            if ui
                .add_sized(
                    [(width - play - gap).max(90.0), 40.0],
                    egui::Button::new("Game details"),
                )
                .clicked()
            {
                this.go(Route::Game(id));
            }
        });
    }

    fn selected_game_prompt(ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::CARD_SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Pick a game").size(theme::SECTION_TITLE_SIZE).strong());
                ui.label(
                    RichText::new("Choose a game on the left to see its cover and whether it is ready to play.")
                        .color(theme::SECONDARY_TEXT),
                );
            });
    }

    fn browse_play_empty(&mut self, ui: &mut egui::Ui, library_empty: bool) {
        egui::Frame::new()
            .fill(theme::CARD_SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if library_empty && !self.loaded {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Loading your game list…").size(theme::SECTION_TITLE_SIZE).strong());
                    });
                    ui.label("This can take a moment for a large library. Nothing is being changed.");
                } else if library_empty {
                    ui.label(RichText::new("No games have been added yet").size(theme::SECTION_TITLE_SIZE).strong());
                    ui.label("Add a game folder in Sources & Providers, then scan it into the existing library.");
                    if browse_primary(ui, "Open Sources") {
                        self.go(Route::Section(Section::Sources));
                    }
                } else {
                    ui.label(RichText::new("No games match").size(theme::SECTION_TITLE_SIZE).strong());
                    ui.label("Try a different search or system, or clear what you have chosen.");
                    if ui.button("Clear search and system").clicked() {
                        self.filter = Filter {
                            list: self.filter.list,
                            ..Default::default()
                        };
                    }
                }
            });
    }
}
