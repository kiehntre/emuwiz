//! Region 4: the browsing rail.
//!
//! The practical library browser, kept visually subordinate to the
//! selected-game stage above it: a wrapping grid of compact cards - small
//! media, title, and a one-line "platform \u{b7} status" - that still supports
//! dense browsing rather than becoming a cover wall. It reuses the exact
//! virtualisation and bounded cover-scheduling the old single-column list
//! had, so a 13,891-record library still only ever asks about what is on
//! screen plus a small look-ahead.

use eframe::egui;
use std::path::{Path, PathBuf};

use super::alpha_jump::{ALPHA_BUCKETS, AlphaJumpIndex, bucket_label};
use super::{
    GamerLibrarySnapshot, GamerViewAction, GamerViewScreen, gamer_display_title,
    gamer_empty_list_guidance, gamer_view_row_state_label,
};
use crate::gamer_artwork::{self, COVER_BOX, CoverSlot};
use crate::ui::platform_artwork::{
    GameRowArtworkPaint, PlatformArtworkCache, paint_cover_fitted, paint_game_row_artwork,
    platform_asset_id, platform_fallback_asset_id,
};
use crate::ui::{components as widgets, theme};
use crate::{ArchiveContext, LoadedData};

/// Height of one browsing-rail card.
const CARD_HEIGHT: f32 = 78.0;
/// Gap between cards, both axes.
const CARD_GAP: f32 = 12.0;

pub(crate) struct RailContext<'a> {
    pub(crate) data: &'a LoadedData,
    pub(crate) snapshot: &'a GamerLibrarySnapshot,
    pub(crate) archive_context: &'a mut ArchiveContext,
    pub(crate) screen: &'a mut GamerViewScreen,
    pub(crate) covers: &'a mut gamer_artwork::GamerCoverCache,
    pub(crate) cover_requests: &'a mut Vec<gamer_artwork::CoverJob>,
    pub(crate) artwork_directory: Option<&'a Path>,
    pub(crate) artwork_cache: &'a mut PlatformArtworkCache,
    pub(crate) columns: usize,
    pub(crate) min_height: f32,
    pub(crate) searching: bool,
    pub(crate) platform_selected: bool,
    /// The live search text, bound directly by the search field drawn here -
    /// the same text the top bar's own search box edits (see
    /// `gamer_view::show_gamer_view`), so either one narrows the same
    /// result set and neither can disagree with the other.
    pub(crate) filter: &'a mut String,
    /// The A-Z jump strip's index over the currently visible result set -
    /// rebuilt here (cheaply, only on an actual change) and also read back
    /// by the caller to draw the strip itself.
    pub(crate) alpha_jump: &'a mut AlphaJumpIndex,
    /// Document-space selected-stage anchor. Used only after an explicit
    /// library-card activation, never while search text or A-Z changes.
    pub(crate) selected_stage_rect: egui::Rect,
}

/// Draws the browsing rail. Returns an action only for the empty-library
/// first-run affordance; ordinary selection is written straight into
/// `archive_context`.
pub(crate) fn show_browsing_rail(
    ui: &mut egui::Ui,
    cx: RailContext<'_>,
) -> Option<GamerViewAction> {
    let RailContext {
        data,
        snapshot,
        archive_context,
        screen,
        covers,
        cover_requests,
        artwork_directory,
        artwork_cache,
        columns,
        min_height,
        searching,
        platform_selected,
        filter,
        alpha_jump,
        selected_stage_rect,
    } = cx;

    let columns = columns.max(1);

    // Rebuilt only when the result set actually changed - see
    // `AlphaJumpIndex::refresh`. This is also what gives the grid a stable
    // alphabetical order: the jump strip and the "current letter as you
    // scroll" highlight only make sense against the same order the cards
    // are drawn in.
    alpha_jump.refresh(&snapshot.visible, &data.records);

    // Snapshot the sorted order before `alpha_jump` needs to be borrowed
    // mutably again below (`report_first_visible_position`) - `sorted()`
    // otherwise stays borrowed for as long as `order` is used.
    let order: Vec<usize> = alpha_jump.sorted().to_vec();

    // The header block - "YOUR LIBRARY", the search field and the A-Z jump
    // strip - is drawn with a tightened vertical rhythm, and the search
    // field shares "YOUR LIBRARY"'s own row rather than taking one of its
    // own. The rail's vertical budget has no slack for extra rows at the
    // shortest supported window - the grid itself only just clears its "at
    // least two full rows" floor already - so this adds exactly one new
    // row (the letters) rather than two. The grid's own `CARD_GAP` is
    // unaffected; only this header region tightens up.
    let jump_target = ui
        .scope(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().button_padding = egui::vec2(2.0, 1.0);

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("YOUR LIBRARY")
                        .size(theme::SECTION_TITLE_SIZE)
                        .strong()
                        .color(theme::PRIMARY_TEXT),
                );
                ui.label(
                    egui::RichText::new(format!("{}", order.len()))
                        .size(theme::SECTION_TITLE_SIZE)
                        .color(theme::SECONDARY_TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let width = (ui.available_width()).min(220.0).max(100.0);
                    ui.add_sized(
                        egui::vec2(width, 18.0),
                        egui::TextEdit::singleline(filter).hint_text("Search games..."),
                    );
                });
            });

            draw_alpha_strip(ui, alpha_jump)
        })
        .inner;

    if order.is_empty() {
        // The empty-library first-run invitation lives in the stage; here we
        // only explain a search/platform filter that currently matches
        // nothing.
        if !data.rows.is_empty() {
            ui.weak(gamer_empty_list_guidance(
                false,
                searching,
                platform_selected,
            ));
            if searching && !filter.is_empty() {
                ui.add_space(theme::SPACE_XS);
                if widgets::action_button(ui, "Clear search", widgets::ActionStyle::Quiet, true)
                    .clicked()
                {
                    filter.clear();
                }
            }
        }
        return None;
    }

    let grid_rows = order.len().div_ceil(columns);
    let row_stride = CARD_HEIGHT + CARD_GAP;
    let selected_path = archive_context.focused.clone();

    // The page-level ScrollArea owns scrolling now. Reserve the complete
    // grid height in that document, then paint only rows intersecting its
    // clip rect. This keeps 68k-game libraries bounded without creating a
    // nested vertical viewport.
    let grid_origin = ui.cursor().min;
    if let Some(position) = jump_target {
        let target = egui::Rect::from_min_size(
            grid_origin + egui::vec2(0.0, scroll_offset_for_item(position, columns)),
            egui::vec2(ui.available_width(), CARD_HEIGHT),
        );
        ui.scroll_to_rect(target, Some(egui::Align::TOP));
    }
    let total_height = (grid_rows as f32 * row_stride).max(min_height.max(120.0));
    ui.allocate_space(egui::vec2(ui.available_width(), total_height));

    let clip = ui.clip_rect();
    let row_range = visible_grid_rows(grid_origin.y, clip.min.y, clip.max.y, grid_rows, row_stride);
    if row_range.start < row_range.end {
        alpha_jump.report_first_visible_position(row_range.start * columns);
        schedule_covers(
            covers,
            cover_requests,
            data,
            &order,
            &row_range,
            columns,
            selected_path.as_deref(),
        );

        for grid_row in row_range {
            let first = grid_row * columns;
            let last = ((grid_row + 1) * columns).min(order.len());
            let row_rect = egui::Rect::from_min_size(
                grid_origin + egui::vec2(0.0, grid_row as f32 * row_stride),
                egui::vec2(ui.available_width(), CARD_HEIGHT),
            );
            let mut row_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(row_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            let available = row_ui.available_width();
            let card_width = ((available - CARD_GAP * (columns.saturating_sub(1)) as f32)
                / columns as f32)
                .max(120.0);
            row_ui.horizontal(|ui| {
                for (offset, &index) in order[first..last].iter().enumerate() {
                    if offset > 0 {
                        ui.add_space(CARD_GAP);
                    }
                    let record = &data.records[index];
                    let row = &data.rows[index];
                    let selected = archive_context.focused.as_deref() == Some(row.path.as_path());
                    let title = gamer_display_title(record);
                    let state_label =
                        gamer_view_row_state_label(row.unknown_platform, record.mount_state);
                    if draw_card(
                        ui,
                        CardArgs {
                            width: card_width,
                            selected,
                            title: &title,
                            platform: &row.platform,
                            unknown_platform: row.unknown_platform,
                            state_label,
                            cover: covers.slot_for(row.path.as_path(), None),
                            artwork_cache,
                            artwork_directory,
                        },
                    ) {
                        archive_context.select_only(row.path.clone());
                        *screen = GamerViewScreen::GameList;
                        // Selection should reveal the thing it changed: the
                        // selected-game stage. This is a one-shot request in
                        // the activation branch, so ordinary scrolling and
                        // search edits remain entirely user-controlled.
                        if let Some(target) = selection_scroll_target(selected_stage_rect, true) {
                            ui.scroll_to_rect(target, Some(egui::Align::TOP));
                        }
                    }
                }
            });
        }
    }

    None
}

/// Returns the selected-stage target only for an explicit game activation.
/// Keeping this gate separate makes it impossible for filter edits or A-Z
/// navigation to reuse the stage target accidentally.
fn selection_scroll_target(
    selected_stage_rect: egui::Rect,
    game_activated: bool,
) -> Option<egui::Rect> {
    game_activated.then_some(selected_stage_rect)
}

/// Draws the A-Z jump strip and returns the jump target (a position within
/// `alpha_jump.sorted()`) when a letter was clicked or keyboard/gamepad
/// activated this frame. The search field lives inline in "YOUR LIBRARY"'s
/// own row instead - see the call site - so this is the only *new* row the
/// header block adds.
///
/// Uses plain `egui` widgets (`Button` inside `add_enabled`), so Tab reaches
/// each letter in the normal focus order and Enter/Space activates a
/// focused one exactly as it does any other button in Gamer View - no
/// separate input system.
fn draw_alpha_strip(ui: &mut egui::Ui, alpha_jump: &AlphaJumpIndex) -> Option<usize> {
    // The strip is a library control, so use the whole row rather than
    // leaving its 27 buttons bunched at the left edge. On narrow windows the
    // same deterministic cells wrap cleanly instead of shrinking below a
    // readable hit target.
    let layout = alpha_strip_layout(ui.available_width());

    let mut jump_target = None;
    let current = alpha_jump.current_bucket();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(layout.gap, 3.0);
        for bucket in 0..ALPHA_BUCKETS {
            let label = bucket_label(bucket);
            let enabled = alpha_jump.is_enabled(bucket);
            let is_current = enabled && current == Some(bucket);
            let (fill, stroke) = if is_current {
                // Restrained: a faint teal wash and a calm stroke, never the
                // bright, whole-strip amber a "selected" card uses elsewhere.
                (
                    theme::TEAL.gamma_multiply(0.22),
                    egui::Stroke::new(1.0_f32, theme::TEAL),
                )
            } else {
                (
                    theme::CARD_SURFACE,
                    egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE),
                )
            };
            let button = egui::Button::new(
                egui::RichText::new(label)
                    .size(theme::TECHNICAL_SIZE)
                    .strong(),
            )
            .min_size(egui::vec2(layout.cell_width, 20.0))
            .fill(fill)
            .stroke(stroke);
            let hover = if enabled {
                format!("Jump to {label}")
            } else {
                format!("No titles starting with {label} in the current list")
            };
            if ui
                .add_enabled(enabled, button)
                .on_hover_text(hover)
                .clicked()
            {
                jump_target = alpha_jump.first_position_for(bucket);
            }
        }
    });

    jump_target
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AlphaStripLayout {
    cell_width: f32,
    gap: f32,
    columns: usize,
}

/// Deterministic sizing for the 27 alpha controls. The final row fills the
/// usable width on wide layouts; below that threshold, the same readable
/// cells wrap into as many columns as fit.
fn alpha_strip_layout(available_width: f32) -> AlphaStripLayout {
    const MIN_CELL_WIDTH: f32 = 20.0;
    const MAX_CELL_WIDTH: f32 = 44.0;
    const GAP: f32 = 4.0;
    // Leave a small rounding/stroke allowance so a mathematically exact
    // final cell does not wrap because egui's layout width is fractional.
    let width = (available_width - 2.0).max(MIN_CELL_WIDTH);
    let columns = ((width + GAP) / (MIN_CELL_WIDTH + GAP))
        .floor()
        .clamp(1.0, ALPHA_BUCKETS as f32) as usize;
    let cell_width = ((width - GAP * (columns.saturating_sub(1)) as f32) / columns as f32)
        .clamp(MIN_CELL_WIDTH, MAX_CELL_WIDTH);
    AlphaStripLayout {
        cell_width,
        gap: GAP,
        columns,
    }
}

/// Converts a jump target's position within the alphabetically sorted
/// visible list into the vertical scroll offset that puts its grid row at
/// the top of the rail's viewport. Accounts for the actual card height, the
/// gap between rows, and however many columns the current window fits -
/// never assumes one item per row.
pub(crate) fn scroll_offset_for_item(item_position: usize, columns: usize) -> f32 {
    let columns = columns.max(1);
    let row = item_position / columns;
    row as f32 * (CARD_HEIGHT + CARD_GAP)
}

/// Returns only the grid rows intersecting the page scroll viewport. The grid
/// has already reserved its full document height, so this is the bounded
/// rendering decision for each frame.
pub(crate) fn visible_grid_rows(
    grid_top: f32,
    clip_min_y: f32,
    clip_max_y: f32,
    row_count: usize,
    row_stride: f32,
) -> std::ops::Range<usize> {
    if row_count == 0 || row_stride <= 0.0 || clip_max_y <= grid_top {
        return 0..0;
    }
    let first = ((clip_min_y - grid_top) / row_stride).floor().max(0.0) as usize;
    let last = ((clip_max_y - grid_top) / row_stride).ceil().max(0.0) as usize;
    first.min(row_count)..last.min(row_count)
}

/// Bounded, viewport-driven cover scheduling - identical policy to the old
/// single-column list, just widened to a grid row's worth of items.
fn schedule_covers(
    covers: &mut gamer_artwork::GamerCoverCache,
    cover_requests: &mut Vec<gamer_artwork::CoverJob>,
    data: &LoadedData,
    visible: &[usize],
    row_range: &std::ops::Range<usize>,
    columns: usize,
    selected: Option<&Path>,
) {
    let flat_start = row_range.start * columns;
    let flat_end = (row_range.end * columns).min(visible.len());
    if flat_start >= flat_end {
        return;
    }
    let wanted = gamer_artwork::look_ahead_range(flat_start..flat_end, visible.len());
    let paths_for = |range: std::ops::Range<usize>| -> Vec<PathBuf> {
        range
            .map(|position| data.rows[visible[position]].path.clone())
            .collect()
    };
    let on_screen = paths_for(flat_start..flat_end);
    let ahead: Vec<PathBuf> = paths_for(wanted)
        .into_iter()
        .filter(|path| !on_screen.contains(path))
        .collect();
    cover_requests.extend(covers.visible(selected, &on_screen, &ahead));
}

struct CardArgs<'a> {
    width: f32,
    selected: bool,
    title: &'a str,
    platform: &'a str,
    unknown_platform: bool,
    state_label: &'a str,
    cover: Option<&'a CoverSlot>,
    artwork_cache: &'a mut PlatformArtworkCache,
    artwork_directory: Option<&'a Path>,
}

/// One compact card. Returns `true` when it was clicked.
fn draw_card(ui: &mut egui::Ui, args: CardArgs<'_>) -> bool {
    let CardArgs {
        width,
        selected,
        title,
        platform,
        unknown_platform,
        state_label,
        cover,
        artwork_cache,
        artwork_directory,
    } = args;

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, CARD_HEIGHT), egui::Sense::click());

    let fill = if selected {
        theme::RAISED_SURFACE
    } else {
        theme::CARD_SURFACE
    };
    let stroke = if selected {
        egui::Stroke::new(1.5_f32, theme::AMBER)
    } else {
        egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE)
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    ui.painter()
        .rect_stroke(rect, 8.0, stroke, egui::StrokeKind::Inside);

    let media_center = egui::pos2(rect.left() + 10.0 + COVER_BOX / 2.0, rect.center().y);
    match cover {
        Some(CoverSlot::Ready { texture, .. }) => {
            paint_cover_fitted(ui, texture, media_center, COVER_BOX);
        }
        _ => {
            let platform_asset = platform_asset_id(platform, unknown_platform);
            let platform_fallback = platform_fallback_asset_id(platform, unknown_platform);
            paint_game_row_artwork(
                ui,
                artwork_cache,
                artwork_directory,
                GameRowArtworkPaint {
                    center: media_center,
                    size: COVER_BOX,
                    title,
                    platform_asset: &platform_asset,
                    platform_fallback,
                },
            );
        }
    }

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 10.0 + COVER_BOX + 10.0, rect.top() + 8.0),
        egui::pos2(rect.right() - 10.0, rect.bottom() - 8.0),
    );
    let mut text_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    text_ui.add(
        egui::Label::new(
            egui::RichText::new(title)
                .size(theme::BODY_SIZE)
                .color(if selected {
                    theme::PRIMARY_TEXT
                } else {
                    theme::SECONDARY_TEXT
                }),
        )
        .truncate(),
    );
    text_ui.add(
        egui::Label::new(
            egui::RichText::new(format!("{platform} \u{b7} {state_label}"))
                .size(theme::TECHNICAL_SIZE)
                .color(theme::TECHNICAL_TEXT),
        )
        .truncate(),
    );

    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.left() + 3.0, rect.bottom()),
            ),
            2.0,
            theme::AMBER,
        );
    }

    // When there is no cover, the hover text still explains why - the same
    // wording the old single-column list surfaced.
    let response = match cover {
        Some(CoverSlot::None(reason)) => {
            response.on_hover_text(format!("{title}\n{}", reason.describe()))
        }
        _ => response,
    };

    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_scroll_target_is_one_shot_and_activation_only() {
        let stage = egui::Rect::from_min_size(egui::pos2(40.0, 900.0), egui::vec2(800.0, 360.0));
        assert_eq!(selection_scroll_target(stage, true), Some(stage));
        assert_eq!(selection_scroll_target(stage, false), None);
    }

    #[test]
    fn alpha_strip_fills_a_wide_library_row() {
        let layout = alpha_strip_layout(1000.0);
        assert_eq!(layout.columns, ALPHA_BUCKETS);
        let used =
            layout.cell_width * ALPHA_BUCKETS as f32 + layout.gap * (ALPHA_BUCKETS - 1) as f32;
        assert!(
            used >= 990.0,
            "wide strip left too much unused width: {used}"
        );
        assert!((20.0..=44.0).contains(&layout.cell_width));
    }

    #[test]
    fn alpha_strip_stays_sane_on_1440_and_wraps_when_narrow() {
        let wide = alpha_strip_layout(1400.0);
        assert_eq!(wide.columns, ALPHA_BUCKETS);
        assert_eq!(wide.cell_width, 44.0);

        let narrow = alpha_strip_layout(300.0);
        assert!(narrow.columns < ALPHA_BUCKETS);
        assert!(narrow.columns >= 2);
        assert!((20.0..=44.0).contains(&narrow.cell_width));
    }

    #[test]
    fn scroll_offset_is_zero_for_the_first_row_at_any_column_count() {
        for columns in [2, 3, 4, 5] {
            assert_eq!(scroll_offset_for_item(0, columns), 0.0);
            // Anything still in the first row (index < columns) stays row 0.
            assert_eq!(scroll_offset_for_item(columns - 1, columns), 0.0);
        }
    }

    #[test]
    fn scroll_offset_converts_item_position_to_the_correct_grid_row() {
        let row_stride = CARD_HEIGHT + CARD_GAP;
        // 3 columns: items 0-2 are row 0, 3-5 are row 1, 6-8 are row 2.
        assert_eq!(scroll_offset_for_item(3, 3), row_stride);
        assert_eq!(scroll_offset_for_item(5, 3), row_stride);
        assert_eq!(scroll_offset_for_item(6, 3), row_stride * 2.0);

        // 2 columns: item 7 is row 3 (7 / 2 = 3).
        assert_eq!(scroll_offset_for_item(7, 2), row_stride * 3.0);

        // 4 columns: item 11 is row 2 (11 / 4 = 2).
        assert_eq!(scroll_offset_for_item(11, 4), row_stride * 2.0);

        // 5 columns: item 24 is row 4 (24 / 5 = 4).
        assert_eq!(scroll_offset_for_item(24, 5), row_stride * 4.0);
    }

    #[test]
    fn scroll_offset_never_assumes_one_item_per_row() {
        let row_stride = CARD_HEIGHT + CARD_GAP;
        // With 5 columns, 5 items exactly fill row 0 - the 6th item (index 5)
        // is the first one to land on row 1, not row 5.
        assert_eq!(scroll_offset_for_item(5, 5), row_stride);
    }

    #[test]
    fn scroll_offset_treats_zero_columns_as_one() {
        // `columns` is always `.max(1)` upstream, but the function itself
        // must not divide by zero if ever called directly with 0.
        assert_eq!(scroll_offset_for_item(3, 0), 3.0 * (CARD_HEIGHT + CARD_GAP));
    }

    #[test]
    fn page_virtualization_reports_only_rows_in_the_visible_clip() {
        let stride = CARD_HEIGHT + CARD_GAP;
        assert_eq!(visible_grid_rows(500.0, 0.0, 499.0, 100, stride), 0..0);
        assert_eq!(visible_grid_rows(500.0, 500.0, 700.0, 100, stride), 0..3);
        assert_eq!(visible_grid_rows(500.0, 660.0, 900.0, 5, stride), 1..5);
    }

    #[test]
    fn page_jump_row_is_independent_of_stage_and_header_height() {
        // The click targets an absolute row rect in the outer document. The
        // stage/header prefix is supplied by live layout, never baked into
        // this grid math, and all supported column counts stay row-aligned.
        for columns in [3, 4, 5] {
            assert_eq!(
                scroll_offset_for_item(10, columns),
                scroll_offset_for_item(11, columns)
            );
        }
    }
}
