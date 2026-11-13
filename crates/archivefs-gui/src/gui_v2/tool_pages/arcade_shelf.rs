//! Virtual Arcade Shelf: one horizontal row of covers for each system.
//!
//! A read-only presentation over the canonical GUI-v2 library. It owns no
//! catalogue, artwork or launch state: rows come from `Library::by_platform`,
//! covers from the shared artwork pipeline in its local-only mode (local
//! artwork and covers already in the disk cache; never a RomM request or a new
//! cache entry, whatever the Online artwork setting), and opening a game hands
//! off to the existing Game Details route. Rows are virtualised vertically and covers
//! horizontally, so only visible covers are laid out or requested and a very
//! large library costs no more per frame than a small one.

use eframe::egui::{self, RichText};

use super::super::{
    App,
    artwork::Picture,
    browse_play::thousands,
    library::{Game, Library},
    media_sources::Kind,
    routes::{Route, Section},
};
use crate::ui::theme;

const COVER: egui::Vec2 = egui::vec2(120.0, 160.0);
const TILE_PAD: f32 = 6.0;
const TITLE_HEIGHT: f32 = 22.0;
const TILE: egui::Vec2 = egui::vec2(
    COVER.x + 2.0 * TILE_PAD,
    COVER.y + TITLE_HEIGHT + 3.0 * TILE_PAD,
);
const GAP: f32 = 12.0;
const STRIDE: f32 = TILE.x + GAP;
const ROW_LABEL: f32 = 28.0;
/// Room under the covers for the row's horizontal scroll bar.
const ROW_SLACK: f32 = 14.0;
const ROW_HEIGHT: f32 = ROW_LABEL + TILE.y + ROW_SLACK;

/// Page state. The cursor names a game, so it survives a library reload and is
/// dropped only when that game is no longer listed.
#[derive(Default)]
pub(in crate::gui_v2) struct State {
    cursor: Option<Cursor>,
    /// Bring the cursor's cover into view on the next frame.
    reveal: bool,
    /// Give the cursor's cover keyboard focus on the next frame.
    focus: bool,
    /// Whether a shelf cover held keyboard focus last frame.
    tile_focused: bool,
    /// Last vertical scroll offset and viewport height of the shelf.
    view: Option<(f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cursor {
    row: usize,
    col: usize,
    game: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::gui_v2) enum Move {
    Left,
    Right,
    Up,
    Down,
    RowStart,
    RowEnd,
}

/// One shelf per system, in the library's platform order; each row lists
/// library indices in title order. Systems with no games have no shelf.
fn shelf_rows(library: &Library) -> Vec<(&str, &[usize])> {
    library
        .by_platform
        .iter()
        .filter(|(_, games)| !games.is_empty())
        .map(|(platform, games)| (platform.as_str(), games.as_slice()))
        .collect()
}

/// Where a keyboard move lands. `lengths` are the row lengths (never zero);
/// with no cursor yet, any move selects the first cover. Up and Down keep the
/// column where the next row is long enough, otherwise its last cover.
pub(in crate::gui_v2) fn step(
    lengths: &[usize],
    at: Option<(usize, usize)>,
    to: Move,
) -> Option<(usize, usize)> {
    if lengths.is_empty() || lengths.contains(&0) {
        return None;
    }
    let Some((row, col)) = at else {
        return Some((0, 0));
    };
    let row = row.min(lengths.len() - 1);
    let col = col.min(lengths[row] - 1);
    let clamp = |row: usize| (row, col.min(lengths[row] - 1));
    Some(match to {
        Move::Left => (row, col.saturating_sub(1)),
        Move::Right => (row, (col + 1).min(lengths[row] - 1)),
        Move::Up => clamp(row.saturating_sub(1)),
        Move::Down => clamp((row + 1).min(lengths.len() - 1)),
        Move::RowStart => (row, 0),
        Move::RowEnd => (row, lengths[row] - 1),
    })
}

/// The cursor checked against the current rows: kept when it still points at
/// its game, moved to wherever that game now is, dropped when it is gone.
fn resolve(rows: &[(&str, &[usize])], library: &Library, cursor: Cursor) -> Option<Cursor> {
    let at = |row: usize, col: usize| {
        rows.get(row)
            .and_then(|(_, games)| games.get(col))
            .and_then(|&index| library.games.get(index))
            .map(|game| game.archive.id)
    };
    if at(cursor.row, cursor.col) == Some(cursor.game) {
        return Some(cursor);
    }
    let &index = library.by_id.get(&cursor.game)?;
    let platform = library.games.get(index)?.platform.as_str();
    let row = rows.iter().position(|(name, _)| *name == platform)?;
    let col = rows[row].1.iter().position(|&game| game == index)?;
    Some(Cursor {
        row,
        col,
        game: cursor.game,
    })
}

fn tile_id(game: i64) -> egui::Id {
    egui::Id::new(("v2_arcade_shelf_cover", game))
}

/// Shelf keys, read only when no text field, dialog or menu owns the keys and
/// focus is either nowhere or on a cover, so a focused button elsewhere keeps
/// its keys.
fn read_keys(ctx: &egui::Context, tile_focused: bool) -> (Option<Move>, bool) {
    let focused_elsewhere = ctx.memory(|memory| memory.focused().is_some()) && !tile_focused;
    if !crate::gui_v2::shortcuts::page_keys_allowed(ctx) || focused_elsewhere {
        return (None, false);
    }
    let (moved, open) = ctx.input_mut(|input| {
        let none = egui::Modifiers::NONE;
        let keys = [
            (egui::Key::ArrowLeft, Move::Left),
            (egui::Key::ArrowRight, Move::Right),
            (egui::Key::ArrowUp, Move::Up),
            (egui::Key::ArrowDown, Move::Down),
            (egui::Key::Home, Move::RowStart),
            (egui::Key::End, Move::RowEnd),
        ];
        let mut moved = None;
        for (key, to) in keys {
            if input.consume_key(none, key) {
                moved = Some(to);
            }
        }
        let open = input.consume_key(none, egui::Key::Enter);
        (moved, open)
    });
    if moved.is_some() {
        // egui would otherwise move focus to the nearest widget in the arrow's
        // direction at the end of the frame, away from the cover just chosen.
        ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
    }
    (moved, open)
}

enum TileEvent {
    Select,
    Open,
}

impl App {
    pub(in crate::gui_v2) fn arcade_shelf_page(&mut self, ui: &mut egui::Ui) {
        let library = self.library.clone();
        let rows = shelf_rows(&library);
        if rows.is_empty() {
            self.arcade_shelf_empty(ui);
            return;
        }
        // First open: prepare the shared cover index read-only (discovery
        // only, never the thumbnail-cache eviction), so no other page is needed.
        // A stopped index worker is asked once, not again on every frame.
        if self.loaded
            && self.artwork.index.is_none()
            && !self.artwork.index_loading
            && !self.artwork.index_unavailable()
        {
            self.artwork.reload_read_only(library.clone());
        }
        let mut state = std::mem::take(&mut self.tools.arcade_shelf);
        let mut cursor = state
            .cursor
            .and_then(|cursor| resolve(&rows, &library, cursor));

        let (moved, mut open) = read_keys(ui.ctx(), state.tile_focused);
        if let Some(to) = moved {
            let lengths: Vec<usize> = rows.iter().map(|(_, games)| games.len()).collect();
            if let Some((row, col)) = step(&lengths, cursor.map(|c| (c.row, c.col)), to) {
                cursor = Some(Cursor {
                    row,
                    col,
                    game: library.games[rows[row].1[col]].archive.id,
                });
                state.reveal = true;
                state.focus = true;
            }
        }

        ui.label(format!(
            "{} games on {} system shelves. Arrow keys move between covers; Enter or a double-click opens Game Details. Nothing here changes your library.",
            thousands(library.games.len()),
            thousands(rows.len()),
        ));
        ui.add_space(theme::SPACE_SM);
        let selected = cursor.and_then(|c| library.games.get(rows[c.row].1[c.col]));
        ui.horizontal_wrapped(|ui| match selected {
            Some(game) => {
                ui.label(RichText::new(&game.title).strong());
                ui.weak(format!("·  {}", game.platform));
                if ui.button("Open Game Details").clicked() {
                    open = true;
                }
            }
            None => {
                ui.weak("Choose a cover, or press an arrow key, to pick a game.");
            }
        });
        ui.add_space(theme::SPACE_SM);

        let stride_y = ROW_HEIGHT + ui.spacing().item_spacing.y;
        let height =
            (ui.clip_rect().bottom() - ui.cursor().top() - theme::SPACE_SM).max(ROW_HEIGHT * 1.5);
        let mut area = egui::ScrollArea::vertical()
            .id_salt("v2_arcade_shelf_rows")
            .auto_shrink([false, false])
            .max_height(height);
        if state.reveal
            && let Some(c) = cursor
        {
            let top = c.row as f32 * stride_y;
            let bottom = top + ROW_HEIGHT;
            let target = match state.view {
                Some((offset, _)) if top < offset => Some(top),
                Some((offset, view)) if bottom > offset + view => Some(bottom - view),
                Some(_) => None,
                None => Some(top),
            };
            if let Some(target) = target {
                area = area.vertical_scroll_offset(target.max(0.0));
            }
        }
        let mut event = None;
        let mut tile_focused = false;
        let output = area.show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
            for row in range {
                let (name, games) = rows[row];
                let target = cursor.filter(|c| c.row == row);
                if let Some((col, happened, focused)) =
                    self.arcade_shelf_row(ui, &library, name, games, target, &mut state)
                {
                    tile_focused |= focused;
                    if happened.is_some() {
                        event = Some((row, col, happened));
                    }
                }
            }
        });
        state.view = Some((output.state.offset.y, output.inner_rect.height()));
        state.reveal = false;
        state.focus = false;

        if let Some((row, col, happened)) = event {
            cursor = Some(Cursor {
                row,
                col,
                game: library.games[rows[row].1[col]].archive.id,
            });
            if matches!(happened, Some(TileEvent::Open)) {
                open = true;
            }
        }
        state.tile_focused = tile_focused
            || cursor.is_some_and(|c| {
                ui.ctx()
                    .memory(|memory| memory.focused() == Some(tile_id(c.game)))
            });
        state.cursor = cursor;
        self.tools.arcade_shelf = state;
        if open && let Some(c) = cursor {
            self.go(Route::Game(c.game));
        }
    }

    /// One system shelf. Only covers inside the visible part of the row (and
    /// the cursor's cover while it is being revealed) are laid out. Returns the
    /// column of a cover that was clicked or focused, what happened, and
    /// whether a cover of this row holds focus.
    fn arcade_shelf_row(
        &mut self,
        ui: &mut egui::Ui,
        library: &Library,
        name: &str,
        games: &[usize],
        target: Option<Cursor>,
        state: &mut State,
    ) -> Option<(usize, Option<TileEvent>, bool)> {
        let mut result: Option<(usize, Option<TileEvent>, bool)> = None;
        ui.push_id(("v2_arcade_shelf_row", name), |ui| {
            ui.set_height(ROW_HEIGHT);
            ui.horizontal(|ui| {
                ui.label(theme::section_title(if name.trim().is_empty() {
                    "Unrecognised system"
                } else {
                    name
                }));
                ui.weak(match games.len() {
                    1 => "1 game".to_string(),
                    count => format!("{} games", thousands(count)),
                });
            });
            egui::ScrollArea::horizontal()
                .id_salt("covers")
                .auto_shrink([false, false])
                .max_height(TILE.y)
                .show_viewport(ui, |ui, viewport| {
                    let total = egui::vec2(games.len() as f32 * STRIDE - GAP, TILE.y);
                    let (content, _) = ui.allocate_exact_size(total, egui::Sense::hover());
                    let (first, last) =
                        visible_columns(viewport.min.x, viewport.max.x, games.len());
                    let mut columns: Vec<usize> = (first..last).collect();
                    if let Some(c) = target
                        && state.reveal
                        && !columns.contains(&c.col)
                    {
                        columns.push(c.col);
                    }
                    for col in columns {
                        let Some(game) = games.get(col).and_then(|&i| library.games.get(i)) else {
                            continue;
                        };
                        let rect = egui::Rect::from_min_size(
                            content.min + egui::vec2(col as f32 * STRIDE, 0.0),
                            TILE,
                        );
                        let selected = target.is_some_and(|c| c.col == col);
                        let (happened, focused) =
                            self.arcade_shelf_tile(ui, rect, game, selected, state);
                        if happened.is_some() || (focused && result.is_none()) {
                            result = Some((col, happened, focused));
                        }
                    }
                });
        });
        result
    }

    fn arcade_shelf_tile(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        game: &Game,
        selected: bool,
        state: &State,
    ) -> (Option<TileEvent>, bool) {
        let response = ui.interact(rect, tile_id(game.archive.id), egui::Sense::click());
        if selected && state.focus {
            response.request_focus();
        }
        if selected && state.reveal {
            ui.scroll_to_rect(rect, None);
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &game.title)
        });
        if ui.is_rect_visible(rect) {
            let blue = theme::blue_profile(ui);
            let fill = if selected && blue {
                theme::blue::SELECTED_FILL
            } else if selected || response.hovered() {
                theme::raised_fill(ui)
            } else {
                theme::card_fill(ui)
            };
            ui.painter().rect_filled(rect, 10.0, fill);
            let stroke = if selected && blue {
                egui::Stroke::new(2.0_f32, theme::blue::SELECTED_OUTLINE)
            } else if selected {
                egui::Stroke::new(2.0_f32, theme::PRIMARY_ACTION_HOVER)
            } else if response.hovered() {
                egui::Stroke::new(1.0_f32, theme::BORDER_FOCUS)
            } else {
                theme::border(ui)
            };
            ui.painter()
                .rect_stroke(rect, 10.0, stroke, egui::StrokeKind::Inside);
            theme::paint_focus_ring(ui, &response, rect, 10.0);
            let clip = rect.intersect(ui.clip_rect());
            let inner = rect.shrink(TILE_PAD);
            let cover = egui::Rect::from_min_size(inner.min, COVER);
            let title = egui::Rect::from_min_max(
                egui::pos2(inner.left(), cover.bottom() + TILE_PAD),
                inner.max,
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(cover));
            child.set_clip_rect(clip);
            self.arcade_shelf_cover(&mut child, game);
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(title)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            child.set_clip_rect(clip);
            child.add(
                egui::Label::new(RichText::new(&game.title).small())
                    .truncate()
                    .selectable(false),
            );
        }
        let response = response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!("{}\n{}", game.title, game.platform));
        let happened = if response.double_clicked() {
            Some(TileEvent::Open)
        } else if response.clicked() || response.gained_focus() {
            Some(TileEvent::Select)
        } else {
            None
        };
        (happened, response.has_focus())
    }

    /// The game's cover from local artwork or the disk cache only, read-only,
    /// or a short label saying why there is no picture. Off-screen covers
    /// are never requested.
    fn arcade_shelf_cover(&mut self, ui: &mut egui::Ui, game: &Game) {
        let (rect, _) = ui.allocate_exact_size(COVER, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let key = self.artwork.request_local(game.archive.id, Kind::Cover);
        ui.painter().rect_filled(rect, 8.0, theme::deep_fill(ui));
        match self.artwork.pictures.get(&key) {
            Some(Picture::Ready { texture, .. }) => {
                let size = texture.size_vec2();
                let factor = (COVER.x / size.x).min(COVER.y / size.y);
                ui.painter().image(
                    texture.id(),
                    egui::Rect::from_center_size(rect.center(), size * factor),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            picture => {
                let text = cover_label(
                    self.artwork.index.is_none(),
                    self.artwork.index_unavailable(),
                    self.artwork.paused,
                    picture,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(12.0),
                    ui.visuals().weak_text_color(),
                );
            }
        }
    }

    fn arcade_shelf_empty(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::card_fill(ui))
            .stroke(theme::border(ui))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !self.loaded {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(theme::section_title("Loading your game list…"));
                    });
                    ui.label("This can take a moment for a large library. Nothing is being changed.");
                } else {
                    ui.label(theme::section_title("Your shelf is empty"));
                    ui.label("No games have been added yet. Add a game folder in Sources & Providers, then scan it into the existing library.");
                    if ui.button("Open Sources").clicked() {
                        self.go(Route::Section(Section::Sources));
                    }
                }
            });
    }
}

/// What a cover without a picture says. Only a cover the index has no local
/// picture for is "No local cover"; every other state says what it is.
/// `preparing`: no index yet; `unavailable`: none can be built this session.
fn cover_label(
    preparing: bool,
    unavailable: bool,
    paused: bool,
    picture: Option<&Picture>,
) -> &'static str {
    match picture {
        Some(Picture::Ready { .. }) => "",
        Some(Picture::Loading) => "Loading…",
        Some(Picture::Missing) => "No local cover",
        // Local-only: a cache miss is `Missing`, so this is a file that could
        // not be read or decoded. It is retried after a while.
        Some(Picture::Failed(_)) => "Cover unreadable",
        Some(Picture::Unavailable { .. }) => "Cover unavailable",
        None if unavailable => "Covers unavailable",
        None if preparing => "Finding covers…",
        None if paused => "Covers paused",
        // Not yet queued, for example while the bounded request queue is full.
        None => "Waiting…",
    }
}

/// The half-open range of columns at least partly inside `[min_x, max_x)`.
fn visible_columns(min_x: f32, max_x: f32, len: usize) -> (usize, usize) {
    let first = ((min_x / STRIDE).floor().max(0.0) as usize).min(len);
    let last = ((max_x / STRIDE).ceil().max(0.0) as usize + 1).min(len);
    (first, last.max(first))
}

#[cfg(test)]
#[path = "arcade_shelf_tests.rs"]
mod tests;
