//! Region 3: the dominant selected-game stage.
//!
//! This is the visual centre of Gamer View - a full-width presentation
//! surface for the one game the user has picked, not a detail form or a
//! widened right-hand panel. It combines the game's cover (or a deliberate
//! platform-art plate when there is none), its title and real metadata, its
//! reconciled readiness, the one prominent primary action, and the secondary
//! actions beneath it.
//!
//! Every launch/readiness/cover/identity decision is still made by the exact
//! helpers `gamer_view` already owned (`gamer_readiness`,
//! `gamer_archive_readiness`, `featured_typed_launch_action`,
//! `show_featured_cover`, `GamerMetadataView`, ...); this module only lays
//! them out in the new composition.

use eframe::egui;
use std::path::Path;

use archivefs_core::{MountState, PreparedMemberCandidate};

use super::layout::GamerStageLayout;
use super::{
    GAMER_SECONDARY_ROW_MIN_WIDTH, GamerMetadataView, GamerReadiness, GamerViewAction,
    GamerViewScreen, emulator_setup_focus, featured_meta_line, featured_platform_line,
    featured_primary_button, featured_typed_launch_action, gamer_archive_readiness,
    gamer_copy_location_label, gamer_display_title, gamer_readiness, gamer_readiness_short_label,
    gamer_undo_available, show_featured_cover, show_gamer_launch_blocker,
    show_gamer_metadata_enrichment,
};
use crate::gamer_artwork;
use crate::launch_readiness_page;
use crate::ui::components::{StatusTone, archive_kind_name, format_size};
use crate::ui::platform_artwork::{
    GameRowArtworkPaint, PlatformArtworkCache, platform_asset_id, platform_fallback_asset_id,
};
use crate::ui::{components as widgets, theme};
use crate::{
    ArchiveAction, ArchiveRecord, ArchiveRow, CheatWorkflowState, OperationRequest, RowOrigin,
};

/// Everything the stage renders from. The launch state machines, cover cache
/// and metadata are all borrowed, never owned or rebuilt here.
pub(crate) struct StageContext<'a> {
    pub(crate) record: Option<&'a ArchiveRecord>,
    pub(crate) row: Option<&'a ArchiveRow>,
    pub(crate) busy: bool,
    pub(crate) block_reason: Option<&'static str>,
    pub(crate) cleanup_after_unmount: bool,
    pub(crate) cheat_workflow: Option<&'a CheatWorkflowState>,
    pub(crate) artwork_directory: Option<&'a Path>,
    pub(crate) artwork_cache: &'a mut PlatformArtworkCache,
    pub(crate) covers: &'a gamer_artwork::GamerCoverCache,
    pub(crate) game_metadata: Option<&'a crate::game_metadata::GameMetadataResult>,
    pub(crate) prepared_member: bool,
    pub(crate) member_choices: Option<&'a [PreparedMemberCandidate]>,
    pub(crate) preparation_message: Option<&'a str>,
    pub(crate) play_action: &'a launch_readiness_page::GamerPlayAction,
    pub(crate) retroarch_launch_state: &'a mut launch_readiness_page::RetroArchLaunchState,
    pub(crate) screen: &'a mut GamerViewScreen,
    pub(crate) layout: GamerStageLayout,
    /// The library holds no games at all - the stage becomes the first-run
    /// "add your games" invitation instead of a "nothing selected" prompt.
    pub(crate) first_run: bool,
}

/// Draws the selected-game stage and returns any action the user triggered.
pub(crate) fn show_selected_game_stage(
    ui: &mut egui::Ui,
    cx: StageContext<'_>,
) -> Option<GamerViewAction> {
    let StageContext {
        record,
        row,
        busy,
        block_reason,
        cleanup_after_unmount,
        cheat_workflow,
        artwork_directory,
        artwork_cache,
        covers,
        game_metadata,
        prepared_member,
        member_choices,
        preparation_message,
        play_action,
        retroarch_launch_state,
        screen,
        layout,
        first_run,
    } = cx;

    let mut action = None;
    let stage_size = egui::vec2(ui.available_width(), layout.stage_height);

    ui.allocate_ui_with_layout(stage_size, egui::Layout::top_down(egui::Align::Min), |ui| {
        stage_card(ui, |ui| match record {
            None => {
                render_empty_stage(ui, first_run, busy, &mut action);
            }
            Some(record) => {
                render_selected_stage(
                    ui,
                    SelectedArgs {
                        record,
                        row,
                        busy,
                        block_reason,
                        cleanup_after_unmount,
                        cheat_workflow,
                        artwork_directory,
                        artwork_cache,
                        covers,
                        game_metadata,
                        prepared_member,
                        member_choices,
                        preparation_message,
                        play_action,
                        retroarch_launch_state,
                        screen,
                        layout,
                    },
                    &mut action,
                );
            }
        });
    });

    action
}

/// The stage surface itself: a single raised panel filling the region, so the
/// selected game reads as one solid object rather than a stack of loose
/// controls.
fn stage_card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::RAISED_SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_FOCUS))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(theme::SPACE_XL as i8))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            add_contents(ui)
        })
        .inner
}

fn render_empty_stage(
    ui: &mut egui::Ui,
    first_run: bool,
    busy: bool,
    action: &mut Option<GamerViewAction>,
) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.28);
        if first_run {
            ui.label(
                egui::RichText::new("Add your games")
                    .size(theme::DISPLAY_SIZE)
                    .strong()
                    .color(theme::PRIMARY_TEXT),
            );
            ui.add_space(theme::SPACE_SM);
            ui.label(
                egui::RichText::new(
                    "Choose the folder where your games are kept. EmuWiz looks inside and \
                     adds what it finds - your files are never changed.",
                )
                .color(theme::SECONDARY_TEXT),
            );
            ui.add_space(theme::SPACE_LG);
            if widgets::action_button(ui, "Add games", widgets::ActionStyle::Primary, !busy)
                .clicked()
                && let Some(folder) = rfd::FileDialog::new()
                    .set_title("Choose your games folder")
                    .pick_folder()
            {
                *action = Some(GamerViewAction::AddGamesFolder(folder));
            }
        } else {
            ui.label(
                egui::RichText::new("Choose a game")
                    .size(theme::DISPLAY_SIZE)
                    .strong()
                    .color(theme::PRIMARY_TEXT),
            );
            ui.add_space(theme::SPACE_SM);
            ui.label(
                egui::RichText::new(
                    "Pick a game from the library below to see its cover, whether it is \
                     ready to play, and how to launch it.",
                )
                .color(theme::SECONDARY_TEXT),
            );
        }
    });
}

struct SelectedArgs<'a> {
    record: &'a ArchiveRecord,
    row: Option<&'a ArchiveRow>,
    busy: bool,
    block_reason: Option<&'static str>,
    cleanup_after_unmount: bool,
    cheat_workflow: Option<&'a CheatWorkflowState>,
    artwork_directory: Option<&'a Path>,
    artwork_cache: &'a mut PlatformArtworkCache,
    covers: &'a gamer_artwork::GamerCoverCache,
    game_metadata: Option<&'a crate::game_metadata::GameMetadataResult>,
    prepared_member: bool,
    member_choices: Option<&'a [PreparedMemberCandidate]>,
    preparation_message: Option<&'a str>,
    play_action: &'a launch_readiness_page::GamerPlayAction,
    retroarch_launch_state: &'a mut launch_readiness_page::RetroArchLaunchState,
    screen: &'a mut GamerViewScreen,
    layout: GamerStageLayout,
}

fn render_selected_stage(
    ui: &mut egui::Ui,
    args: SelectedArgs<'_>,
    action: &mut Option<GamerViewAction>,
) {
    let SelectedArgs {
        record,
        row,
        busy,
        block_reason,
        cleanup_after_unmount,
        cheat_workflow,
        artwork_directory,
        artwork_cache,
        covers,
        game_metadata,
        prepared_member,
        member_choices,
        preparation_message,
        play_action,
        retroarch_launch_state,
        screen,
        layout,
    } = args;

    let archive_path = record.mount_plan.archive.path.clone();
    let platform = record
        .metadata
        .platform
        .as_deref()
        .or(record.identity.platform.as_deref())
        .unwrap_or("Unknown");
    let unknown_platform = row.is_some_and(|row| row.unknown_platform);
    let title = gamer_display_title(record);
    let stage_width = ui.available_width();
    let inner_height = ui.available_height();

    // The media plate: a fixed portrait frame, so the text beside it is what
    // reflows on a short window, never the artwork.
    let media_width = layout.stage_media_width.min(stage_width - 24.0).max(120.0);
    // Side by side, the media plate gets the full inner height; stacked, it
    // must leave room for the title/status/actions that sit *below* it.
    let media_budget = if layout.stage_side_by_side {
        inner_height
    } else {
        (inner_height - gamer_artwork::FEATURED_RESERVED_BELOW)
            .max(gamer_artwork::FEATURED_COVER_MIN_HEIGHT)
    };
    // A loaded cover is sized against the taller, wider real-cover budget; a
    // still-loading, missing, or nonexistent cover gets the more restrained
    // fallback budget instead - see `featured_cover_box`. Read here, before
    // the box is sized, rather than inside `paint_media`, purely so both this
    // function and the paint closure agree on the same cover state for one
    // frame.
    let cover = covers.slot_for(archive_path.as_path(), None).cloned();
    let is_real_cover = matches!(cover, Some(gamer_artwork::CoverSlot::Ready { .. }));
    let media_box = gamer_artwork::featured_cover_box(media_width, media_budget, is_real_cover)
        .unwrap_or_else(|| egui::vec2(media_width, media_width * 1.3));

    let paint_media = |ui: &mut egui::Ui, artwork_cache: &mut PlatformArtworkCache| {
        let platform_asset = platform_asset_id(platform, unknown_platform);
        let platform_fallback = platform_fallback_asset_id(platform, unknown_platform);
        show_featured_cover(
            ui,
            media_box,
            cover.as_ref(),
            GameRowArtworkPaint {
                center: egui::Pos2::ZERO,
                size: 0.0,
                title: &title,
                platform_asset: &platform_asset,
                platform_fallback,
            },
            artwork_cache,
            artwork_directory,
        );
    };

    let mut text_column = |ui: &mut egui::Ui, action: &mut Option<GamerViewAction>| {
        render_stage_text(
            ui,
            StageTextArgs {
                record,
                row,
                platform,
                unknown_platform,
                title: &title,
                archive_path: archive_path.as_path(),
                busy,
                block_reason,
                cleanup_after_unmount,
                cheat_workflow,
                game_metadata,
                prepared_member,
                member_choices,
                preparation_message,
                play_action,
                retroarch_launch_state,
                screen,
            },
            action,
        );
    };

    // The stage's essential block (title, readiness, Play, secondary actions)
    // is always drawn in full - never inside an inner scroll area that could
    // clip Play or an action off the bottom. `stage_height` is a budget, not
    // a hard clip: the only self-bounded part is the synopsis, which already
    // has its own "show more" toggle and height cap. If a very small window
    // cannot fit everything, the content flows into the page's own scroll
    // rather than hiding a control.
    if layout.stage_side_by_side {
        ui.horizontal_top(|ui| {
            let text_width = layout.stage_text_width(stage_width);
            ui.allocate_ui_with_layout(
                egui::vec2(text_width, inner_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| text_column(ui, action),
            );
            // Push the media plate to the stage's right edge (design §4A:
            // typography and CTA on the left, box-art showcase on the
            // right), keeping at least one gutter between them.
            let gap = (ui.available_width() - media_box.x).max(theme::SPACE_XL);
            ui.add_space(gap);
            ui.allocate_ui_with_layout(
                egui::vec2(media_box.x, inner_height),
                egui::Layout::top_down(egui::Align::Center),
                |ui| paint_media(ui, artwork_cache),
            );
        });
    } else {
        paint_media(ui, artwork_cache);
        ui.add_space(theme::SPACE_LG);
        text_column(ui, action);
    }
}

struct StageTextArgs<'a> {
    record: &'a ArchiveRecord,
    row: Option<&'a ArchiveRow>,
    platform: &'a str,
    unknown_platform: bool,
    title: &'a str,
    archive_path: &'a Path,
    busy: bool,
    block_reason: Option<&'static str>,
    cleanup_after_unmount: bool,
    cheat_workflow: Option<&'a CheatWorkflowState>,
    game_metadata: Option<&'a crate::game_metadata::GameMetadataResult>,
    prepared_member: bool,
    member_choices: Option<&'a [PreparedMemberCandidate]>,
    preparation_message: Option<&'a str>,
    play_action: &'a launch_readiness_page::GamerPlayAction,
    retroarch_launch_state: &'a mut launch_readiness_page::RetroArchLaunchState,
    screen: &'a mut GamerViewScreen,
}

fn render_stage_text(
    ui: &mut egui::Ui,
    args: StageTextArgs<'_>,
    action: &mut Option<GamerViewAction>,
) {
    let StageTextArgs {
        record,
        row,
        platform,
        unknown_platform,
        title,
        archive_path,
        busy,
        block_reason,
        cleanup_after_unmount,
        cheat_workflow,
        game_metadata,
        prepared_member,
        member_choices,
        preparation_message,
        play_action,
        retroarch_launch_state,
        screen,
    } = args;

    // --- Identity block -------------------------------------------------
    ui.label(
        egui::RichText::new(format!("SYSTEM \u{b7} {}", platform.to_uppercase()))
            .size(theme::TECHNICAL_SIZE)
            .color(theme::SECONDARY_TEXT),
    );
    ui.add_space(theme::SPACE_XS);
    ui.label(
        egui::RichText::new(title)
            .size(theme::DISPLAY_SIZE)
            .strong()
            .color(theme::PRIMARY_TEXT),
    );

    let metadata_view = GamerMetadataView::merge(&record.metadata, game_metadata);
    featured_meta_line(
        ui,
        featured_platform_line(
            platform,
            archive_kind_name(record.mount_plan.archive.kind),
            metadata_view.release_year,
        ),
        false,
    );

    // --- Reconciled readiness ----------------------------------------------
    let readiness = if record.is_mount_input() {
        gamer_archive_readiness(
            record.mount_state,
            prepared_member,
            play_action,
            member_choices,
        )
    } else {
        gamer_readiness(record.mount_state, play_action)
    };
    ui.add_space(theme::SPACE_SM);
    widgets::status_badge(
        ui,
        gamer_readiness_short_label(&readiness),
        readiness_tone(&readiness),
    );

    if let Some(row) = row
        && row.origin != RowOrigin::Live
    {
        featured_meta_line(ui, row.origin.gamer_view_label().to_string(), false);
    }

    if unknown_platform {
        ui.add_space(theme::SPACE_XS);
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "We couldn't tell which game system this is for.",
        );
        if widgets::action_button(ui, "Review", widgets::ActionStyle::Secondary, true)
            .on_hover_text(
                "See what EmuWiz found for this game in Advanced View, and help identify it \
                 if you can.",
            )
            .clicked()
        {
            *action = Some(GamerViewAction::ReviewIdentity(archive_path.to_path_buf()));
        }
    }

    ui.add_space(theme::SPACE_LG);

    // --- Primary action --------------------------------------------------
    render_primary_action(
        ui,
        PrimaryArgs {
            readiness: &readiness,
            archive_path,
            busy,
            cleanup_after_unmount,
            preparation_message,
            retroarch_launch_state,
        },
        action,
    );
    if let Some(reason) = block_reason {
        widgets::technical_details(ui, "gamer-operation-block", |ui| {
            ui.label(reason);
        });
    }

    ui.add_space(theme::SPACE_LG);
    ui.separator();
    ui.add_space(theme::SPACE_MD);

    // --- Secondary actions ---------------------------------------------------
    render_secondary_actions(
        ui,
        SecondaryArgs {
            record,
            archive_path,
            cleanup_after_unmount,
            cheat_workflow,
            screen,
        },
        action,
    );

    // --- Synopsis / enrichment (subordinate) --------------------------------
    if !metadata_view.is_empty() {
        ui.add_space(theme::SPACE_MD);
        ui.separator();
        ui.add_space(theme::SPACE_SM);
        show_gamer_metadata_enrichment(ui, &metadata_view);
    }
}

struct PrimaryArgs<'a> {
    readiness: &'a GamerReadiness<'a>,
    archive_path: &'a Path,
    busy: bool,
    cleanup_after_unmount: bool,
    preparation_message: Option<&'a str>,
    retroarch_launch_state: &'a mut launch_readiness_page::RetroArchLaunchState,
}

fn render_primary_action(
    ui: &mut egui::Ui,
    args: PrimaryArgs<'_>,
    action: &mut Option<GamerViewAction>,
) {
    let PrimaryArgs {
        readiness,
        archive_path,
        busy,
        cleanup_after_unmount,
        preparation_message,
        retroarch_launch_state,
    } = args;

    match readiness {
        GamerReadiness::Mount | GamerReadiness::Prepare => {
            ui.label(
                egui::RichText::new(
                    "Temporarily makes this archived game available. The original is unchanged.",
                )
                .color(theme::muted(ui)),
            );
            if let Some(message) = preparation_message {
                ui.colored_label(ui.visuals().warn_fg_color, message);
            }
            if featured_primary_button(ui, "Prepare game", !busy).clicked() {
                *action = Some(GamerViewAction::Prepare(archive_path.to_path_buf()));
            }
        }
        GamerReadiness::ChooseMember { candidates } => {
            ui.label("Choose the game file to play:");
            for candidate in *candidates {
                let label = format!(
                    "{} ({})",
                    candidate.member_name,
                    format_size(Some(candidate.size_bytes)),
                );
                if widgets::action_button(ui, &label, widgets::ActionStyle::Secondary, !busy)
                    .on_hover_text(&candidate.reason)
                    .clicked()
                {
                    *action = Some(GamerViewAction::SelectArchiveMember(
                        archive_path.to_path_buf(),
                        candidate.member_name.clone(),
                    ));
                }
            }
        }
        GamerReadiness::Unmount => {
            if featured_primary_button(ui, "Unmount", !busy).clicked() {
                *action = Some(GamerViewAction::Operation(OperationRequest {
                    action: ArchiveAction::Unmount,
                    archive_path: archive_path.to_path_buf(),
                    cleanup_after_unmount,
                }));
            }
        }
        GamerReadiness::Ready { request } => {
            if featured_typed_launch_action(ui, request, retroarch_launch_state, !busy) {
                *action = Some(GamerViewAction::Play(Box::new((*request).clone())));
            }
            widgets::technical_details(ui, "gamer-play-adapter", |ui| {
                ui.label(format!(
                    "Uses the {} launch adapter.",
                    request.adapter_name()
                ));
            });
        }
        GamerReadiness::NeedsSetup { blocker } => {
            show_gamer_launch_blocker(ui, blocker);
            let (label, next_action) = blocker_next_action(blocker, archive_path);
            if let Some(next_action) = next_action
                && widgets::action_button(ui, label, widgets::ActionStyle::Secondary, !busy)
                    .clicked()
            {
                *action = Some(next_action);
            }
        }
        GamerReadiness::NeedsAttention { reason } => {
            ui.colored_label(ui.visuals().warn_fg_color, reason.as_str());
        }
    }
}

fn blocker_next_action(
    blocker: &launch_readiness_page::GamerBlocker,
    archive_path: &Path,
) -> (&'static str, Option<GamerViewAction>) {
    use launch_readiness_page::GamerBlockerKind as Kind;
    match &blocker.kind {
        Kind::UnknownSystem | Kind::ConflictingIdentity => (
            "Review game identity",
            Some(GamerViewAction::ReviewIdentity(archive_path.to_path_buf())),
        ),
        Kind::EmulatorNotInstalled | Kind::EmulatorSetupIncomplete => (
            "Open Emulator Setup",
            blocker.emulator.as_deref().map_or_else(
                || Some(GamerViewAction::CheckEmulators(archive_path.to_path_buf())),
                |emulator| {
                    Some(GamerViewAction::OpenEmulatorSetup(
                        archive_path.to_path_buf(),
                        emulator_setup_focus(emulator),
                    ))
                },
            ),
        ),
        Kind::EmulatorNotChecked => (
            "Check Emulators",
            Some(GamerViewAction::CheckEmulators(archive_path.to_path_buf())),
        ),
        Kind::MultipleChoices => (
            "Choose an emulator",
            Some(GamerViewAction::OpenLaunchChoices(
                archive_path.to_path_buf(),
            )),
        ),
        Kind::ContentNeedsPreparation | Kind::LaunchPlanInvalid => (
            "Open launch readiness",
            Some(GamerViewAction::OpenLaunchChoices(
                archive_path.to_path_buf(),
            )),
        ),
        Kind::NoSafeEmulator => (
            "Check Emulators",
            Some(GamerViewAction::CheckEmulators(archive_path.to_path_buf())),
        ),
        Kind::CheckingGame => ("Checking\u{2026}", None),
    }
}

struct SecondaryArgs<'a> {
    record: &'a ArchiveRecord,
    archive_path: &'a Path,
    cleanup_after_unmount: bool,
    cheat_workflow: Option<&'a CheatWorkflowState>,
    screen: &'a mut GamerViewScreen,
}

fn render_secondary_actions(
    ui: &mut egui::Ui,
    args: SecondaryArgs<'_>,
    action: &mut Option<GamerViewAction>,
) {
    let SecondaryArgs {
        record,
        archive_path,
        cleanup_after_unmount,
        cheat_workflow,
        screen,
    } = args;

    let stacked = ui.available_width() < GAMER_SECONDARY_ROW_MIN_WIDTH;
    let secondary = |ui: &mut egui::Ui, label: &str| {
        if stacked {
            let width = ui.available_width();
            ui.add(egui::Button::new(label).min_size(egui::vec2(width, 34.0)))
        } else {
            widgets::action_button(ui, label, widgets::ActionStyle::Secondary, true)
        }
    };

    let mut body = |ui: &mut egui::Ui| {
        if record.is_mount_input()
            && record.mount_state == MountState::Mounted
            && secondary(ui, "Unmount").clicked()
        {
            *action = Some(GamerViewAction::Operation(OperationRequest {
                action: ArchiveAction::Unmount,
                archive_path: archive_path.to_path_buf(),
                cleanup_after_unmount,
            }));
        }
        if secondary(ui, "Cheats & Mods").clicked() {
            *action = Some(GamerViewAction::OpenCheatsMods(archive_path.to_path_buf()));
        }
        if secondary(ui, "Details").clicked() {
            *screen = GamerViewScreen::Details;
        }
        if let Some(folder) = archive_path.parent().filter(|folder| folder.is_dir())
            && secondary(ui, gamer_copy_location_label()).clicked()
        {
            *action = Some(GamerViewAction::CopyLocation(folder.display().to_string()));
        }
    };
    if stacked {
        body(ui);
    } else {
        ui.horizontal_wrapped(body);
    }

    if gamer_undo_available(cheat_workflow, Some(archive_path)) {
        ui.add_space(theme::SPACE_SM);
        if widgets::action_button(ui, "Undo last change", widgets::ActionStyle::Quiet, true)
            .clicked()
        {
            *action = Some(GamerViewAction::Undo);
        }
    }
}

/// Maps the reconciled readiness onto a status-badge tone, so the stage's
/// readiness pill carries colour *and* the word (never colour alone).
fn readiness_tone(readiness: &GamerReadiness<'_>) -> StatusTone {
    match readiness {
        GamerReadiness::Ready { .. } => StatusTone::Success,
        GamerReadiness::Unmount => StatusTone::Active,
        GamerReadiness::Mount | GamerReadiness::Prepare | GamerReadiness::ChooseMember { .. } => {
            StatusTone::Pending
        }
        GamerReadiness::NeedsSetup { .. } | GamerReadiness::NeedsAttention { .. } => {
            StatusTone::Warning
        }
    }
}
