//! Surprise Me: a playful random pick from the Library.
//!
//! Reads only the loaded, read-only catalogue snapshot and the finished
//! readiness answers Browse & Play already holds. It never scans the library,
//! changes the catalogue or starts anything: Play selects the pick in Browse &
//! Play, where the usual launch-readiness check and Play button take over.
//!
//! The cover uses the shared `picture_tile` pipeline, exactly as Browse & Play
//! does, so showing it can write to the thumbnail cache: a resized copy of
//! local artwork, and, only with the optional Online artwork setting on, a
//! RomM cover fetched because it was not cached yet. The pipeline's routine
//! thumbnail-cache trim may also run.

use super::{
    App,
    library::{CheckedReadiness, Game, Library, ReadinessCache, UNKNOWN_PLATFORM},
    media_sources::Kind,
    routes::Route,
};
use crate::ui::theme;
use eframe::egui::{self, RichText};

const COVER_SIZE: egui::Vec2 = egui::vec2(150.0, 196.0);
const ALL_SYSTEMS: &str = "All systems";

/// Opening lines, picked with the same seeded generator as the game.
const TAGLINES: [&str; 6] = [
    "How about this one?",
    "The dice have spoken.",
    "Tonight's pick:",
    "Fancy a change?",
    "Fortune favours the bold:",
    "Why not give this a go?",
];

pub(super) const PLAY_NOTE: &str =
    "Play opens the usual readiness check. Nothing starts until you confirm.";
pub(super) const READY_TO_TRY_LABEL: &str = "Only potentially launchable games";
const READY_TO_TRY_HINT: &str = "Leaves out games whose files are recorded as missing or \
     damaged, recordings, games with no known system, and games whose last check said \
     Not ready.";
/// Shown under the filter while it is on: picks are judged from saved
/// records only, never by touching the disk or a mount.
pub(super) const LAST_RECORDED_NOTE: &str = "Availability comes from the last recorded \
     scan and readiness checks, so a pick may still turn out to be unavailable. Play \
     checks again before anything starts.";
/// Shown with the readiness card when the game Surprise Me handed over failed
/// its check, so the person can try another without leaving the Library.
pub(super) const RETRY_NOTE: &str =
    "This surprise pick is not ready. The card above says why. Want another?";
pub(super) const EMPTY_LIBRARY: &str =
    "Your library is empty. Add a game folder and scan it, then roll again.";
pub(super) const NO_MATCH: &str = "No games match. Try another system, or turn off \
     \"Only potentially launchable games\".";

/// A small seeded generator (SplitMix64), so a seed always gives the same
/// picks. Not for anything security-related.
#[derive(Clone, Debug)]
pub(super) struct SurpriseRng(u64);

impl SurpriseRng {
    #[cfg(test)]
    pub fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        Self(nanos ^ u64::from(std::process::id()).rotate_left(32))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`; `n` must be above zero.
    pub fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }
}

/// What the picker may choose from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SurpriseFilter {
    /// `None`: every system.
    pub platform: Option<String>,
    pub ready_to_try: bool,
}

impl Default for SurpriseFilter {
    fn default() -> Self {
        Self {
            platform: None,
            ready_to_try: true,
        }
    }
}

/// A game worth offering when "ready to try" is on: the catalogue does not
/// record its file as missing or damaged, it is not a preservation-only
/// recording, its system is known, and no finished check said Not ready.
/// The readiness check at Play still decides whether it can start.
pub(super) fn ready_to_try(game: &Game, cache: &ReadinessCache) -> bool {
    !game.attention
        && game.archive.last_verified_missing_at.is_none()
        && !game.is_preservation_only()
        && game.platform != UNKNOWN_PLATFORM
        && !matches!(cache.get(game), Some(CheckedReadiness::NotReady(_)))
}

/// Library indices the filter allows, in catalogue order.
pub(super) fn candidates(
    library: &Library,
    filter: &SurpriseFilter,
    cache: &ReadinessCache,
) -> Vec<usize> {
    let allowed = |index: &usize| {
        let game = &library.games[*index];
        filter
            .platform
            .as_deref()
            .is_none_or(|platform| game.platform == platform)
            && (!filter.ready_to_try || ready_to_try(game, cache))
    };
    match filter.platform.as_deref() {
        Some(platform) => library
            .by_platform
            .get(platform)
            .map(|indices| indices.iter().copied().filter(allowed).collect())
            .unwrap_or_default(),
        None => (0..library.games.len()).filter(allowed).collect(),
    }
}

/// One uniform pick from `candidates`, never `avoid` while another choice
/// exists, so Roll Again always changes the game when it can.
pub(super) fn pick(
    library: &Library,
    candidates: &[usize],
    rng: &mut SurpriseRng,
    avoid: Option<i64>,
) -> Option<i64> {
    let id = |index: usize| library.games[index].archive.id;
    let others: Vec<usize> = match avoid {
        Some(avoid) if candidates.len() > 1 => candidates
            .iter()
            .copied()
            .filter(|index| id(*index) != avoid)
            .collect(),
        _ => candidates.to_vec(),
    };
    if others.is_empty() {
        return None;
    }
    Some(id(others[rng.below(others.len())]))
}

#[derive(Default)]
pub(super) struct SurpriseState {
    pub open: bool,
    pub filter: SurpriseFilter,
    /// Seeded from the clock on first use unless a test seeded it.
    rng: Option<SurpriseRng>,
    pub current: Option<i64>,
    tagline: usize,
    /// Set after a roll found nothing, so the empty state is shown instead of
    /// rolling again on every frame.
    empty: bool,
    /// The game Play handed to Browse & Play, until another roll.
    handoff: Option<i64>,
}

/// What a click in the window asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SurpriseAction {
    Roll,
    Play(i64),
}

impl SurpriseState {
    #[cfg(test)]
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: Some(SurpriseRng::seeded(seed)),
            ..Self::default()
        }
    }

    pub fn tagline(&self) -> &'static str {
        TAGLINES[self.tagline % TAGLINES.len()]
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.empty
    }

    /// The failed check's label when `game` is the one Play handed over and
    /// its finished readiness check said Not ready (a missing file, an
    /// unavailable mount, a missing emulator, ...). The readiness card shows
    /// the full reason; this only decides whether to offer Roll Again.
    pub fn retry_offer(&self, game: &Game, cache: &ReadinessCache) -> Option<&'static str> {
        if self.handoff != Some(game.archive.id) {
            return None;
        }
        match cache.get(game) {
            Some(CheckedReadiness::NotReady(reason)) => Some(reason),
            _ => None,
        }
    }

    /// Pick a new game for the current filter, avoiding the one on show.
    pub fn roll(&mut self, library: &Library, cache: &ReadinessCache) -> Option<i64> {
        self.handoff = None;
        let rng = self.rng.get_or_insert_with(SurpriseRng::from_clock);
        let pool = candidates(library, &self.filter, cache);
        self.current = pick(library, &pool, rng, self.current);
        self.tagline = rng.below(TAGLINES.len());
        self.empty = self.current.is_none();
        self.current
    }

    /// The game on show, while it is still in the catalogue and still passes
    /// the filter. A reload or a finished Not ready check can remove it.
    pub fn current_game<'a>(
        &self,
        library: &'a Library,
        cache: &ReadinessCache,
    ) -> Option<&'a Game> {
        let game = library.game(self.current?)?;
        let still_allowed = self
            .filter
            .platform
            .as_deref()
            .is_none_or(|platform| game.platform == platform)
            && (!self.filter.ready_to_try || ready_to_try(game, cache));
        still_allowed.then_some(game)
    }

    /// Keep a valid pick on show: roll when there is none and the last roll
    /// did not already come up empty.
    pub fn refresh(&mut self, library: &Library, cache: &ReadinessCache) {
        if self.current_game(library, cache).is_none() && !self.empty {
            self.current = None;
            self.roll(library, cache);
        }
    }

    /// The filter changed: the old pick may not fit, and an earlier empty
    /// result no longer applies.
    pub fn filter_changed(&mut self, library: &Library, cache: &ReadinessCache) {
        self.empty = false;
        if self.current_game(library, cache).is_none() {
            self.current = None;
        }
        self.roll(library, cache);
    }
}

impl App {
    pub(super) fn open_surprise_me(&mut self) {
        let surprise = &mut self.browse_play.surprise;
        surprise.open = true;
        surprise.empty = false;
        surprise.roll(&self.library, &self.browse_play.readiness);
    }

    /// Play hands the pick to Browse & Play's own selected-game panel, where
    /// the existing readiness check and Play run. Nothing starts from here.
    pub(super) fn apply_surprise_action(&mut self, action: SurpriseAction) {
        match action {
            SurpriseAction::Roll => {
                self.browse_play
                    .surprise
                    .roll(&self.library, &self.browse_play.readiness);
            }
            SurpriseAction::Play(id) => {
                if self
                    .browse_play
                    .surprise
                    .current_game(&self.library, &self.browse_play.readiness)
                    .is_some_and(|game| game.archive.id == id)
                {
                    self.browse_play.surprise.open = false;
                    self.browse_play.surprise.handoff = Some(id);
                    self.go(Route::BrowsePlayGame(id));
                }
            }
        }
    }

    /// Under the selected game's readiness card: when the game Surprise Me
    /// handed over failed its check, offer another pick. The failed game is
    /// left out of the next roll by its finished Not ready answer.
    pub(super) fn surprise_retry_offer(&mut self, ui: &mut egui::Ui, game: &Game) {
        if self
            .browse_play
            .surprise
            .retry_offer(game, &self.browse_play.readiness)
            .is_none()
        {
            return;
        }
        ui.add(egui::Label::new(RichText::new(RETRY_NOTE).color(theme::SECONDARY_TEXT)).wrap());
        if ui.button("Roll again").clicked() {
            self.open_surprise_me();
        }
    }

    pub(super) fn surprise_me_window(&mut self, context: &egui::Context) {
        if !self.browse_play.surprise.open {
            return;
        }
        let library = self.library.clone();
        self.browse_play
            .surprise
            .refresh(&library, &self.browse_play.readiness);
        let mut open = true;
        let mut action = None;
        egui::Window::new("Surprise me")
            .id(egui::Id::new("v2_surprise_me"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .show(context, |ui| {
                action = self.surprise_me_body(ui, &library);
            });
        if let Some(action) = action {
            self.apply_surprise_action(action);
        }
        if !open {
            self.browse_play.surprise.open = false;
        }
    }

    fn surprise_me_body(&mut self, ui: &mut egui::Ui, library: &Library) -> Option<SurpriseAction> {
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            let filter = &mut self.browse_play.surprise.filter;
            let selected = filter
                .platform
                .as_deref()
                .unwrap_or(ALL_SYSTEMS)
                .to_string();
            egui::ComboBox::from_id_salt("v2_surprise_me_platform")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut filter.platform, None, ALL_SYSTEMS)
                        .changed();
                    for (platform, count) in &library.platforms {
                        changed |= ui
                            .selectable_value(
                                &mut filter.platform,
                                Some(platform.clone()),
                                format!("{platform} ({count})"),
                            )
                            .changed();
                    }
                });
            changed |= ui
                .checkbox(&mut filter.ready_to_try, READY_TO_TRY_LABEL)
                .on_hover_text(READY_TO_TRY_HINT)
                .changed();
        });
        if self.browse_play.surprise.filter.ready_to_try {
            ui.add(
                egui::Label::new(
                    RichText::new(LAST_RECORDED_NOTE)
                        .size(theme::METADATA_SIZE)
                        .color(theme::SECONDARY_TEXT),
                )
                .wrap(),
            );
        }
        if changed {
            self.browse_play
                .surprise
                .filter_changed(library, &self.browse_play.readiness);
        }
        ui.add_space(theme::SPACE_SM);

        let game = self
            .browse_play
            .surprise
            .current_game(library, &self.browse_play.readiness)
            .cloned();
        let Some(game) = game else {
            let message = if library.games.is_empty() {
                EMPTY_LIBRARY
            } else {
                NO_MATCH
            };
            ui.add(egui::Label::new(RichText::new(message).color(theme::SECONDARY_TEXT)).wrap());
            ui.add_space(theme::SPACE_SM);
            let can_roll = !library.games.is_empty();
            return ui
                .add_enabled(can_roll, egui::Button::new("Roll again"))
                .clicked()
                .then_some(SurpriseAction::Roll);
        };

        let mut action = None;
        ui.horizontal_top(|ui| {
            self.picture_tile(ui, &game, Kind::Cover, COVER_SIZE);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(self.browse_play.surprise.tagline())
                        .italics()
                        .color(theme::SECONDARY_TEXT),
                );
                super::title_fit::title_label(ui, &game.title, theme::TextRole::SectionTitle, 2);
                ui.label(RichText::new(&game.platform).color(theme::SECONDARY_TEXT));
                ui.small(game.status());
                ui.add_space(theme::SPACE_SM);
                ui.horizontal_wrapped(|ui| {
                    let play = egui::Button::new(RichText::new("Play").strong())
                        .fill(theme::primary_action_fill(ui))
                        .min_size(egui::vec2(96.0, 34.0));
                    if ui.add(play).on_hover_text(PLAY_NOTE).clicked() {
                        action = Some(SurpriseAction::Play(game.archive.id));
                    }
                    if ui
                        .add(egui::Button::new("Roll again").min_size(egui::vec2(96.0, 34.0)))
                        .clicked()
                    {
                        action = Some(SurpriseAction::Roll);
                    }
                });
                ui.add(
                    egui::Label::new(
                        RichText::new(PLAY_NOTE)
                            .size(theme::METADATA_SIZE)
                            .color(theme::SECONDARY_TEXT),
                    )
                    .wrap(),
                );
            });
        });
        action
    }
}

#[cfg(test)]
#[path = "tests/surprise_me.rs"]
mod tests;
