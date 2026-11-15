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
    library::{CheckedReadiness, Filter, Game, ReadinessCache, ReadinessFilter, SortOrder},
    media_sources::Kind,
    routes::{Route, Section},
};
use crate::gamer_view::alpha_jump::{ALPHA_BUCKETS, bucket_for_title, bucket_label};
use crate::ui::theme;
use eframe::egui::{self, Color32, RichText};

// Keep small-catalogue interactions immediate; larger scans use the existing
// generation-checked, debounced Filter worker rather than blocking paint.
pub(super) const IMMEDIATE_FILTER_LIMIT: usize = 10_000;
/// The ScreenScraper artwork batch picker (at most 25 games). It is separate
/// from the Library's Ctrl/Shift-click selection ("Selected games").
pub(super) const SELECT_FOR_ARTWORK: &str = "Select for artwork";
pub(super) const SELECT_FOR_ARTWORK_HINT: &str = "Select for artwork: pick up to 25 games to look \
     up artwork and details on ScreenScraper. This is separate from Selected games \
     (Ctrl/Shift-click or Ctrl+A).";
/// The toggle where "Select for artwork" does not fit but this does.
pub(super) const SELECT_FOR_ARTWORK_MEDIUM: &str = "Artwork (25 max)";
/// The toggle on a very narrow toolbar: no wider than "Done selecting", so the
/// row keeps its layout. Its accessible name is still the full one, and the
/// picker states its purpose on screen once opened.
pub(super) const SELECT_FOR_ARTWORK_SHORT: &str = "Artwork";
/// What keyboard focus and assistive tech announce for the toggle, whatever
/// label fits on screen.
pub(super) const SELECT_FOR_ARTWORK_NAME: &str = "Select for artwork (up to 25 games)";
/// Shown while the artwork picker is open, so its purpose never depends on hover.
pub(super) const ARTWORK_PICKER_PURPOSE: &str = "Artwork selection: pick up to 25 games, then \
     Enrich metadata looks up their artwork and details on ScreenScraper. Separate from \
     Selected games.";

/// Which label the artwork toggle shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArtworkLabel {
    Full,
    Medium,
    Short,
}

impl ArtworkLabel {
    pub(super) fn text(self) -> &'static str {
        match self {
            Self::Full => SELECT_FOR_ARTWORK,
            Self::Medium => SELECT_FOR_ARTWORK_MEDIUM,
            Self::Short => SELECT_FOR_ARTWORK_SHORT,
        }
    }
}

/// The widest label whose extra width (over the short one) still leaves
/// `spare` at least `min`. Pure, so the choice is unit tested.
pub(super) fn artwork_label_for(
    spare: f32,
    full_extra: f32,
    medium_extra: f32,
    min: f32,
) -> ArtworkLabel {
    if spare - full_extra >= min {
        ArtworkLabel::Full
    } else if spare - medium_extra >= min {
        ArtworkLabel::Medium
    } else {
        ArtworkLabel::Short
    }
}

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
const SIDE_BY_SIDE_MIN_WIDTH: f32 = 1100.0;
const BIG_PICTURE: &str = "Big Picture";
/// Narrowest the Library search field is allowed to become before the
/// List/Grid switch moves to its own row.
const SEARCH_MIN_WIDTH: f32 = 140.0;
/// The button at the end of the systems row that opens the Surprise Me picker.
pub(super) const SURPRISE_ME: &str = "Surprise me";
/// The Library's way into the existing read-only Check Games workflow.
pub(super) const VERIFY_GAMES: &str = "Verify Games";
/// Below this height the selected-game panel uses the compact cover so Play
/// stays beside it instead of below a tall picture.
const TIGHT_PANEL_HEIGHT: f32 = 440.0;
/// The shortest body the Library lays itself out in; a shorter window scrolls
/// the page to it rather than squeezing the shelf and the selected game.
pub(super) const MIN_BODY_HEIGHT: f32 = 300.0;
/// A compact selection needs space for its strip and a reachable shelf.
pub(super) fn minimum_body_height(width: f32, selected: bool, blue: bool) -> f32 {
    if blue && selected && width < SIDE_BY_SIDE_MIN_WIDTH {
        520.0
    } else {
        MIN_BODY_HEIGHT
    }
}

/// Horizontal padding inside each readiness button.
const READINESS_BUTTON_PAD: f32 = 5.0;
const ATTENTION_TEXT: Color32 = Color32::from_rgb(232, 170, 62);
/// How long a selection must stay put before its readiness check starts, so
/// moving quickly through the shelf reads no game files.
pub(super) const SELECTION_SETTLE_SECS: f64 = 0.35;

/// Platform, search, (readiness view, cache-aware), unverified-only, library.
type FilterKey = (
    String,
    String,
    (ReadinessFilter, bool),
    (bool, SortOrder),
    usize,
);
/// Base list address, cache version, view, and the narrowed list.
type ReadinessView = (usize, u64, ReadinessFilter, Arc<Vec<usize>>);

/// Fewest games for which the A-Z strip is worth its row.
const ALPHA_STRIP_MIN: usize = 30;

/// Set (by the controller layer) to ask the shelf to move focus one grid row:
/// -1 up, +1 down. The shelf knows its own columns and which rows are drawn;
/// egui's spatial search cannot see a card that is fully scrolled out of view.
pub(super) fn row_step_id() -> egui::Id {
    egui::Id::new("browse_play_row_step")
}

/// The game id of the focused Library card, kept for the controller layer.
pub(super) fn focused_card_id() -> egui::Id {
    egui::Id::new("browse_play_focused_card")
}

/// The egui control id of that card.
pub(super) fn focused_card_control_id() -> egui::Id {
    egui::Id::new("browse_play_focused_card_control")
}

/// Cached filter result so typing or scrolling never rescans the catalogue.
#[derive(Default)]
pub(super) struct BrowsePlayState {
    /// The A-Z strip: where each letter first appears in the list shown, for
    /// the list it was made from (address and length of the shown indices).
    alpha: Option<(usize, usize, [Option<usize>; ALPHA_BUCKETS])>,
    /// A jump asked for by the strip, applied by the shelf in the same pass.
    alpha_jump: Option<usize>,
    /// The game to focus once the shelf has drawn it (after a jump).
    focus_game: Option<i64>,
    /// The card holding keyboard focus this frame: (control, game id).
    focused_card: Option<(egui::Id, i64)>,
    /// Where the shelf's scrolling viewport was drawn last frame.
    pub(super) shelf_rect: Option<egui::Rect>,
    key: Option<FilterKey>,
    indices: Arc<Vec<usize>>,
    applied_generation: Option<u64>,
    // Keep the allocation token alive so pointer-based keys cannot suffer ABA.
    library_token: std::sync::Weak<super::library::Library>,
    /// The shelf starts from the top again when the search or system changes.
    reset_scroll: bool,
    /// Plain-language reason some covers are missing, taken from the covers
    /// drawn last frame, so it is said once instead of on every card.
    cover_note: Option<String>,
    /// The selected game and when it was first shown, for the settle delay.
    settling: Option<(i64, f64)>,
    /// Games whose readiness check has finished positive. Filled only by the
    /// selection check; the Ready view never plans launches in bulk.
    pub(super) readiness: ReadinessCache,
    /// The readiness view of `indices`, rebuilt only when the base list or
    /// the cache changes: (base list address, cache version, filter, result).
    readiness_view: Option<ReadinessView>,
    /// Games in the shown list missing the sort value:
    /// (list address, list length, order, count).
    sort_note: Option<(usize, usize, SortOrder, usize)>,
    /// Background checks for the games on screen.
    background: super::bg_readiness::Scheduler,
    /// The emulator-state snapshot the background checks run against, and the
    /// signature it was taken at; a different signature invalidates both.
    env: Option<(u64, super::bg_readiness::EnvSnapshot)>,
    env_signature: Option<u64>,
    /// The library snapshot generation the settled decisions were made at.
    database_generation: Option<u64>,
    /// Ids drawn this frame / last frame, nearest the top first.
    visible_now: Vec<i64>,
    visible: Vec<i64>,
    /// The Surprise Me picker's filter, seeded generator and current pick.
    pub(super) surprise: super::surprise_me::SurpriseState,
}

impl BrowsePlayState {
    /// The library snapshot generation moved: a rescan may have added a disc
    /// set or changed what a game belongs to, so every earlier answer (cached
    /// or settled) is dropped. Foreground answers re-record as games are drawn.
    fn note_database_generation(&mut self, generation: Option<u64>) {
        if self.database_generation != generation {
            self.database_generation = generation;
            self.background.invalidate();
            self.readiness.clear();
        }
    }

    /// Seconds still to wait before `game`'s readiness check may start.
    fn settle_remaining(&mut self, game: i64, now: f64) -> f64 {
        let since = match self.settling {
            Some((current, since)) if current == game => since,
            _ => {
                self.settling = Some((game, now));
                now
            }
        };
        (SELECTION_SETTLE_SECS - (now - since)).max(0.0)
    }
}

#[cfg(test)]
impl BrowsePlayState {
    pub(super) fn reset_scroll_pending(&self) -> bool {
        self.reset_scroll
    }
    pub(super) fn note_generation_for_test(&mut self, generation: Option<u64>) {
        self.note_database_generation(generation);
    }
    pub(super) fn is_ready_for_test(&self, game: &Game) -> bool {
        self.readiness.get(game).is_some()
    }
    pub(super) fn record_readiness(&mut self, game: &Game, result: Option<CheckedReadiness>) {
        self.readiness.record(game, result);
    }
}

fn browse_primary(ui: &mut egui::Ui, label: &str) -> bool {
    let fill = theme::primary_action_fill(ui);
    ui.add(
        egui::Button::new(RichText::new(label).strong())
            .fill(fill)
            .min_size(egui::vec2(120.0, 40.0)),
    )
    .clicked()
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn filtered_game_indices(app: &App) -> Vec<usize> {
    let base = app.library.filter(&app.filter);
    apply_readiness(&app.library, base, &app.filter, &app.browse_play.readiness)
}

/// Narrow a catalogue-filtered list to the readiness view.
fn apply_readiness(
    library: &super::library::Library,
    base: Vec<usize>,
    filter: &Filter,
    cache: &ReadinessCache,
) -> Vec<usize> {
    if !filter.uses_cache() {
        return base;
    }
    base.into_iter()
        .filter(|index| super::library::matches(&library.games[*index], filter, cache))
        .collect()
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
pub(super) fn thousands(value: usize) -> String {
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

/// The card's status words and colour; Blue uses its own status tokens.
fn status_text(game: &Game, blue: bool) -> (&'static str, Color32) {
    if game.is_preservation_only() {
        (
            super::library::PRESERVATION_ONLY_STATUS,
            if blue {
                theme::blue::SECONDARY_TEXT
            } else {
                theme::SECONDARY_TEXT
            },
        )
    } else if game.attention {
        (
            "Needs attention",
            if blue {
                theme::blue::WARN
            } else {
                ATTENTION_TEXT
            },
        )
    } else {
        match game.identity_summary() {
            "Verified" => (
                "Verified",
                if blue {
                    theme::blue::OK
                } else {
                    theme::SUCCESS
                },
            ),
            other => (
                other,
                if blue {
                    theme::blue::SECONDARY_TEXT
                } else {
                    theme::SECONDARY_TEXT
                },
            ),
        }
    }
}

/// Shown under Play in Blue (board `D10-GP-SelectVsPlay`).
pub(super) const SELECT_VS_PLAY: &str =
    "Clicking a cover only selects it. Press Play to start the game when it is ready.";
/// Preservation-only media has no launch route, so it gets this instead of
/// a readiness check.
pub(super) const PRESERVATION_ONLY_SENTENCE: &str = "A recording of a play session, kept for \
     preservation. EmuWiz has no verified replay workflow, so Play is not offered.";

/// One sentence that says what Play will do for this game.
fn readiness_sentence(game: &Game) -> &'static str {
    if game.attention {
        "Something needs attention before this can start. Game Details shows what."
    } else if game.identity_summary() == "Mismatch" {
        "Game identity evidence conflicts. Review it before play."
    } else if game.identified {
        "Play starts this game when it is ready."
    } else if game.platform != super::library::UNKNOWN_PLATFORM {
        "Play checks that this game can start, then starts it. Catalogue (DAT) verification is optional."
    } else {
        "More platform evidence is needed. Assign the platform or review the source folder."
    }
}

/// Platform, identity and DAT state on one line, without conflating them.
fn status_line(
    game: &Game,
    width: f32,
    blue: bool,
    not_ready: Option<&str>,
) -> egui::text::LayoutJob {
    let (status, colour) = status_text(game, blue);
    let status = match not_ready {
        Some(reason) => format!("Not ready · {reason}"),
        None => status.to_string(),
    };
    let colour = if not_ready.is_some() {
        if blue {
            theme::blue::WARN
        } else {
            ATTENTION_TEXT
        }
    } else {
        colour
    };
    let font = egui::FontId::proportional(theme::METADATA_SIZE);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &format!("{} · {status}", game.platform),
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: colour,
            ..Default::default()
        },
    );
    job.append(
        &format!("  ·  DAT: {}", game.dat_identity_summary()),
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
        break_anywhere: false,
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
        // Decided before anything is drawn: a widget that overflows widens the
        // parent, and the layout below must not be sized from that.
        // Leaves the right-hand gutter, so the selected-game panel and the
        // view switch never sit flush against the window edge.
        let page_width = ui.available_width() - theme::SPACE_MD;

        let sort_in_toolbar = self.browse_play_toolbar(ui);
        self.batch_selection
            .retain(|id| library.by_id.contains_key(&id));
        self.browse_play_batch_controls(ui, &library);
        ui.add_space(theme::SPACE_XS);
        self.browse_play_systems(ui, &library, !sort_in_toolbar);
        let indices = self.browse_play_indices();
        self.library_selection_shortcuts(ui.ctx(), &indices);
        self.browse_play_sort_note(ui, &library, &indices);
        self.library_selection_bar(ui, &indices);
        self.library_selection_claim_escape(ui.ctx());
        let blue = theme::blue_profile(ui);
        self.browse_play_background(ui.ctx(), blue);
        ui.add_space(theme::SPACE_XS);
        ui.separator();

        let size = egui::vec2(page_width, ui.available_height());
        let height = size.y.max(260.0);
        let side_by_side_min = if theme::blue_profile(ui) {
            SIDE_BY_SIDE_MIN_WIDTH
        } else {
            900.0
        };
        if size.x >= side_by_side_min {
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
        self.browse_play_batch_window(ui.ctx());
        self.surprise_me_window(ui.ctx());
    }

    fn browse_play_batch_controls(&mut self, ui: &mut egui::Ui, library: &super::library::Library) {
        if !self.batch_selecting && self.batch_selection.len() == 0 && !self.batch_state.active() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} / 25 selected for artwork",
                self.batch_selection.len()
            ))
            .on_hover_text(SELECT_FOR_ARTWORK_HINT);
            if ui
                .add_enabled(
                    self.batch_selection.len() > 0 || self.batch_state.active(),
                    egui::Button::new(if self.batch_state.active() {
                        "Resume batch"
                    } else {
                        "Enrich metadata"
                    }),
                )
                .clicked()
            {
                if !self.batch_state.active() {
                    let games = self
                        .batch_selection
                        .ids()
                        .filter_map(|id| library.game(id))
                        .map(|game| (game.archive.clone(), game.screenscraper.clone()))
                        .collect();
                    self.batch_message = self.batch_state.prepare_blue(games).err();
                }
                self.batch_open = true;
            }
            if ui.small_button("Clear selection").clicked() {
                self.batch_selection.clear();
            }
        });
        if self.batch_selecting {
            ui.add(
                egui::Label::new(
                    RichText::new(ARTWORK_PICKER_PURPOSE)
                        .size(theme::METADATA_SIZE)
                        .color(theme::SECONDARY_TEXT),
                )
                .wrap(),
            );
        }
        if let Some(message) = &self.batch_message {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
    }

    fn browse_play_batch_window(&mut self, context: &egui::Context) {
        if !self.batch_open {
            return;
        }
        let mut open = self.batch_open;
        egui::Window::new("Batch metadata enrichment")
            .open(&mut open).resizable(true).vscroll(true)
            .default_width(580.0).max_width(700.0).max_height(500.0)
            .show(context, |ui| {
                let workflows = self.native_workflows.get_or_insert_with(||
                    super::native_workflows::NativeWorkflows::new(context.clone()));
                if let Some(crate::screenscraper_enrichment_page::ScreenScraperEnrichmentAction::Apply { archive_id, values, receipt }) =
                    crate::screenscraper_batch_enrichment_page::show_blue(
                        ui, &mut self.batch_state, &workflows.app.screenscraper_page)
                {
                    let result = self.batch_state.current_blue_write(archive_id)
                        .ok_or_else(|| "The reviewed game is no longer current.".to_string())
                        .and_then(|(expected, prior_values, prior_receipt)| {
                            archivefs_core::default_database_path()
                                .and_then(archivefs_core::Database::open_existing)
                                .and_then(|mut db| db.apply_screenscraper_enrichment_if_current(&expected, prior_values.as_ref(), prior_receipt.as_ref(), &values, &receipt))
                                .map_err(|error| error.to_string())
                        });
                    match result {
                        Ok(()) => {
                            self.batch_state.mark_applied(archive_id);
                            self.cache_applied_batch_enrichment(
                                archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment {
                                    archive_id, values, receipt,
                                },
                            );
                        }
                        Err(error) => self.batch_state.mark_apply_failed(archive_id, error),
                    }
                }
                if self.batch_state.active() { context.request_repaint_after(std::time::Duration::from_millis(100)); }
            });
        self.batch_open = open;
    }

    pub(super) fn cache_applied_batch_enrichment(
        &mut self,
        saved: archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment,
    ) {
        if let Some(index) = self.library.by_id.get(&saved.archive_id).copied() {
            Arc::make_mut(&mut self.library).games[index].screenscraper = Some(saved);
            // A resolved row may already contain the previous provider values.
            // reload also advances the generation when indexing is in flight, so
            // an older worker cannot publish its stale snapshot afterward.
            self.artwork.reload(self.library.clone());
        }
    }

    /// The games matching the current search and system. Recomputed only when
    /// the filter or the library changes.
    pub(super) fn browse_play_indices(&mut self) -> Arc<Vec<usize>> {
        let key = (
            self.filter.platform.clone(),
            self.filter.search.trim().to_lowercase(),
            (self.filter.readiness, self.filter.cache_aware),
            (self.filter.unverified_only, self.filter.sort),
            Arc::as_ptr(&self.library) as usize,
        );
        if self.browse_play.key.as_ref() != Some(&key) {
            self.browse_play.reset_scroll = true;
            if self
                .browse_play
                .key
                .as_ref()
                .is_some_and(|old| old.4 != key.4)
            {
                // A different catalogue: earlier answers no longer describe it.
                self.browse_play.readiness.clear();
                self.browse_play.background.invalidate();
            }
            self.browse_play.key = Some(key);
            self.browse_play.library_token = Arc::downgrade(&self.library);
            if self.library.games.len() <= IMMEDIATE_FILTER_LIMIT {
                self.browse_play.indices = Arc::new(self.library.filter(&self.filter));
            } else {
                self.browse_play.indices = Arc::default();
                self.browse_play.applied_generation = None;
                self.change_filter();
            }
        }
        if self.library.games.len() > IMMEDIATE_FILTER_LIMIT
            && self.filter_dirty.is_none()
            && !self.filter_inflight
            && self.browse_play.applied_generation != Some(self.filter_generation)
        {
            self.browse_play.indices = Arc::new(self.indices.clone());
            self.browse_play.applied_generation = Some(self.filter_generation);
        }
        let base = self.browse_play.indices.clone();
        let filter = self.filter.readiness;
        if !self.filter.uses_cache() {
            self.browse_play.readiness_view = None;
            return base;
        }
        let token = Arc::as_ptr(&base) as usize;
        let version = self.browse_play.readiness.version();
        if let Some((t, v, f, view)) = &self.browse_play.readiness_view
            && (*t, *v, *f) == (token, version, filter)
        {
            return view.clone();
        }
        let view = Arc::new(apply_readiness(
            &self.library,
            base.to_vec(),
            &self.filter,
            &self.browse_play.readiness,
        ));
        self.browse_play.readiness_view = Some((token, version, filter, view.clone()));
        view
    }

    /// Drop every readiness answer whose inputs moved: the library snapshot
    /// (disc sets, catalogue rows) or what the checks read (emulator scans,
    /// BIOS evidence, emulator choice, settings). Until a game has been
    /// selected once, no scan has run and there is nothing to check against.
    fn sync_readiness_freshness(&mut self) {
        let state = &mut self.browse_play;
        let generation = self
            .native_workflows
            .as_ref()
            .map(|w| w.database_generation());
        state.note_database_generation(generation);
        let readiness = self
            .native_workflows
            .as_ref()
            .map(|w| w.emulator_readiness());
        // One signature, and one reuse window for its expensive part, shared
        // with the selected game's drawn launch input.
        let signature = self
            .native_workflows
            .as_ref()
            .and_then(|w| w.environment_signature(std::time::Instant::now()));
        if state.env_signature != signature {
            state.env_signature = signature;
            state.background.invalidate();
            state.readiness.clear();
            state.env =
                signature.zip(readiness.and_then(super::bg_readiness::EnvSnapshot::capture));
        }
    }

    /// Record a launch check another screen (Game Details, Setup & Doctor,
    /// Big Picture) just showed for `game` in the Library's one readiness
    /// cache, so the Ready view agrees with it. Freshness is checked first,
    /// so the answer is stored against the inputs it was taken under; a
    /// checking, stale or partial answer forgets the game (Not checked).
    /// Preservation-only media gets no launch check here, as in the Library.
    pub(super) fn note_checked_readiness(
        &mut self,
        game: &Game,
        view: &super::launch_readiness_summary::CompactReadiness,
    ) {
        if game.is_preservation_only() {
            return;
        }
        self.sync_readiness_freshness();
        let result = CheckedReadiness::from_compact(view);
        self.browse_play.readiness.record(game, result);
    }

    /// Keep the background readiness checks in step with what is on screen.
    /// Blue only. Never selects a game, loads evidence for the details panel,
    /// or starts any scan; it waits until the foreground has finished them.
    fn browse_play_background(&mut self, context: &egui::Context, blue: bool) {
        if !blue {
            return;
        }
        self.sync_readiness_freshness();
        let state = &mut self.browse_play;
        for (id, outcome) in state.background.drain() {
            if let Some(game) = self.library.game(id) {
                super::bg_readiness::record_outcome(&mut state.readiness, game, outcome);
            }
        }
        // The games drawn last frame, in order and without repeats.
        state.visible = std::mem::take(&mut state.visible_now);
        state.visible.dedup();
        let Some((_, env)) = &state.env else {
            return;
        };
        let jobs = state.background.pick(
            state.visible.iter().filter_map(|id| self.library.game(*id)),
            |game| state.readiness.get(game).is_some(),
        );
        for mut job in jobs {
            // Fail closed: with no workflows or no library snapshot the disc
            // set context is unknown, so the game is not answered (yet).
            job.multidisc = self
                .native_workflows
                .as_ref()
                .is_none_or(|w| w.has_multidisc_context(&job.archive.absolute_path));
            if job.multidisc {
                state.background.settle(job.id);
                continue;
            }
            state.background.spawn(env, job, context.clone());
        }
    }

    /// Returns whether the sort control was drawn here. A narrow window
    /// stacks the toolbar, and the sort control then joins the view switch's
    /// row, which has room, instead of wrapping the systems row onto a line
    /// of its own and pushing the game panel down.
    fn browse_play_toolbar(&mut self, ui: &mut egui::Ui) -> bool {
        // The view switch keeps its width; the search takes what is left and
        // shrinks before it can run over List/Grid. When even a small search
        // cannot share the row, the switch drops to its own row.
        let font = egui::TextStyle::Button.resolve(ui.style());
        let pad = 2.0 * ui.spacing().button_padding.x;
        let gap = ui.spacing().item_spacing.x;
        let measure = |text: &str| super::title_fit::text_width(ui, text, &font) + pad;
        // The artwork toggle is laid out at its narrowest labels; a longer
        // one is shown only where the row has room left for it.
        let toggle = measure("Done selecting").max(measure(SELECT_FOR_ARTWORK_SHORT));
        let full_extra = (measure(SELECT_FOR_ARTWORK) - toggle).max(0.0);
        let medium_extra = (measure(SELECT_FOR_ARTWORK_MEDIUM) - toggle).max(0.0);
        let switch = measure("List") + measure("Grid") + toggle + measure(BIG_PICTURE) + 3.0 * gap;
        let clear = if self.filter.search.is_empty() {
            0.0
        } else {
            measure("Clear") + gap
        };
        let blue = theme::blue_profile(ui);
        let attention = if blue {
            ReadinessFilter::ALL
                .iter()
                .map(|filter| measure(filter.label()) - pad + 2.0 * READINESS_BUTTON_PAD + 2.0)
                .sum::<f32>()
                + gap
        } else {
            0.0
        };
        let available = ui.available_width();
        let room = available - switch - clear - attention - gap;
        let search = room.min(300.0);
        let stacked = search < SEARCH_MIN_WIDTH;
        // On one row a longer artwork label may take width from the search,
        // never below the search's minimum.
        let artwork_in_row = artwork_label_for(room, full_extra, medium_extra, SEARCH_MIN_WIDTH);
        let search = if stacked {
            search
        } else {
            match artwork_in_row {
                ArtworkLabel::Full => (room - full_extra).min(300.0),
                ArtworkLabel::Medium => (room - medium_extra).min(300.0),
                ArtworkLabel::Short => search,
            }
        };
        let search = if stacked {
            (available - clear - gap).clamp(SEARCH_MIN_WIDTH.min(available), 300.0)
        } else {
            search
        };
        let attention_filter = |this: &mut Self, ui: &mut egui::Ui| {
            if !blue {
                return;
            }
            if stacked {
                // A narrow window keeps one control's width: the same four
                // choices in a dropdown instead of a four-button row.
                let before = this.filter.readiness;
                egui::ComboBox::from_id_salt("readiness_filter")
                    .selected_text(this.filter.readiness.label())
                    .show_ui(ui, |ui| {
                        for choice in ReadinessFilter::ALL {
                            ui.selectable_value(&mut this.filter.readiness, choice, choice.label())
                                .on_hover_text(readiness_filter_hint(choice));
                        }
                    });
                if this.filter.readiness != before {
                    this.filter.cache_aware = true;
                }
                return;
            }
            // One tight segmented group, so four choices stay on the row.
            ui.scope(|ui| {
                ui.spacing_mut().button_padding.x = READINESS_BUTTON_PAD;
                ui.spacing_mut().item_spacing.x = 2.0;
                for choice in ReadinessFilter::ALL {
                    if ui
                        .add(toggle_button(
                            choice.label(),
                            this.filter.readiness == choice,
                        ))
                        .on_hover_text(readiness_filter_hint(choice))
                        .clicked()
                    {
                        this.filter.readiness = choice;
                        // Blue folds finished launch checks into these views.
                        this.filter.cache_aware = true;
                    }
                }
            });
        };
        let view_switch = |this: &mut Self, ui: &mut egui::Ui| {
            view_toggle(ui, &mut this.filter.list, false, "Grid");
            view_toggle(ui, &mut this.filter.list, true, "List");
        };
        ui.horizontal(|ui| {
            let field = ui.add_sized(
                [search, 34.0],
                egui::TextEdit::singleline(&mut self.filter.search)
                    .hint_text("Search by title or system")
                    .desired_width(search),
            );
            super::pad_shell::mark_suggestions(&field);
            if !self.filter.search.is_empty() && ui.button("Clear").clicked() {
                self.filter.search.clear();
            }
            if !stacked {
                attention_filter(self, ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    view_toggle(ui, &mut self.filter.list, true, "List");
                    view_toggle(ui, &mut self.filter.list, false, "Grid");
                    self.browse_play_selection_toggle(ui, artwork_in_row);
                    self.big_picture_button(ui);
                });
            }
        });
        if stacked {
            ui.horizontal_wrapped(|ui| {
                attention_filter(self, ui);
                view_switch(self, ui);
                // The very narrow layout: the short label keeps Big Picture
                // and the Sort box on this row. The full name is announced on
                // focus and the picker states its purpose once open.
                self.browse_play_selection_toggle(ui, ArtworkLabel::Short);
                self.big_picture_button(ui);
                self.browse_play_sort_control(ui);
            });
        }
        if blue
            && let Some(note) = readiness_filter_note(
                self.filter.readiness,
                self.browse_play.readiness.len(),
                self.library.games.len(),
                self.browse_play.background.in_flight() > 0,
            )
        {
            ui.add(
                egui::Label::new(
                    RichText::new(note)
                        .size(theme::METADATA_SIZE)
                        .color(theme::blue::SECONDARY_TEXT),
                )
                .wrap(),
            );
        }
        stacked
    }

    /// The sort control. It sits at the end of the systems row (or of the
    /// stacked toolbar's second row), so it adds no row of its own: when the full "Sort: …" label
    /// does not fit on what is left of the row, a short "Sort" box takes its
    /// place and names the current order on hover.
    fn browse_play_sort_control(&mut self, ui: &mut egui::Ui) {
        let full = sort_button_text(self.filter.sort);
        // A combo box asks the row only for the space left on it, so the
        // wrapping row never moves it down and its label runs off the edge.
        // Claiming the label's width first lets the row place it.
        let spacing = ui.spacing();
        let font = egui::TextStyle::Button.resolve(ui.style());
        let icon = spacing.icon_spacing + spacing.icon_width;
        let pad = spacing.button_padding.x;
        let combo_width = spacing.combo_width;
        let height = spacing.interact_size.y;
        let remaining = ui.available_rect_before_wrap().width();
        let full_width =
            (super::title_fit::text_width(ui, &full, &font) + icon + 2.0 * pad).max(combo_width);
        // The short box also trims its side padding: on a narrow Blue window
        // that is what keeps it on the row instead of wrapping it.
        let (text, width, pad) = if full_width <= remaining {
            (full, full_width, pad)
        } else {
            let short_pad = pad.min(SORT_SHORT_PAD);
            let width = (super::title_fit::text_width(ui, SORT_SHORT_LABEL, &font)
                + icon
                + 2.0 * short_pad)
                .ceil();
            (SORT_SHORT_LABEL.to_owned(), width, short_pad)
        };
        let hover = format!(
            "{}. {}",
            sort_button_text(self.filter.sort),
            self.filter.sort.hint()
        );
        ui.allocate_ui(egui::vec2(width, height), |ui| {
            ui.spacing_mut().button_padding.x = pad;
            egui::ComboBox::from_id_salt("library_sort")
                .selected_text(text)
                .width(width)
                .wrap_mode(egui::TextWrapMode::Extend)
                .show_ui(ui, |ui| {
                    for choice in SortOrder::ALL {
                        ui.selectable_value(&mut self.filter.sort, choice, choice.label())
                            .on_hover_text(choice.hint());
                    }
                })
                .response
                .on_hover_text(hover);
        });
    }

    /// When the list is sorted by a value some shown games lack, says how
    /// many, so a block of them at the end is explained. Nothing is drawn
    /// otherwise. The count is made once per list.
    fn browse_play_sort_note(
        &mut self,
        ui: &mut egui::Ui,
        library: &super::library::Library,
        indices: &Arc<Vec<usize>>,
    ) {
        let sort = self.filter.sort;
        let Some(noun) = sort.missing_noun() else {
            return;
        };
        let key = (Arc::as_ptr(indices) as usize, indices.len(), sort);
        let missing = match self.browse_play.sort_note {
            Some((k0, k1, k2, count)) if (k0, k1, k2) == key => count,
            _ => {
                let count = library.missing_sort_value(indices, sort);
                self.browse_play.sort_note = Some((key.0, key.1, key.2, count));
                count
            }
        };
        if let Some(note) = sort_missing_note(missing, indices.len(), noun) {
            ui.add(
                egui::Label::new(
                    RichText::new(note)
                        .size(theme::METADATA_SIZE)
                        .color(theme::SECONDARY_TEXT),
                )
                .wrap(),
            );
        }
    }

    fn big_picture_button(&mut self, ui: &mut egui::Ui) {
        if ui
            .button(BIG_PICTURE)
            .on_hover_text("Browse and launch from the sofa with a controller")
            .clicked()
        {
            self.enter_big_picture(ui.ctx());
        }
    }

    fn browse_play_surprise_button(&mut self, ui: &mut egui::Ui) {
        if ui
            .button(SURPRISE_ME)
            .on_hover_text("Pick a random game from your library")
            .clicked()
        {
            self.open_surprise_me();
        }
    }

    fn browse_play_verify_button(&mut self, ui: &mut egui::Ui) {
        let hint = if self.filter.platform.is_empty() {
            "Check your games against trusted catalogues. Opens Check Games; no game file is read until you preview and confirm.".to_string()
        } else {
            format!(
                "Check your {} games against trusted catalogues. Opens Check Games; no game file is read until you preview and confirm.",
                self.filter.platform
            )
        };
        if ui.button(VERIFY_GAMES).on_hover_text(hint).clicked() {
            self.open_verify_games();
        }
    }

    /// Navigation only: the existing Check Games page, on the system chosen in
    /// the Library when there is one, otherwise on its platform list. That
    /// page owns the summary, catalogue setup, Preview and confirmation.
    pub(super) fn open_verify_games(&mut self) {
        let platform = self.filter.platform.clone();
        if platform.is_empty() {
            self.check_platform = None;
            self.verification = None;
            self.go(Route::Section(Section::Check));
        } else {
            self.check_platform(platform);
        }
    }

    /// The ScreenScraper artwork picker toggle, with the label that fits its
    /// row. Keyboard focus and assistive tech always get the full name.
    fn browse_play_selection_toggle(&mut self, ui: &mut egui::Ui, label: ArtworkLabel) {
        let selecting = self.batch_selecting;
        let response = ui
            .button(if selecting {
                "Done selecting"
            } else {
                label.text()
            })
            .on_hover_text(SELECT_FOR_ARTWORK_HINT);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                if selecting {
                    "Done selecting for artwork"
                } else {
                    SELECT_FOR_ARTWORK_NAME
                },
            )
        });
        if response.clicked() {
            self.batch_selecting = !self.batch_selecting;
        }
    }

    fn browse_play_systems(
        &mut self,
        ui: &mut egui::Ui,
        library: &super::library::Library,
        with_sort: bool,
    ) {
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
                    // Tall enough for the whole list but never taller than the
                    // window, so the menu is not cut off on short screens.
                    .height((ui.ctx().screen_rect().height() * 0.75).clamp(160.0, 720.0))
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
            self.browse_play_verify_button(ui);
            self.browse_play_surprise_button(ui);
            if with_sort {
                self.browse_play_sort_control(ui);
            }
        });
        if let Some(platform) = chosen {
            self.choose_system(platform);
        }
    }

    /// Applies a system chip. A selected game the new system hides is
    /// deselected, so the panel never shows (for example) an Acorn Electron
    /// game while the list is filtered to Acorn Archimedes.
    pub(super) fn choose_system(&mut self, platform: String) {
        self.filter.select_platform(platform);
        if let Route::BrowsePlayGame(id) = self.router.current
            && !self.filter.platform.is_empty()
            && self
                .library
                .game(id)
                .is_none_or(|game| game.platform != self.filter.platform)
        {
            self.go(Route::BrowsePlay);
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
            theme::primary_action_fill(ui)
        } else if hovered {
            theme::raised_fill(ui)
        } else {
            theme::card_fill(ui)
        };
        ui.painter().rect_filled(rect, 16.0, fill);
        ui.painter().rect_stroke(
            rect,
            16.0,
            egui::Stroke::new(
                1.0_f32,
                if selected && theme::blue_profile(ui) {
                    theme::blue::SELECTED_OUTLINE
                } else if selected {
                    theme::PRIMARY_ACTION_HOVER
                } else {
                    theme::border_color(ui)
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
        if library.games.len() > IMMEDIATE_FILTER_LIMIT
            && (self.filter_dirty.is_some() || self.filter_inflight)
        {
            ui.spinner();
            ui.label("Updating results…");
            return;
        }
        self.browse_play_summary(ui, indices.len());
        if indices.is_empty() {
            self.browse_play_empty(ui, library.games.is_empty());
            return;
        }

        self.browse_play_alpha_strip(ui, library, indices);
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
            // A card focused from the keyboard must be in view in the pass
            // that finds it, not after a scroll animation.
            .animated(false)
            .max_height(scroll_height);
        if reset {
            area = area.vertical_scroll_offset(0.0);
        }
        if let Some(position) = self.browse_play.alpha_jump.take() {
            // Land on the letter's first game: scroll its row to the top and
            // hand it keyboard/controller focus when it is drawn.
            let row = (position / columns) as f32;
            area = area.vertical_scroll_offset(row * (row_height + spacing.y));
            self.browse_play.focus_game = indices
                .get(position)
                .and_then(|i| library.games.get(*i))
                .map(|game| game.archive.id);
        }
        // Virtualised like `show_rows` (which already draws one row below the
        // viewport), plus one row ABOVE it: a controller move to the row above
        // the first visible one must find a card there, or it escapes to the
        // controls over the shelf instead of scrolling up.
        self.browse_play.focused_card = None;
        let row_step = row_height + spacing.y;
        // A controller Up/Down on a card: move one grid row in the list shown
        // (spatial search at the edge of a scrolling shelf would miss it).
        if let Some(step) = ui.data_mut(|d| d.remove_temp::<i32>(row_step_id())) {
            let current = ui.data(|d| d.get_temp::<i64>(focused_card_id()));
            let position = current.and_then(|game| {
                indices
                    .iter()
                    .position(|i| library.games.get(*i).is_some_and(|g| g.archive.id == game))
            });
            let target = position
                .map(|p| p as isize + step as isize * columns as isize)
                .filter(|t| *t >= 0 && (*t as usize) < indices.len());
            match target.and_then(|t| library.games.get(indices[t as usize])) {
                Some(game) => self.browse_play.focus_game = Some(game.archive.id),
                None => ui.ctx().memory_mut(|m| {
                    m.move_focus(if step < 0 {
                        egui::FocusDirection::Up
                    } else {
                        egui::FocusDirection::Down
                    })
                }),
            }
        }
        let shelf = area.show_viewport(ui, |ui, viewport| {
            ui.set_height((row_step * rows as f32 - spacing.y).max(0.0));
            let first = ((viewport.min.y / row_step).floor().max(0.0) as usize).saturating_sub(1);
            let mut last = (viewport.max.y / row_step).ceil() as usize + 1;
            let mut first = first;
            if last > rows {
                let span = last.saturating_sub(first);
                last = rows;
                first = rows.saturating_sub(span);
            }
            let top = ui.max_rect().top();
            let band = egui::Rect::from_x_y_ranges(
                ui.max_rect().x_range(),
                (top + first as f32 * row_step)..=(top + last as f32 * row_step),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(band), |ui| {
                ui.skip_ahead_auto_ids(first);
                for row in first..last {
                    let start = row * columns;
                    let end = (start + columns).min(indices.len());
                    ui.horizontal_top(|ui| {
                        for position in start..end {
                            let Some(game) = library.games.get(indices[position]) else {
                                continue;
                            };
                            let selected = selected_id == Some(game.archive.id);
                            if list {
                                self.browse_play_list_row(
                                    ui,
                                    game,
                                    selected,
                                    indices,
                                    &mut cover_note,
                                );
                            } else {
                                self.browse_play_card(
                                    ui,
                                    game,
                                    selected,
                                    card_width,
                                    indices,
                                    &mut cover_note,
                                );
                            }
                        }
                    });
                }
            });
        });
        ui.data_mut(|d| match self.browse_play.focused_card {
            Some((control, game)) => {
                d.insert_temp(focused_card_id(), game);
                d.insert_temp(focused_card_control_id(), control);
            }
            None => {
                d.remove::<i64>(focused_card_id());
                d.remove::<egui::Id>(focused_card_control_id());
            }
        });
        self.browse_play.shelf_rect = Some(shelf.inner_rect);
        self.browse_play.cover_note = cover_note;
    }

    /// `#` and A-Z over the list shown: each letter jumps to its first game,
    /// and a letter with no game is disabled (so the controller skips it). It
    /// follows the current sort, filter and search because it is made from
    /// the list the shelf is showing; it is rebuilt only when that list changes.
    fn browse_play_alpha_strip(
        &mut self,
        ui: &mut egui::Ui,
        library: &super::library::Library,
        indices: &Arc<Vec<usize>>,
    ) {
        // Letters only mean something while the list is in title order.
        if indices.len() < ALPHA_STRIP_MIN
            || !matches!(self.filter.sort, SortOrder::TitleAsc | SortOrder::TitleDesc)
        {
            return;
        }
        let key = (Arc::as_ptr(indices) as usize, indices.len());
        if self.browse_play.alpha.as_ref().map(|a| (a.0, a.1)) != Some(key) {
            let mut first = [None; ALPHA_BUCKETS];
            for (position, index) in indices.iter().enumerate() {
                if let Some(game) = library.games.get(*index) {
                    let slot = &mut first[bucket_for_title(&game.title)];
                    if slot.is_none() {
                        *slot = Some(position);
                    }
                }
            }
            self.browse_play.alpha = Some((key.0, key.1, first));
        }
        let first = self
            .browse_play
            .alpha
            .as_ref()
            .map(|a| a.2)
            .unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (bucket, position) in first.iter().enumerate() {
                let button = egui::Button::new(RichText::new(bucket_label(bucket)).strong())
                    .min_size(egui::vec2(28.0, 28.0));
                let response = ui
                    .add_enabled(position.is_some(), button)
                    .on_hover_text(format!("Jump to {}", bucket_label(bucket)));
                if response.clicked()
                    && let Some(position) = position
                {
                    self.browse_play.alpha_jump = Some(*position);
                }
            }
        });
        ui.add_space(theme::SPACE_XS);
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
            if let Some((note, detail)) = retired_note(&self.library) {
                ui.label(
                    RichText::new(note)
                        .size(theme::METADATA_SIZE)
                        .color(theme::SECONDARY_TEXT),
                )
                .on_hover_text(detail);
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

    /// Tab (or Shift+Tab) onto a card scrolls the shelf so the whole card is
    /// shown. Pointer presses and the wheel never scroll it.
    fn reveal_keyboard_focus(ui: &egui::Ui, response: &egui::Response) {
        let pointer = ui.input(|input| input.pointer.any_pressed() || input.pointer.any_released());
        // egui reports `gained_focus` for Tab but not for a directional
        // (controller) move, so notice the change ourselves: the focused card
        // is revealed once, in the pass that finds it newly focused.
        let key = egui::Id::new("browse_play_revealed_card");
        let revealed = ui.data(|d| d.get_temp::<egui::Id>(key));
        if response.has_focus() {
            if revealed != Some(response.id) {
                ui.data_mut(|d| d.insert_temp(key, response.id));
                if !pointer {
                    ui.scroll_to_rect(response.rect, None);
                }
            }
        } else if revealed == Some(response.id) {
            ui.data_mut(|d| d.remove::<egui::Id>(key));
        }
    }

    /// `selected` is the game open in the panel; `in_set` is a game in the
    /// multi-select set. Both get the selected look; a set member's check mark
    /// is painted by `paint_selection_mark` once the content is drawn.
    fn card_background(
        ui: &egui::Ui,
        rect: egui::Rect,
        selected: bool,
        in_set: bool,
        response: &egui::Response,
    ) {
        Self::card_background_fill(ui, rect, selected || in_set, response);
    }

    /// The check mark of a multi-select set member, painted after the card's
    /// content so the cover cannot hide it.
    fn paint_selection_mark(ui: &egui::Ui, rect: egui::Rect, in_set: bool) {
        if in_set {
            let centre = egui::pos2(rect.right() - 14.0, rect.top() + 14.0);
            let painter = ui.painter();
            painter.circle_filled(centre, 9.0, theme::primary_action_fill(ui));
            let stroke = egui::Stroke::new(2.0_f32, egui::Color32::WHITE);
            painter.line_segment(
                [
                    centre + egui::vec2(-4.5, 0.0),
                    centre + egui::vec2(-1.0, 3.5),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    centre + egui::vec2(-1.0, 3.5),
                    centre + egui::vec2(4.5, -3.5),
                ],
                stroke,
            );
        }
    }

    fn card_background_fill(
        ui: &egui::Ui,
        rect: egui::Rect,
        selected: bool,
        response: &egui::Response,
    ) {
        let hovered = response.hovered();
        let blue = theme::blue_profile(ui);
        let fill = if selected && blue {
            theme::blue::SELECTED_FILL
        } else if selected || hovered {
            theme::raised_fill(ui)
        } else {
            theme::card_fill(ui)
        };
        ui.painter().rect_filled(rect, 10.0, fill);
        let stroke = if selected && blue {
            egui::Stroke::new(2.0_f32, theme::blue::SELECTED_OUTLINE)
        } else if selected {
            egui::Stroke::new(2.0_f32, theme::PRIMARY_ACTION_HOVER)
        } else if hovered {
            egui::Stroke::new(1.0_f32, theme::BORDER_FOCUS)
        } else {
            theme::border(ui)
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
        list: &[usize],
        note: &mut Option<String>,
    ) {
        let id = game.archive.id;
        self.browse_play.visible_now.push(id);
        ui.push_id(("v2_browse_play_game", id), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(card_width, CARD_HEIGHT), egui::Sense::click());
            if self.browse_play.focus_game == Some(id) {
                self.browse_play.focus_game = None;
                response.request_focus();
            }
            if response.has_focus() {
                self.browse_play.focused_card = Some((response.id, id));
            }
            Self::reveal_keyboard_focus(ui, &response);
            // Name the card for assistive tech and the controller layer.
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, game.title.clone())
            });
            if !ui.is_rect_visible(rect) {
                return;
            }
            let in_set = self.library_selection.contains(id);
            Self::card_background(ui, rect, selected, in_set, &response);
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
            // Reserve two rows even for a short title: metadata aligns across
            // the shelf while covers and placeholders keep identical bounds.
            child.allocate_ui_with_layout(
                egui::vec2(inner.width(), 42.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_height(42.0);
                    ui.add(
                        egui::Label::new(two_line_title(&game.title, inner.width()))
                            .wrap()
                            .show_tooltip_when_elided(false)
                            .selectable(false),
                    );
                },
            );
            let blue = theme::blue_profile(&child);
            child.add(
                egui::Label::new(status_line(
                    game,
                    inner.width(),
                    blue,
                    self.browse_play.readiness.not_ready_reason(game),
                ))
                .wrap()
                .selectable(false),
            );
            Self::paint_selection_mark(ui, rect, in_set);
            let response = response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("{}\n{}", game.title, game.platform));
            if self.batch_selecting {
                let mark = if self.batch_selection.contains(id) {
                    "☑"
                } else {
                    "☐"
                };
                child.label(format!("{mark} Select"));
                if response.clicked() {
                    self.batch_message = self
                        .batch_selection
                        .toggle(id)
                        .err()
                        .map(|limit| format!("Select no more than {limit} games."));
                }
            } else if response.clicked() {
                self.browse_play_row_clicked(ui, id, list);
            }
        });
    }

    /// A card or list row was clicked (or activated from the keyboard). Ctrl
    /// and Shift change the multi-select set; a plain click opens the game
    /// exactly as before and empties the set.
    fn browse_play_row_clicked(&mut self, ui: &egui::Ui, id: i64, list: &[usize]) {
        let modifiers = ui.input(|input| input.modifiers);
        if !self.library_selection_click(id, modifiers, list) {
            self.go(Route::BrowsePlayGame(id));
        }
    }

    fn browse_play_list_row(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        selected: bool,
        list: &[usize],
        note: &mut Option<String>,
    ) {
        let id = game.archive.id;
        self.browse_play.visible_now.push(id);
        ui.push_id(("v2_browse_play_game", id), |ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), LIST_ROW_HEIGHT - 8.0),
                egui::Sense::click(),
            );
            if self.browse_play.focus_game == Some(id) {
                self.browse_play.focus_game = None;
                response.request_focus();
            }
            if response.has_focus() {
                self.browse_play.focused_card = Some((response.id, id));
            }
            Self::reveal_keyboard_focus(ui, &response);
            // Name the card for assistive tech and the controller layer.
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, game.title.clone())
            });
            if !ui.is_rect_visible(rect) {
                return;
            }
            let in_set = self.library_selection.contains(id);
            Self::card_background(ui, rect, selected, in_set, &response);
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
                let blue = theme::blue_profile(ui);
                ui.add(
                    egui::Label::new(status_line(
                        game,
                        text_width,
                        blue,
                        self.browse_play.readiness.not_ready_reason(game),
                    ))
                    .wrap()
                    .selectable(false),
                );
            });
            Self::paint_selection_mark(ui, rect, in_set);
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            if self.batch_selecting {
                let mark = if self.batch_selection.contains(id) {
                    "☑"
                } else {
                    "☐"
                };
                child.label(mark);
                if response.clicked() {
                    self.batch_message = self
                        .batch_selection
                        .toggle(id)
                        .err()
                        .map(|limit| format!("Select no more than {limit} games."));
                }
            } else if response.clicked() {
                self.browse_play_row_clicked(ui, id, list);
            }
        });
    }

    /// The selected game. Full panel beside the shelf, or a compact strip when
    /// the window is narrow. Play is always the first thing to see.
    fn browse_play_selected(&mut self, ui: &mut egui::Ui, game: &Game, compact: bool) {
        let id = game.archive.id;
        egui::Frame::new()
            .fill(theme::card_fill(ui))
            .stroke(theme::border(ui))
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
                    // A short panel keeps the small cover and puts Play beside
                    // it, so Play is on screen without scrolling.
                    let tight = compact || ui.available_height() < TIGHT_PANEL_HEIGHT;
                    let cover = if tight || inner < 420.0 {
                        COMPACT_COVER_SIZE
                    } else {
                        SELECTED_COVER_SIZE
                    };
                    let text_width = (inner - cover.x - gap).max(120.0);
                    let heading = |this: &mut Self, ui: &mut egui::Ui| {
                        let not_ready = this.browse_play.readiness.not_ready_reason(game);
                        // In a short, narrow window title and actions share a strip.
                        // Stacking every piece would put Play below the footer.
                        if compact && theme::blue_profile(ui) && inner >= 600.0 {
                            let remaining = inner - cover.x - gap * 2.0;
                            let title_width = remaining * 0.38;
                            let action_width = remaining - title_width;
                            ui.horizontal_top(|ui| {
                                this.picture_tile(ui, game, Kind::Cover, cover);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(title_width, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        super::title_fit::title_label(
                                            ui,
                                            &game.title,
                                            theme::TextRole::CardTitle,
                                            2,
                                        );
                                        ui.add(
                                            egui::Label::new(status_line(
                                                game,
                                                title_width,
                                                theme::blue_profile(ui),
                                                not_ready,
                                            ))
                                            .wrap()
                                            .selectable(false),
                                        );
                                    },
                                );
                                ui.allocate_ui_with_layout(
                                    egui::vec2(action_width, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| Self::play_row(this, ui, game, action_width, true),
                                );
                            });
                            return;
                        }
                        ui.horizontal_top(|ui| {
                            this.picture_tile(ui, game, Kind::Cover, cover);
                            ui.vertical(|ui| {
                                ui.set_width(text_width);
                                super::title_fit::title_label(
                                    ui,
                                    &game.title,
                                    theme::TextRole::SectionTitle,
                                    2,
                                );
                                let blue = theme::blue_profile(ui);
                                if tight {
                                    // One line, so Play stays above the fold.
                                    ui.add(
                                        egui::Label::new(status_line(
                                            game, text_width, blue, not_ready,
                                        ))
                                        .wrap()
                                        .selectable(false),
                                    );
                                } else {
                                    ui.label(
                                        RichText::new(&game.platform).color(theme::SECONDARY_TEXT),
                                    );
                                    let (status, colour) = status_text(game, blue);
                                    match not_ready {
                                        Some(reason) => ui.label(
                                            RichText::new(format!("Not ready · {reason}"))
                                                .strong()
                                                .color(if blue {
                                                    theme::blue::WARN
                                                } else {
                                                    ATTENTION_TEXT
                                                }),
                                        ),
                                        None => {
                                            ui.label(RichText::new(status).strong().color(colour))
                                        }
                                    };
                                    ui.label(format!("DAT: {}", game.dat_identity_summary()));
                                }
                                if tight {
                                    // The detail scroll bar takes 12 points.
                                    let scrollbar = if compact { 0.0 } else { 12.0 };
                                    Self::play_row(this, ui, game, text_width - scrollbar, tight);
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
                            if !tight {
                                Self::play_row(self, ui, game, inner - 12.0, tight);
                            }
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

    /// The game's real launch readiness with its one primary button (Play
    /// when the game is ready or not yet checked, otherwise the step that
    /// fixes the first problem), then Game details. Preservation-only media
    /// gets no readiness check and no Play, only Game details.
    fn play_row(this: &mut Self, ui: &mut egui::Ui, game: &Game, width: f32, tight: bool) {
        use super::launch_readiness_summary::{self as readiness, CompactChoice};
        let id = game.archive.id;
        let details = if game.is_preservation_only() {
            ui.add(
                egui::Label::new(
                    RichText::new(PRESERVATION_ONLY_SENTENCE)
                        .size(theme::METADATA_SIZE)
                        .color(theme::SECONDARY_TEXT),
                )
                .wrap(),
            );
            ui.add_sized([width, 34.0], egui::Button::new("Game details"))
                .clicked()
        } else {
            let now = ui.input(|input| input.time);
            let remaining = this.browse_play.settle_remaining(id, now);
            let workflows = this.native_workflows.get_or_insert_with(|| {
                super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
            });
            let path = &game.archive.absolute_path;
            // Evidence already gathered (or being gathered) for this game is
            // reused at once; otherwise the check waits for the selection to
            // settle, and until then the card says Checking.
            let view = if remaining > 0.0 && !workflows.has_evidence_for(path) {
                workflows.release_other_selection(path);
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(remaining));
                readiness::compact(&readiness::not_started_summary(), false)
            } else {
                workflows.note_catalogue_row(&game.archive);
                workflows.compact_readiness(
                    ui.ctx(),
                    id,
                    path,
                    game.archive.identity_report.as_ref(),
                    game.dat_exact.is_some(),
                    &mut this.activity,
                )
            };
            {
                // Only a finished, current answer is kept; checking, stale
                // and partial answers leave the game Not checked.
                let result = CheckedReadiness::from_compact(&view);
                this.browse_play.readiness.record(game, result);
            }
            let rows = if tight { 2 } else { 3 };
            match readiness::show_compact(ui, &view, width, rows, Some("Game details")) {
                Some(CompactChoice::Trailing) => true,
                Some(CompactChoice::Action(action)) => {
                    if let Some(route) = workflows.readiness_action_route(ui.ctx(), action, id) {
                        this.go(route);
                    }
                    false
                }
                None => false,
            }
        };
        if details {
            this.go(Route::Game(id));
        }
        this.surprise_retry_offer(ui, game);
        if !game.is_preservation_only() && !tight && theme::blue_profile(ui) {
            ui.add(
                egui::Label::new(
                    RichText::new(SELECT_VS_PLAY)
                        .size(13.0)
                        .color(theme::blue::SECONDARY_TEXT),
                )
                .wrap(),
            );
            ui.small(readiness_sentence(game));
        }
    }

    fn selected_game_prompt(ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::card_fill(ui))
            .stroke(theme::border(ui))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::section_title("Pick a game"));
                ui.label(
                    RichText::new("Choose a game on the left to see its cover and whether it is ready to play.")
                        .color(theme::SECONDARY_TEXT),
                );
            });
    }

    fn browse_play_empty(&mut self, ui: &mut egui::Ui, library_empty: bool) {
        egui::Frame::new()
            .fill(theme::card_fill(ui))
            .stroke(theme::border(ui))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if library_empty && !self.loaded {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(theme::section_title("Loading your game list…"));
                    });
                    ui.label("This can take a moment for a large library. Nothing is being changed.");
                } else if library_empty {
                    ui.label(theme::section_title("No games have been added yet"));
                    ui.label("Add a game folder in Sources & Providers, then scan it into the existing library.");
                    if browse_primary(ui, "Open Sources") {
                        self.go(Route::Section(Section::Sources));
                    }
                } else {
                    ui.label(theme::section_title("No games match"));
                    ui.label("Try a different search or system, or clear what you have chosen.");
                    if ui.button("Clear search and system").clicked() {
                        self.filter = Filter {
                            list: self.filter.list,
                            sort: self.filter.sort,
                            ..Default::default()
                        };
                    }
                }
            });
    }
}

/// A toolbar on/off control: a real button, so the unselected state matches
/// Select games beside it instead of reading as plain text.
fn readiness_filter_hint(filter: ReadinessFilter) -> &'static str {
    match filter {
        ReadinessFilter::All => "Show every game.",
        ReadinessFilter::Ready => {
            "Games whose launch check has already passed. Select a game to check it."
        }
        ReadinessFilter::NeedsAttention => "Show only games that need attention.",
        ReadinessFilter::NotChecked => "Games with no passed launch check yet.",
    }
}

/// How many records of removed library locations are not listed, and why.
/// The records are kept; this only says they exist.
pub(super) fn retired_note(library: &super::library::Library) -> Option<(String, String)> {
    use archivefs_core::catalogue_supersession::Retirement;
    if library.retired.is_empty() {
        return None;
    }
    let count = |reason| {
        library
            .retired_reasons
            .values()
            .filter(|r| **r == reason)
            .count()
    };
    Some((
        format!(
            "{} older records not listed: their library location was removed and the files are no longer there.",
            thousands(library.retired.len())
        ),
        format!(
            "Kept in the database, not deleted. {} have a file with the same name in your library that could not be confirmed as the same game; {} were not found in your library.",
            thousands(count(Retirement::UnresolvedCopies)),
            thousands(count(Retirement::NotFoundAnywhere))
        ),
    ))
}

/// Said under every readiness view that could be mistaken for a verdict on
/// the whole library. Launch checks run for the game you select, so nothing
/// here has assessed every game.
/// The sort control's closed text when the row has no room for the order.
const SORT_SHORT_LABEL: &str = "Sort";
/// Side padding of the short sort box.
const SORT_SHORT_PAD: f32 = 6.0;

/// The sort control's closed text: says what it is, not just the order.
fn sort_button_text(sort: SortOrder) -> String {
    format!("Sort: {}", sort.label())
}

/// Plain note for games placed last because they lack the sorted value.
pub(super) fn sort_missing_note(missing: usize, shown: usize, noun: &str) -> Option<String> {
    if missing == 0 || shown == 0 {
        return None;
    }
    if missing == shown {
        return Some(format!(
            "None of these {shown} games has a {} yet, so they stay in title order.",
            noun.trim_start_matches("no ")
        ));
    }
    Some(if missing == 1 {
        format!("1 of {shown} games has {noun}; it is listed after the rest, by title.")
    } else {
        format!("{missing} of {shown} games have {noun}; they are listed after the rest, by title.")
    })
}

fn readiness_filter_note(
    filter: ReadinessFilter,
    checked: usize,
    total: usize,
    checking: bool,
) -> Option<String> {
    let progress = format!(
        "{checked} of {total} games checked so far. A game is checked when you select it, and most systems are also checked in the background as they scroll into view."
    );
    let progress = if checking {
        format!("{progress} Checking more now…")
    } else {
        progress
    };
    match filter {
        ReadinessFilter::All => None,
        ReadinessFilter::Ready => Some(format!(
            "{progress} Only checked games can appear here; the rest are Not checked."
        )),
        ReadinessFilter::NeedsAttention => Some(format!(
            "{progress} This lists games flagged by the catalogue and checked games that are Not ready, with the reason. Games that are Not checked are not listed."
        )),
        ReadinessFilter::NotChecked => Some(format!(
            "{progress} Not checked only means no result yet, not a problem."
        )),
    }
}

fn toggle_button(text: &str, selected: bool) -> egui::Button<'_> {
    egui::Button::new(text).selected(selected)
}

fn view_toggle(ui: &mut egui::Ui, current: &mut bool, value: bool, text: &str) {
    if ui.add(toggle_button(text, *current == value)).clicked() {
        *current = value;
    }
}

#[cfg(test)]
mod classic_identity_status_tests {
    use super::{readiness_sentence, status_text};
    use crate::gui_v2::library::Game;
    use archivefs_core::PersistedArchive;
    use archivefs_core::game_identity::*;

    #[test]
    fn conflicting_game_identity_is_not_hidden_by_recognised_platform() {
        let path = std::path::PathBuf::from("/fixture/conflict.zip");
        let mut archive = PersistedArchive {
            id: 1,
            source_folder_id: 1,
            relative_path: "conflict.zip".into(),
            absolute_path: path.clone(),
            archive_kind: "zip".into(),
            display_name: "Conflict".into(),
            normalized_name: "conflict".into(),
            size_bytes: Some(1),
            modified_time_unix_seconds: Some(1),
            platform: Some("PSX".into()),
            platform_source: Some("folder_alias".into()),
            last_known_health: "pending".into(),
            last_seen_at: "fixture".into(),
            last_verified_missing_at: None,
            identity_report: None,
        };
        archive.identity_report = Some(GameIdentityReport {
            archive_path: path.clone(),
            platform: IdentityPlatform::PlayStation,
            format: IdentityImageFormat::Iso,
            evidence: ["SCUS-94163", "SLUS-00001"]
                .into_iter()
                .map(|value| IdentityEvidence {
                    kind: IdentityKind::Ps1Serial,
                    status: IdentityStatus::Verified,
                    value: Some(value.into()),
                    confidence: IdentityConfidence::StructuredMetadata,
                    provenance: IdentityProvenance {
                        archive_path: path.clone(),
                        member_path: None,
                        member_index: None,
                        method: "fixture".into(),
                    },
                    diagnostic: "fixture".into(),
                })
                .collect(),
            warnings: vec![],
            bytes_read: 1,
            archive_members_inspected: 0,
            metadata_paths_inspected: 1,
            nested_container_depth: 0,
            complete: true,
        });
        let mut game = Game::from_archive(archive);
        game.dat_state = Some(archivefs_core::LibraryDatState::NoMatch);
        assert_eq!(game.identity_summary(), "Mismatch");
        assert_eq!(game.status(), "Identity conflict · review evidence");
        assert_eq!(status_text(&game, true).0, "Mismatch");
        assert!(readiness_sentence(&game).contains("conflicts"));
        let mut exact = game.clone();
        exact.dat_state = None;
        exact.mark_dat_exact(Some("DAT"));
        assert_eq!(exact.identity_summary(), "Mismatch");
        assert_eq!(exact.status(), "Identity conflict · review evidence");
    }
}

#[cfg(test)]
mod play_wording_tests {
    use super::SELECT_VS_PLAY;
    use crate::gui_v2::launch_readiness_summary::PLAY_RUNS_THE_CHECK;

    /// Play launches directly now; no hint may still promise a detour through
    /// a readiness page or a confirmation step.
    #[test]
    fn play_hints_do_not_describe_the_old_detour() {
        for text in [
            SELECT_VS_PLAY,
            PLAY_RUNS_THE_CHECK,
            crate::gui_v2::surprise_me::PLAY_NOTE,
        ] {
            let lower = text.to_lowercase();
            assert!(!lower.contains("opens the"), "{text}");
            assert!(!lower.contains("until you confirm"), "{text}");
            assert!(!lower.contains("readiness check first"), "{text}");
        }
    }
}
