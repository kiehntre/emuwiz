//! EmuWiz Blue Home (design boards `C-Home-*`, `IH5-HomeWelcome-*`).
//!
//! Presentation over state the app already holds: the library totals, the
//! read-only environment snapshot, the shared problem summary, the
//! deterministic guidance engine, this session's Activity jobs and the
//! existing cover and platform pictures. Every button routes to a page that
//! already exists; nothing here starts work the Classic Home would not.
//!
//! Honest gaps against the boards: EmuWiz records no play history, so there
//! is no "Continue playing" hero or "Recently played" row. The hero shows the
//! game opened most recently in this session (or the welcome panel), and the
//! cover row is the existing "Recently opened" / "Newest in your library".

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, RichText};

use super::{
    App,
    activity::Phase,
    media_sources::Kind,
    routes::{Route, Section},
    simple_shell::palette,
};
use crate::ui::{
    components::{ActionStyle, Cue, action_button, paint_cue},
    theme::{self, blue},
};

/// One "What would you like to do?" tile: the legacy Home card's own title
/// and explanation, onto an existing route.
pub(super) struct HomeTile {
    pub title: &'static str,
    pub explanation: &'static str,
    pub section: Section,
    tint: egui::Color32,
}

/// The five approved tiles. Titles and explanations are the app's own
/// (`home_page.rs` HomeCard); tints are the Manage hub's tile colours.
pub(super) const HOME_TILES: [HomeTile; 5] = [
    HomeTile {
        title: "Verify your games",
        explanation: "Check game names, versions, and known-good file matches using trusted game catalogues.",
        section: Section::Check,
        tint: egui::Color32::from_rgb(0x1f, 0x6f, 0xd6),
    },
    HomeTile {
        title: "Set up emulators",
        explanation: "Check which emulators EmuWiz can find and whether each is ready to launch games.",
        section: Section::Emulators,
        tint: egui::Color32::from_rgb(0x5b, 0x4b, 0xc4),
    },
    HomeTile {
        title: "Cheats & Mods",
        explanation: "Find cheats and game enhancements for a selected game.",
        section: Section::Mods,
        tint: egui::Color32::from_rgb(0xa8, 0x3a, 0x6a),
    },
    HomeTile {
        title: "Find duplicate games",
        explanation: "Find identical or equivalent copies in your library, keep one, and move the rest into a recoverable quarantine. Nothing is permanently deleted.",
        section: Section::Duplicates,
        tint: egui::Color32::from_rgb(0x1e, 0x8a, 0x5a),
    },
    HomeTile {
        title: "Convert discs",
        explanation: "Convert supported disc images into verified CHD files.",
        section: Section::Converter,
        tint: egui::Color32::from_rgb(0xb0, 0x62, 0x2a),
    },
];

pub(super) const WELCOME_TITLE: &str = "Welcome back!";
pub(super) const FIRST_RUN_TITLE: &str = "Welcome to EmuWiz";
pub(super) const WELCOME_PURPOSE: &str = "Your games, and the things you can do with them.";
const RIGHT_COLUMN_WIDE: f32 = 440.0;
const RIGHT_COLUMN: f32 = 360.0;
const COLUMN_GAP: f32 = 24.0;
const TILE_MIN_WIDTH: f32 = 148.0;
const TILE_GAP: f32 = 14.0;

/// The tone of a Setup status row: colour plus a painted mark and words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SetupTone {
    Ready,
    NeedsSetup,
    NotChecked,
}

impl SetupTone {
    fn color(self) -> egui::Color32 {
        match self {
            Self::Ready => blue::OK,
            Self::NeedsSetup => blue::WARN,
            Self::NotChecked => blue::DIM_TEXT,
        }
    }
}

/// One Setup status row, in the app's own words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SetupRow {
    pub name: &'static str,
    pub detail: String,
    pub state: String,
    pub tone: SetupTone,
}

/// The welcome title: 58 / 42 / 34 px at the large, medium and compact
/// windows (design handoff, page title scale).
pub(super) fn welcome_title_size(window_width: f32, compact: bool) -> f32 {
    if compact {
        34.0
    } else if window_width >= 1680.0 {
        58.0
    } else {
        42.0
    }
}

/// Columns for the tiles that fit `available` points (5 at most, 2 at least).
pub(super) fn tile_columns(available: f32) -> usize {
    (((available + TILE_GAP) / (TILE_MIN_WIDTH + TILE_GAP)).floor() as usize).clamp(2, 5)
}

/// "just now", "4 min ago", "2 h ago": no invented precision.
pub(super) fn relative(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes} min ago"),
        _ => format!("{} h ago", minutes / 60),
    }
}

impl App {
    /// The Setup status rows, from the read-only environment snapshot.
    pub(super) fn home_setup_rows(&self) -> Vec<SetupRow> {
        let Some(environment) = self.environment.as_ref() else {
            return ["Emulators", "Game folders", "Game catalogues"]
                .into_iter()
                .map(|name| SetupRow {
                    name,
                    detail: "Checking…".into(),
                    state: "Not checked yet".into(),
                    tone: SetupTone::NotChecked,
                })
                .collect();
        };
        let emulators = environment.installed_emulator_count();
        let folders = environment.source_count;
        let catalogues = environment.identification_data_count;
        vec![
            SetupRow {
                name: "Emulators",
                detail: format!("{emulators} found"),
                state: if emulators > 0 {
                    "Ready".into()
                } else {
                    "None found yet".into()
                },
                tone: if emulators > 0 {
                    SetupTone::Ready
                } else {
                    SetupTone::NeedsSetup
                },
            },
            SetupRow {
                name: "Game folders",
                detail: if environment.unavailable_sources.is_empty() {
                    format!("{folders} folders")
                } else {
                    format!(
                        "{folders} folders · {} unavailable",
                        environment.unavailable_sources.len()
                    )
                },
                state: if folders == 0 {
                    "No source folders yet".into()
                } else if environment.source_needs_attention() {
                    "Needs attention".into()
                } else {
                    "Ready to browse".into()
                },
                tone: if folders > 0 && !environment.source_needs_attention() {
                    SetupTone::Ready
                } else {
                    SetupTone::NeedsSetup
                },
            },
            SetupRow {
                name: "Game catalogues",
                detail: format!("{catalogues} catalogues"),
                state: if environment.identification_data_ready {
                    "Ready".into()
                } else {
                    "Not set up".into()
                },
                tone: if environment.identification_data_ready {
                    SetupTone::Ready
                } else {
                    SetupTone::NotChecked
                },
            },
        ]
    }

    /// Blue Home: welcome, tasks and collection on the left; Mr Wiz, Setup
    /// status and Recent activity on the right (one column when narrow).
    pub(super) fn home_blue(&mut self, ui: &mut egui::Ui) {
        // Home and Problems & Repair count the same findings: one summary.
        if self.loaded && self.problem_summary.is_none() {
            self.start_problem_summary();
        }
        let first_run = self.loaded && self.library.games.is_empty();
        let compact = crate::ui::components::blue_compact(ui);
        let window = ui.ctx().content_rect().width();
        ui.label(
            RichText::new(if first_run {
                FIRST_RUN_TITLE
            } else {
                WELCOME_TITLE
            })
            .size(welcome_title_size(window, compact))
            .strong(),
        );
        ui.label(
            RichText::new(WELCOME_PURPOSE)
                .size(if compact { 15.0 } else { 18.0 })
                .color(palette::SECONDARY_TEXT),
        );
        ui.add_space(if compact { 8.0 } else { 14.0 });
        let width = ui.available_width();
        let right = if width >= 1500.0 {
            RIGHT_COLUMN_WIDE
        } else {
            RIGHT_COLUMN
        };
        if !compact && width >= 1080.0 {
            let left = width - right - COLUMN_GAP;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = COLUMN_GAP;
                ui.allocate_ui_with_layout(
                    egui::vec2(left, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(left);
                        self.home_welcome_panel(ui, false);
                        self.home_tiles(ui);
                        self.home_collection(ui);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(right, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(right);
                        self.home_mr_wiz_card(ui);
                        ui.add_space(16.0);
                        self.home_setup_card(ui);
                        ui.add_space(16.0);
                        self.home_activity_card(ui);
                    },
                );
            });
        } else {
            self.home_welcome_panel(ui, compact);
            self.home_tiles(ui);
            if compact {
                self.home_setup_strip(ui);
            } else {
                self.home_mr_wiz_card(ui);
                ui.add_space(16.0);
                self.home_setup_card(ui);
                ui.add_space(16.0);
                self.home_activity_card(ui);
            }
            self.home_collection(ui);
        }
    }

    fn home_welcome_panel(&mut self, ui: &mut egui::Ui, compact: bool) {
        let recent = self
            .imagery
            .recently_opened()
            .first()
            .and_then(|id| self.library.game(*id).cloned());
        let mut go = None;
        blue_card(ui, if compact { 14 } else { 28 }, |ui| {
            if !self.loaded {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        "Loading your existing game list. You can already explore the tasks below.",
                    );
                });
                return;
            }
            if self.library.games.is_empty() {
                eyebrow(ui, "Let's get started");
                ui.label(RichText::new("Let's find your games").size(28.0).strong());
                ui.label("No games are listed yet. Start by adding a folder you already keep games in. EmuWiz lists the files; it does not move them.");
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    if action_button(ui, "Add game folder", ActionStyle::Primary, true).clicked() {
                        go = Some(Route::Section(Section::Sources));
                    }
                    if action_button(ui, "Check my setup", ActionStyle::Secondary, true).clicked() {
                        go = Some(Route::Section(Section::Setup));
                    }
                });
                return;
            }
            if let Some(game) = recent {
                ui.horizontal_top(|ui| {
                    let cover = if compact {
                        egui::vec2(96.0, 128.0)
                    } else {
                        egui::vec2(150.0, 200.0)
                    };
                    self.picture(ui, &game, Kind::Cover, cover);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        eyebrow(ui, "Recently opened");
                        ui.label(RichText::new(&game.platform).color(palette::SECONDARY_TEXT));
                        super::title_fit::title_label(
                            ui,
                            &game.title,
                            if compact {
                                theme::TextRole::CardTitle
                            } else {
                                theme::TextRole::SectionTitle
                            },
                            2,
                        );
                        ui.add_space(10.0);
                        ui.horizontal_wrapped(|ui| {
                            if action_button(ui, "Open game", ActionStyle::Primary, true).clicked()
                            {
                                go = Some(Route::Game(game.archive.id));
                            }
                            if action_button(ui, "Open Library", ActionStyle::Secondary, true)
                                .clicked()
                            {
                                go = Some(Route::BrowsePlay);
                            }
                        });
                    });
                });
                return;
            }
            eyebrow(ui, "No game selected");
            ui.label(
                RichText::new("Pick a game and it will appear here")
                    .size(if compact { 22.0 } else { 30.0 })
                    .strong(),
            );
            ui.label("Open your Library to choose a game, or add a game folder.");
            ui.add_space(if compact { 6.0 } else { 12.0 });
            ui.horizontal(|ui| {
                if action_button(ui, "Open Library", ActionStyle::Primary, true).clicked() {
                    go = Some(Route::BrowsePlay);
                }
                if action_button(ui, "Add game folder", ActionStyle::Secondary, true).clicked() {
                    go = Some(Route::Section(Section::Sources));
                }
            });
        });
        if let Some(route) = go {
            self.go(route);
        }
    }

    fn home_tiles(&mut self, ui: &mut egui::Ui) {
        let compact = crate::ui::components::blue_compact(ui);
        if compact {
            // The compact board has no heading here: the tiles follow directly.
            ui.add_space(10.0);
        } else {
            ui.add_space(18.0);
            ui.label(theme::section_title("What would you like to do?"));
            ui.add_space(6.0);
        }
        // Compact shows four, as on the compact board; every task is also on
        // the sidebar and in All Tools.
        let shown = if compact { 4 } else { HOME_TILES.len() };
        let columns = tile_columns(ui.available_width()).min(shown);
        let width = (ui.available_width() - TILE_GAP * (columns as f32 - 1.0)) / columns as f32;
        let base = if compact { 88.0 } else { 172.0 };
        let mut chosen = None;
        for row in HOME_TILES[..shown].chunks(columns) {
            // Every tile in a row grows to the tallest one's whole text, so a
            // narrow window wraps the explanation instead of cutting it off.
            let height = row
                .iter()
                .map(|tile| tile_height(ui, tile, width, compact))
                .fold(base, f32::max);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = TILE_GAP;
                for tile in row {
                    if home_tile(ui, tile, egui::vec2(width, height), compact).clicked() {
                        chosen = Some(tile.section);
                    }
                }
            });
            ui.add_space(TILE_GAP);
        }
        if let Some(section) = chosen {
            self.go(Route::Section(section));
        }
    }

    fn home_collection(&mut self, ui: &mut egui::Ui) {
        if self.loaded && !self.library.games.is_empty() {
            self.home_showcase(ui);
            self.home_systems(ui);
        }
    }

    /// Mr Wiz with the page's one deterministic tip. The badge is the only
    /// approved asset (decision D2); it is left out on warnings.
    fn home_mr_wiz_card(&mut self, ui: &mut egui::Ui) {
        let tip = self
            .guidance_context(&Route::Home)
            .and_then(|context| self.guidance.select(&context));
        let Some(tip) = tip else {
            return;
        };
        let friendly = matches!(
            tip.category,
            super::guidance::GuidanceCategory::Tip
                | super::guidance::GuidanceCategory::Explain
                | super::guidance::GuidanceCategory::Success
                | super::guidance::GuidanceCategory::EmptyState
        );
        let badge = friendly
            .then(|| self.imagery.mascot(ui.ctx()).cloned())
            .flatten();
        blue_card(ui, 20, |ui| {
            ui.horizontal_top(|ui| {
                if let Some(texture) = &badge {
                    ui.add(
                        egui::Image::new((texture.id(), egui::vec2(96.0, 96.0)))
                            .sense(egui::Sense::hover()),
                    );
                    ui.add_space(8.0);
                }
                ui.vertical(|ui| {
                    ui.label(theme::card_title("Mr Wiz says…"));
                    ui.add_space(4.0);
                    egui::Frame::new()
                        .fill(blue::INSET)
                        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
                        .corner_radius(10)
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&tip.message).color(blue::BODY_TEXT),
                                )
                                .wrap(),
                            );
                        });
                });
            });
        });
    }

    fn home_setup_card(&mut self, ui: &mut egui::Ui) {
        let rows = self.home_setup_rows();
        let attention = self
            .problem_summary
            .as_ref()
            .map(|summary| summary.attention_count());
        self.tools.needs_attention.poll(ui.ctx());
        let saved_attention = self.tools.needs_attention.unresolved();
        let mut go = None;
        blue_card(ui, 20, |ui| {
            if card_header(ui, "Setup status", "Open Setup ›") {
                go = Some(Route::Section(Section::Setup));
            }
            for row in &rows {
                ui.separator();
                setup_row(ui, row);
            }
            if let Some(count) = attention
                && count > 0
            {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{count} need attention in your library"))
                            .color(blue::WARN),
                    );
                    if ui.link("Review ›").clicked() {
                        go = Some(Route::Section(Section::Problems));
                    }
                });
            }
            if let Some(count) = saved_attention
                && count > 0
            {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{count} saved {} need attention",
                            if count == 1 { "item" } else { "items" }
                        ))
                        .color(blue::WARN),
                    );
                    if ui.link("Needs Attention ›").clicked() {
                        go = Some(Route::Tool(super::routes::ToolPage::NeedsAttention));
                    }
                });
            }
        });
        if let Some(route) = go {
            if route == Route::Section(Section::Problems) {
                self.open_problems_from_home();
            } else {
                self.go(route);
            }
        }
    }

    fn home_activity_card(&mut self, ui: &mut egui::Ui) {
        let mut jobs: Vec<_> = self.activity.jobs.values().collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.started));
        let mut open = false;
        blue_card(ui, 20, |ui| {
            open = card_header(ui, "Recent activity", "View all ›");
            if jobs.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new("Nothing has run yet in this session.")
                        .color(palette::SECONDARY_TEXT),
                );
            }
            for job in jobs.into_iter().take(5) {
                ui.separator();
                ui.horizontal(|ui| {
                    let (cue, color) = match job.phase {
                        Phase::Complete => (Cue::Check, blue::OK),
                        Phase::Failed => (Cue::Cross, blue::BAD),
                        Phase::Queued | Phase::Running => (Cue::Play, blue::ACCENT),
                        Phase::Cancelled | Phase::Superseded => (Cue::Dot, blue::DIM_TEXT),
                    };
                    let (mark, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    paint_cue(ui.painter(), mark.center(), cue, color);
                    let when = job
                        .started
                        .map(|started| relative(started.elapsed()))
                        .unwrap_or_default();
                    let when_width = text_width(ui, &when, 13.0) + 4.0;
                    let title_width =
                        (ui.available_width() - when_width - ui.spacing().item_spacing.x).max(40.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(title_width, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_width(title_width);
                            ui.add(egui::Label::new(&job.title).truncate())
                                .on_hover_text(job.phase.label());
                        },
                    );
                    ui.label(RichText::new(when).size(13.0).color(palette::DIM_TEXT));
                });
            }
        });
        if open {
            self.activity_attention_only = false;
            self.go(Route::Section(Section::Activity));
        }
    }

    /// Compact: Setup status as one line of marks, with a small Mr Wiz badge
    /// and the short "Looking is always safe." note (board `C-Home-Compact`).
    fn home_setup_strip(&mut self, ui: &mut egui::Ui) {
        let rows = self.home_setup_rows();
        let badge = self.imagery.mascot(ui.ctx()).cloned();
        let mut open = false;
        blue_card(ui, 12, |ui| {
            ui.horizontal_wrapped(|ui| {
                open = ui
                    .link(RichText::new("Setup status").strong())
                    .on_hover_text("Open Setup")
                    .clicked();
                // Marks and words placed directly in the wrapping row, so a
                // narrow window wraps them instead of painting past the edge.
                for row in &rows {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    paint_cue(
                        ui.painter(),
                        rect.center(),
                        tone_cue(row.tone),
                        row.tone.color(),
                    );
                    ui.label(
                        RichText::new(format!("{}: {}", row.name, row.state))
                            .color(row.tone.color()),
                    );
                }
                if let Some(texture) = &badge {
                    ui.add(egui::Image::new((texture.id(), egui::vec2(28.0, 28.0))));
                }
                ui.label(RichText::new("Looking is always safe.").color(palette::SECONDARY_TEXT));
            });
        });
        if open {
            self.go(Route::Section(Section::Setup));
        }
    }
}

fn tone_cue(tone: SetupTone) -> Cue {
    match tone {
        SetupTone::Ready => Cue::Check,
        SetupTone::NeedsSetup => Cue::Attention,
        SetupTone::NotChecked => Cue::Dot,
    }
}

/// A painted mark followed by coloured words: status is never colour alone.
fn marked_label(ui: &mut egui::Ui, cue: Cue, text: &str, color: egui::Color32, strong: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.spacing_mut().interact_size.y = 0.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        paint_cue(ui.painter(), rect.center(), cue, color);
        let text = RichText::new(text).color(color);
        ui.label(if strong { text.strong() } else { text });
    });
}

/// One Setup status row: name and detail on the left, the state on the
/// right. Widths are measured, so nothing is painted past the card.
fn setup_row(ui: &mut egui::Ui, row: &SetupRow) {
    let body = egui::TextStyle::Body.resolve(ui.style()).size;
    let state_width = text_width(ui, &row.state, body) + 12.0 + 5.0 + 8.0;
    ui.horizontal(|ui| {
        let left = (ui.available_width() - state_width - ui.spacing().item_spacing.x).max(60.0);
        ui.allocate_ui_with_layout(
            egui::vec2(left, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(left);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add(egui::Label::new(RichText::new(row.name).strong()).truncate());
                ui.add(
                    egui::Label::new(
                        RichText::new(&row.detail)
                            .size(13.0)
                            .color(palette::SECONDARY_TEXT),
                    )
                    .truncate(),
                );
            },
        );
        marked_label(ui, tone_cue(row.tone), &row.state, row.tone.color(), true);
    });
}

/// A card title with one link at the right edge. Returns whether the link
/// was clicked.
fn card_header(ui: &mut egui::Ui, title: &str, link: &str) -> bool {
    let body = egui::TextStyle::Body.resolve(ui.style()).size;
    let link_width = text_width(ui, link, body) + 6.0;
    ui.horizontal(|ui| {
        let left = (ui.available_width() - link_width - ui.spacing().item_spacing.x).max(60.0);
        ui.allocate_ui_with_layout(
            egui::vec2(left, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_width(left);
                ui.add(egui::Label::new(theme::card_title(title)).truncate());
            },
        );
        ui.link(link).clicked()
    })
    .inner
}

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(size),
            palette::PRIMARY_TEXT,
        )
        .size()
        .x
}

fn eyebrow(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(13.0)
            .strong()
            .color(palette::ACCENT),
    );
}

/// The Blue panel surface: card fill, subtle border, radius 14.
fn blue_card<R>(ui: &mut egui::Ui, padding: i8, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(blue::CARD_SURFACE_TOP)
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(padding))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

const TILE_PADDING: f32 = 16.0;
/// Room kept right of the explanation for the arrow circle.
const TILE_ARROW_RESERVE: f32 = 30.0;

fn tile_icon_side(compact: bool) -> f32 {
    if compact { 34.0 } else { 46.0 }
}

fn tile_title(
    painter: &egui::Painter,
    tile: &HomeTile,
    width: f32,
    compact: bool,
) -> Arc<egui::Galley> {
    painter.layout(
        tile.title.to_owned(),
        egui::FontId::proportional(if compact { 16.0 } else { 18.0 }),
        palette::PRIMARY_TEXT,
        width - 2.0 * TILE_PADDING,
    )
}

/// The whole explanation, wrapped to the tile; never elided.
fn tile_body(painter: &egui::Painter, tile: &HomeTile, width: f32) -> Arc<egui::Galley> {
    painter.layout(
        tile.explanation.to_owned(),
        egui::FontId::proportional(13.5),
        palette::SECONDARY_TEXT,
        width - 2.0 * TILE_PADDING - TILE_ARROW_RESERVE,
    )
}

/// The height a tile needs to show its title and its whole explanation.
fn tile_height(ui: &egui::Ui, tile: &HomeTile, width: f32, compact: bool) -> f32 {
    let painter = ui.painter();
    let title = tile_title(painter, tile, width, compact).size().y;
    let body = if compact {
        0.0
    } else {
        6.0 + tile_body(painter, tile, width).size().y
    };
    TILE_PADDING + tile_icon_side(compact) + 12.0 + title + body + TILE_PADDING
}

/// One task tile: coloured icon square, title, the app's explanation and an
/// arrow. A real focusable button; it only navigates.
fn home_tile(
    ui: &mut egui::Ui,
    tile: &HomeTile,
    size: egui::Vec2,
    compact: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tile.title)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let active = response.hovered() || response.has_focus();
        painter.rect(
            rect,
            14.0,
            if active {
                palette::HOVER
            } else {
                palette::CARD_SURFACE
            },
            egui::Stroke::new(
                1.0_f32,
                if active {
                    palette::SELECTED_OUTLINE
                } else {
                    palette::BORDER_SUBTLE
                },
            ),
            egui::StrokeKind::Inside,
        );
        let tile_side = tile_icon_side(compact);
        let icon = egui::Rect::from_min_size(
            rect.min + egui::vec2(TILE_PADDING, TILE_PADDING),
            egui::vec2(tile_side, tile_side),
        );
        painter.rect_filled(icon, 10.0, tile.tint);
        paint_tile_icon(painter, tile.section, icon.center(), tile_side / 46.0);
        let title = tile_title(painter, tile, rect.width(), compact);
        let title_pos = icon.left_bottom() + egui::vec2(0.0, 12.0);
        let title_height = title.size().y;
        painter.galley(title_pos, title, palette::PRIMARY_TEXT);
        if !compact {
            // The row is sized for the whole explanation (`tile_height`).
            let body = tile_body(painter, tile, rect.width());
            painter.with_clip_rect(rect.shrink(6.0)).galley(
                title_pos + egui::vec2(0.0, title_height + 6.0),
                body,
                palette::SECONDARY_TEXT,
            );
            let arrow = rect.right_bottom() - egui::vec2(28.0, 28.0);
            painter.circle_stroke(arrow, 13.0, egui::Stroke::new(1.2_f32, palette::ACCENT));
            painter.arrow(
                arrow - egui::vec2(6.0, 0.0),
                egui::vec2(12.0, 0.0),
                egui::Stroke::new(1.6_f32, palette::ACCENT),
            );
        }
    }
    theme::paint_focus_ring(ui, &response, rect, 14.0);
    response.on_hover_text(tile.explanation)
}

/// White line icons on the tiles: a magnifier, a controller, a star, two
/// copies and a disc. `scale` is 1 at the full 46 px tile.
fn paint_tile_icon(painter: &egui::Painter, section: Section, c: egui::Pos2, scale: f32) {
    let stroke = egui::Stroke::new(2.0_f32, egui::Color32::WHITE);
    let at = |x: f32, y: f32| c + egui::vec2(x, y) * scale;
    match section {
        Section::Check => {
            painter.circle_stroke(at(-2.0, -2.0), 7.0 * scale, stroke);
            painter.line_segment([at(3.2, 3.2), at(9.0, 9.0)], stroke);
        }
        Section::Emulators => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(22.0, 13.0) * scale),
                5.0 * scale,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([at(-7.5, 0.0), at(-3.0, 0.0)], stroke);
            painter.line_segment([at(-5.25, -2.25), at(-5.25, 2.25)], stroke);
            painter.circle_filled(at(5.0, 0.0), 1.6 * scale, egui::Color32::WHITE);
        }
        Section::Mods => {
            let points: Vec<_> = (0..10)
                .map(|index| {
                    let radius = if index % 2 == 0 { 10.0 } else { 4.4 };
                    let angle =
                        -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.0;
                    at(radius * angle.cos(), radius * angle.sin())
                })
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
        }
        Section::Duplicates => {
            for offset in [-3.0, 3.0] {
                painter.rect_stroke(
                    egui::Rect::from_center_size(
                        at(offset, offset),
                        egui::vec2(12.0, 12.0) * scale,
                    ),
                    2.0 * scale,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
        _ => {
            painter.circle_stroke(c, 9.0 * scale, stroke);
            painter.circle_stroke(c, 2.5 * scale, stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_use_the_apps_own_titles_on_existing_routes() {
        let titles: Vec<_> = HOME_TILES.iter().map(|tile| tile.title).collect();
        assert_eq!(
            titles,
            [
                "Verify your games",
                "Set up emulators",
                "Cheats & Mods",
                "Find duplicate games",
                "Convert discs"
            ]
        );
        for tile in &HOME_TILES {
            assert!(super::super::routes::SECTIONS.contains(&tile.section));
        }
    }

    #[test]
    fn tiles_reflow_between_five_and_two_across() {
        assert_eq!(tile_columns(1200.0), 5);
        // The 1440 left column fits all five.
        assert_eq!(tile_columns(816.0), 5);
        assert_eq!(tile_columns(600.0), 3);
        assert_eq!(tile_columns(300.0), 2);
    }

    #[test]
    fn activity_times_are_coarse_and_never_invented() {
        assert_eq!(relative(Duration::from_secs(20)), "just now");
        assert_eq!(relative(Duration::from_secs(4 * 60 + 10)), "4 min ago");
        assert_eq!(relative(Duration::from_secs(2 * 3600 + 5)), "2 h ago");
    }
}

#[cfg(test)]
#[path = "home_blue/polish_tests.rs"]
mod polish_tests;
