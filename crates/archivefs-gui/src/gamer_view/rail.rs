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

use super::{
    GamerLibrarySnapshot, GamerViewAction, GamerViewScreen, gamer_display_title,
    gamer_empty_list_guidance, gamer_view_row_state_label,
};
use crate::gamer_artwork::{self, COVER_BOX, CoverSlot};
use crate::ui::platform_artwork::{
    GameRowArtworkPaint, PlatformArtworkCache, paint_cover_fitted, paint_game_row_artwork,
    platform_asset_id, platform_fallback_asset_id,
};
use crate::ui::theme;
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
    } = cx;

    let visible = &snapshot.visible;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("YOUR LIBRARY")
                .size(theme::SECTION_TITLE_SIZE)
                .strong()
                .color(theme::PRIMARY_TEXT),
        );
        ui.label(
            egui::RichText::new(format!("{}", visible.len()))
                .size(theme::SECTION_TITLE_SIZE)
                .color(theme::SECONDARY_TEXT),
        );
    });
    ui.add_space(theme::SPACE_SM);

    if visible.is_empty() {
        // The empty-library first-run invitation lives in the stage; here we
        // only explain a search/platform filter that currently matches
        // nothing.
        if !data.rows.is_empty() {
            ui.weak(gamer_empty_list_guidance(
                false,
                searching,
                platform_selected,
            ));
        }
        return None;
    }

    let columns = columns.max(1);
    let grid_rows = visible.len().div_ceil(columns);
    let row_stride = CARD_HEIGHT + CARD_GAP;
    let selected_path = archive_context.focused.clone();

    egui::ScrollArea::vertical()
        .id_salt("gamer_browsing_rail")
        .auto_shrink([false, false])
        .min_scrolled_height(min_height.max(120.0))
        .show_rows(ui, row_stride, grid_rows, |ui, row_range| {
            schedule_covers(
                covers,
                cover_requests,
                data,
                visible,
                &row_range,
                columns,
                selected_path.as_deref(),
            );

            for grid_row in row_range {
                let first = grid_row * columns;
                let last = ((grid_row + 1) * columns).min(visible.len());
                let available = ui.available_width();
                let card_width = ((available - CARD_GAP * (columns.saturating_sub(1)) as f32)
                    / columns as f32)
                    .max(120.0);
                ui.horizontal(|ui| {
                    for &index in &visible[first..last] {
                        let record = &data.records[index];
                        let row = &data.rows[index];
                        let selected =
                            archive_context.focused.as_deref() == Some(row.path.as_path());
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
                        }
                        ui.add_space(CARD_GAP);
                    }
                });
                ui.add_space(CARD_GAP);
            }
        });

    None
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
