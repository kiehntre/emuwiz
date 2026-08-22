//! Gamer View's platform shelf: the horizontally-scrolling row of platform
//! cards above the game list. Extracted verbatim from `main.rs`
//! (2026-08-22, GUI extraction pass 2); kept separate from `gamer_view.rs`
//! per that module's own size, matching the "split by coherent
//! responsibility" rule rather than growing one already-large file further.
//!
//! Two RomM dialog-sizing helpers (`romm_window_body_cap`/
//! `romm_dialog_sizes`) were physically interleaved with this code in the
//! old `main.rs` but are unrelated (used only by RomM browse/configure
//! windows far away) - they stayed in `main.rs`. Their doc comment had
//! also become detached from a comment actually describing
//! `platform_shelf_state_id` (visible below); the misplaced comment was
//! left exactly where it was for the RomM functions, and only the text
//! that actually describes `platform_shelf_state_id` moved with it - a
//! pure comment-attribution fix with no code or behaviour change.

use super::*;

pub(crate) const PLATFORM_CARD_MIN_WIDTH: f32 = 132.0;
pub(crate) const PLATFORM_CARD_MAX_WIDTH: f32 = 164.0;

/// Cards grow modestly with desktop width, but never shrink below a
/// readable minimum or consume so much room that horizontal browsing
/// becomes awkward. The shelf itself remains a single scrolling row.
pub(crate) fn gamer_platform_card_width(viewport_width: f32) -> f32 {
    (viewport_width * 0.14).clamp(PLATFORM_CARD_MIN_WIDTH, PLATFORM_CARD_MAX_WIDTH)
}

// --- Platform shelf horizontal navigation ---------------------------------

/// The exact height of one platform card. Shared by the cards and the chevrons
/// so a control can never be taller than the strip it sits beside.
pub(crate) const PLATFORM_CARD_HEIGHT: f32 = 142.0;

/// The shelf's total height: one card plus room for the horizontal scrollbar
/// beneath it.
///
/// A single fixed shelf height is what makes the space below it predictable -
/// "the platform picker must not consume most of the vertical height" is true
/// unconditionally, not just for a typical library - and it is the boundary the
/// game list and details pane are positioned against. Kept at module scope, next
/// to the card height it is derived from, so the two cannot drift apart.
pub(crate) const PLATFORM_SHELF_HEIGHT: f32 = 150.0;

/// The chevron glyphs. Plain ASCII deliberately: egui's bundled fonts do not
/// carry the geometric-shape triangles (U+25C0/U+25B6), so those render as
/// nothing at all. The accessible name on each button carries the real meaning
/// either way - see `shelf_chevron`.
pub(crate) const SHELF_PREVIOUS_GLYPH: &str = "<";
pub(crate) const SHELF_NEXT_GLYPH: &str = ">";

/// Width reserved for one chevron button. Generous on purpose: this is the
/// control a TV/Moonlight user aims a pointer at from across a room, and the
/// one a D-pad lands focus on.
pub(crate) const SHELF_CHEVRON_WIDTH: f32 = 44.0;

/// How many whole cards a strip of `usable` width should show.
///
/// `preferred` is the width `gamer_platform_card_width` would like to use; the
/// answer is the count whose exactly-fitting width is no wider than that, and
/// no narrower than `PLATFORM_CARD_MIN_WIDTH`.
pub(crate) fn shelf_visible_card_count(usable: f32, preferred: f32, spacing: f32) -> usize {
    let stride = preferred + spacing;
    if usable <= 0.0 || stride <= 0.0 {
        return 1;
    }
    let fitted_at = |count: f32| (usable + spacing) / count - spacing;
    // The count that fits at the preferred width, then one more when that
    // count would have to stretch the cards past `preferred` to fill the strip.
    let mut count = ((usable + spacing) / stride).floor().max(1.0);
    if fitted_at(count) > preferred {
        count += 1.0;
    }
    // Never so many that a card falls below the readable minimum.
    while count > 1.0 && fitted_at(count) < PLATFORM_CARD_MIN_WIDTH {
        count -= 1.0;
    }
    count as usize
}

/// The card width that makes a whole number of cards fill `usable` exactly.
///
/// Clamped to the readable range. When the clamp binds - a strip too narrow for
/// even one minimum-width card - the cards no longer fill the strip exactly and
/// a little unused space is left before the trailing chevron. That is the safe
/// direction to miss in: unused space never puts a card under a control.
pub(crate) fn shelf_fitted_card_width(usable: f32, preferred: f32, spacing: f32) -> f32 {
    let count = shelf_visible_card_count(usable, preferred, spacing) as f32;
    let fitted = (usable + spacing) / count - spacing;
    fitted.clamp(
        PLATFORM_CARD_MIN_WIDTH,
        preferred.max(PLATFORM_CARD_MIN_WIDTH),
    )
}

/// The width the scrolling strip is given: exactly the cards it shows, and
/// never more than the space left between the chevrons.
pub(crate) fn shelf_strip_width(usable: f32, card_width: f32, spacing: f32) -> f32 {
    let count = shelf_visible_card_count(usable, card_width, spacing) as f32;
    (count * (card_width + spacing) - spacing)
        .min(usable)
        .max(0.0)
}

/// The space one chevron takes out of the row: the button plus the gap that
/// separates it from the strip.
pub(crate) fn shelf_chevron_reserve(spacing: f32) -> f32 {
    SHELF_CHEVRON_WIDTH + spacing
}

/// How much of the visible strip one chevron press moves, as a fraction of the
/// viewport. Deliberately short of a full page so a card or two stays on screen
/// as a visual anchor - a person should be able to see that they moved along a
/// continuous shelf rather than jumped to an unrelated place.
pub(crate) const SHELF_PAGE_FRACTION: f32 = 0.75;

/// Sub-pixel tolerance for "is this edge reached". Scroll offsets are floats
/// and land fractionally short of the end after an animation, so comparing
/// exactly would leave the next-chevron enabled with nothing left to reveal.
pub(crate) const SHELF_EDGE_EPSILON: f32 = 1.0;

/// What a navigation control asks the shelf to do. Deliberately an intent
/// rather than a pixel delta, so the same value serves a click, an arrow key
/// and a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShelfScroll {
    PageLeft,
    PageRight,
    Start,
    End,
}

/// The shelf's measured geometry, as the scroll area reported it last frame.
///
/// Every navigation decision is derived from these three numbers, which is what
/// makes the whole behaviour testable without a window: the button states, the
/// distance a press travels, and whether controls are needed at all.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct PlatformShelfMetrics {
    /// Current horizontal scroll offset.
    pub(crate) offset_x: f32,
    /// Total width of all cards laid out in a row.
    pub(crate) content_width: f32,
    /// Width of the visible strip.
    pub(crate) viewport_width: f32,
    /// One card plus the gap that follows it. Paging moves whole multiples of
    /// this so an aligned shelf stays aligned. Zero means "not measured yet",
    /// which falls back to the raw fractional page.
    pub(crate) card_stride: f32,
}

impl PlatformShelfMetrics {
    /// The largest offset that still shows content: zero when everything fits.
    pub(crate) fn max_offset(&self) -> f32 {
        (self.content_width - self.viewport_width).max(0.0)
    }

    /// Whether any card is off-screen, and therefore whether the controls have
    /// anything to do. Zero-width measurements (the first frame, or an empty
    /// library) count as fitting.
    pub(crate) fn overflows(&self) -> bool {
        self.viewport_width > 0.0 && self.max_offset() > SHELF_EDGE_EPSILON
    }

    pub(crate) fn at_start(&self) -> bool {
        self.offset_x <= SHELF_EDGE_EPSILON
    }

    pub(crate) fn at_end(&self) -> bool {
        self.offset_x >= self.max_offset() - SHELF_EDGE_EPSILON
    }

    /// Whether the "Previous platforms" control can do anything.
    pub(crate) fn can_page_left(&self) -> bool {
        self.overflows() && !self.at_start()
    }

    /// Whether the "Next platforms" control can do anything.
    pub(crate) fn can_page_right(&self) -> bool {
        self.overflows() && !self.at_end()
    }

    /// How far one page press travels: a whole number of cards.
    ///
    /// Rounding down to whole cards is what keeps every resting position
    /// card-aligned. The strip is sized to hold an exact number of cards, so
    /// starting from the left edge - and, because the maximum offset is itself
    /// a whole number of strides, ending at the right edge too - no press can
    /// leave a card sliced in half against a chevron.
    pub(crate) fn page_delta(&self) -> f32 {
        let raw = self.viewport_width * SHELF_PAGE_FRACTION;
        if self.card_stride <= 0.0 {
            return raw;
        }
        (raw / self.card_stride).floor().max(1.0) * self.card_stride
    }

    /// The offset `scroll` would land on, clamped to the real range so a press
    /// at either edge is a no-op rather than an over-scroll.
    pub(crate) fn offset_after(&self, scroll: ShelfScroll) -> f32 {
        let target = match scroll {
            ShelfScroll::PageLeft => self.offset_x - self.page_delta(),
            ShelfScroll::PageRight => self.offset_x + self.page_delta(),
            ShelfScroll::Start => 0.0,
            ShelfScroll::End => self.max_offset(),
        };
        target.clamp(0.0, self.max_offset())
    }

    /// The signed distance `scroll` moves, in scroll-offset terms. Zero when
    /// there is nowhere to go.
    pub(crate) fn scroll_distance(&self, scroll: ShelfScroll) -> f32 {
        self.offset_after(scroll) - self.offset_x
    }

    /// Whether `scroll` would change anything.
    pub(crate) fn can_scroll(&self, scroll: ShelfScroll) -> bool {
        self.scroll_distance(scroll).abs() > SHELF_EDGE_EPSILON
    }
}

/// What the shelf remembers between frames.
///
/// Held in egui's per-context temporary store rather than in the application
/// struct: it is presentation state for one widget, it must not be persisted to
/// disk, and keeping it here means `show_gamer_view`'s signature does not grow a
/// parameter that every caller and test would have to thread through.
#[derive(Debug, Clone, Default)]
pub(crate) struct PlatformShelfState {
    pub(crate) metrics: PlatformShelfMetrics,
    /// A press waiting to be applied. Applied inside the scroll area on the
    /// next frame, because that is the only place egui will bind a scroll delta
    /// to this particular scroll area.
    pending: Option<ShelfScroll>,
    /// Where the shelf and its parts ended up, kept so a rendered test can
    /// assert the layout without reaching into egui's widget internals.
    pub(crate) geometry: ShelfGeometry,
    /// The selected platform as of the last frame, so a change can be detected
    /// and the newly selected card scrolled back into view. The outer `Option`
    /// distinguishes "not recorded yet" from "recorded as All".
    last_selected: Option<Option<String>>,
}

/// The shelf's state lives under one fixed id rather than one derived from the
/// parent `Ui`. There is exactly one platform shelf in the application, so a
/// stable id is both accurate and readable from a test that renders the whole
/// window.
pub(crate) fn platform_shelf_state_id() -> egui::Id {
    egui::Id::new("gamer_platform_shelf_nav")
}

/// Reads the shelf's remembered state.
pub(crate) fn platform_shelf_state(ctx: &egui::Context, id: egui::Id) -> PlatformShelfState {
    ctx.data(|data| data.get_temp::<PlatformShelfState>(id))
        .unwrap_or_default()
}

/// Writes the shelf's remembered state.
pub(crate) fn set_platform_shelf_state(
    ctx: &egui::Context,
    id: egui::Id,
    state: PlatformShelfState,
) {
    ctx.data_mut(|data| data.insert_temp(id, state));
}

/// Horizontal padding reserved inside a platform card around its label
/// text (artwork and card border share the rest of `card_width`).
pub(crate) const PLATFORM_LABEL_HORIZONTAL_PADDING: f32 = 16.0;
/// Assumed average glyph advance width, in pixels, for the platform
/// shelf's label font (`FontId::proportional(10.0)`, see
/// `show_platform_shelf_item`). Deliberately conservative: real measured
/// widths for ordinary mixed-case platform names at this size average
/// ~4.4-5.0px/char, so this leaves headroom for names with more
/// uppercase-heavy or wide glyphs than typical without overflowing the
/// card. Because `platform_label_character_limit` derives its ceiling
/// from this same constant, a truncated label is guaranteed (for text at
/// or under this average width) to fit within `card_width -
/// PLATFORM_LABEL_HORIZONTAL_PADDING`, which
/// `compact_platform_label_never_overflows_the_available_card_width`
/// verifies against the real bundled font.
pub(crate) const PLATFORM_LABEL_ASSUMED_PX_PER_CHAR: f32 = 6.5;
/// Never show fewer than this many characters, even for a pathologically
/// narrow card - keeps a truncated label recognisable rather than a bare
/// ellipsis, and gives `compact_platform_label` a defined, panic-free
/// floor for zero, negative, or otherwise degenerate widths.
pub(crate) const PLATFORM_LABEL_MIN_CHARACTERS: usize = 10;

/// The character budget a card of `card_width` gets before truncation
/// kicks in. Scales with `card_width` between `PLATFORM_LABEL_MIN_CHARACTERS`
/// and whatever `PLATFORM_CARD_MAX_WIDTH` itself naturally allows - tied
/// directly to the card-width constants so enlarging or shrinking the
/// platform shelf's cards (`PLATFORM_CARD_MIN_WIDTH`/`_MAX_WIDTH`) can
/// never desynchronise this ceiling from them again, the way a
/// previously separate, unrelated magic number once did.
pub(crate) fn platform_label_character_limit(card_width: f32) -> usize {
    let budget = (card_width - PLATFORM_LABEL_HORIZONTAL_PADDING).max(0.0);
    let natural = (budget / PLATFORM_LABEL_ASSUMED_PX_PER_CHAR).floor() as usize;
    let ceiling = (((PLATFORM_CARD_MAX_WIDTH - PLATFORM_LABEL_HORIZONTAL_PADDING)
        / PLATFORM_LABEL_ASSUMED_PX_PER_CHAR)
        .floor() as usize)
        .max(PLATFORM_LABEL_MIN_CHARACTERS);
    natural.clamp(PLATFORM_LABEL_MIN_CHARACTERS, ceiling)
}

/// Keep the count on its own visible line. Long platform names use the
/// full-name hover/accessibility label and a width-aware compact form in
/// the card rather than forcing the shelf taller.
///
/// Pure and deterministic: the character budget is a fixed arithmetic
/// function of `card_width` (see `platform_label_character_limit`), never
/// a live font/glyph measurement, so this needs no `egui::Context` and
/// behaves identically on every host regardless of installed system
/// fonts.
pub(crate) fn compact_platform_label(label: &str, card_width: f32) -> String {
    let character_limit = platform_label_character_limit(card_width);
    if label.chars().count() <= character_limit {
        return label.to_string();
    }
    let mut compact: String = label
        .chars()
        .take(character_limit.saturating_sub(1))
        .collect();
    compact.push('\u{2026}');
    compact
}

/// One card in the platform shelf: a `Button` (for its built-in Tab
/// focus, Enter/Space activation, visible focus ring, hover, and
/// `.selected` highlight - none of that is reinvented here) with the
/// platform's vector glyph and name/count painted into its bounds, plus
/// an accessible label naming the platform in plain language ("platform
/// artwork must never be the only way to identify a platform").
pub(crate) struct PlatformShelfArtwork<'a> {
    pub(crate) directory: Option<&'a Path>,
    pub(crate) cache: &'a mut PlatformArtworkCache,
}

/// What one frame of the platform shelf produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ShelfOutcome {
    /// The newly chosen filter, when the user picked a different card.
    /// `Some(None)` is the "All" card; the outer `None` means no change.
    pub(crate) chosen: Option<Option<String>>,
    /// Whether the navigation controls were drawn at all.
    pub(crate) controls_visible: bool,
    /// Whether each control could act, which is what "disabled at its edge"
    /// means in practice.
    pub(crate) previous_enabled: bool,
    pub(crate) next_enabled: bool,
    /// The geometry this frame measured, after the strip was laid out. Reported
    /// so a caller - and a test - can see what the controls were derived from
    /// without reaching into egui's internal ids.
    pub(crate) metrics: PlatformShelfMetrics,
    /// The screen rectangles the shelf occupied, so a rendered test can assert
    /// that nothing overlaps the content below and no card hides behind a
    /// chevron. Not used for layout - reported after the fact.
    pub(crate) geometry: ShelfGeometry,
}

/// Where the shelf and its parts ended up on screen.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ShelfGeometry {
    /// The whole shelf row, chevrons included.
    pub(crate) row: egui::Rect,
    /// The scrolling strip itself.
    pub(crate) strip: egui::Rect,
    pub(crate) previous: Option<egui::Rect>,
    pub(crate) next: Option<egui::Rect>,
    /// Every card, in shelf order, in screen coordinates. A card scrolled out
    /// of view still reports its rect, so a test can tell "off-screen" from
    /// "hidden behind a control".
    pub(crate) cards: Vec<egui::Rect>,
    /// Each card's widget id, in the same order as `cards`. Reported so a
    /// test can drive a card through egui's own keyboard focus rather than
    /// by synthesising a click at a coordinate.
    pub(crate) card_ids: Vec<egui::Id>,
}

impl Default for ShelfGeometry {
    fn default() -> Self {
        Self {
            row: egui::Rect::NOTHING,
            strip: egui::Rect::NOTHING,
            previous: None,
            next: None,
            cards: Vec::new(),
            card_ids: Vec::new(),
        }
    }
}

/// One card in the platform shelf: what to draw, and what picking it selects.
#[derive(Debug, Clone)]
pub(crate) struct ShelfEntry<'a> {
    pub(crate) asset_id: String,
    pub(crate) label: &'a str,
    pub(crate) count: usize,
    /// The platform filter this card applies. `None` is the "All" card.
    pub(crate) platform: Option<&'a str>,
}

/// The horizontally scrolling platform picker, with its navigation controls.
///
/// Returns the newly chosen filter when the user picked a different card, so the
/// caller owns the filter change and this function owns none of the filtering
/// logic.
///
/// # Layout
///
/// The chevrons are laid out as siblings of the scroll area, never painted over
/// it, so they cannot cover a card at any width. The strip is given the width
/// that remains after their slots are reserved. Whether they appear at all is
/// decided from the content width against the *full* width available to the row,
/// which does not itself depend on the chevrons being present - so the decision
/// cannot oscillate between frames.
///
/// # Input
///
/// Wheel, trackpad and drag scrolling are untouched: this adds a scroll delta
/// through egui's normal animated scroll request and never sets the offset
/// directly, so it composes with whatever the user does by hand.
pub(crate) fn show_gamer_platform_shelf(
    ui: &mut egui::Ui,
    entries: &[ShelfEntry<'_>],
    selected: Option<&str>,
    card_width: f32,
    artwork: &mut PlatformShelfArtwork<'_>,
    shelf_height: f32,
) -> ShelfOutcome {
    let state_id = platform_shelf_state_id();
    let mut state = platform_shelf_state(ui.ctx(), state_id);
    let mut outcome = ShelfOutcome::default();

    // Measured before anything is laid out, so it is independent of whether the
    // chevrons end up being drawn.
    let full_width = ui.available_width();
    let spacing = ui.spacing().item_spacing.x;

    // Decided from what the shelf *would* be at the preferred card width,
    // computed here rather than read back from last frame's measurement.
    //
    // That matters now that the card width depends on whether the chevrons are
    // shown: a decision fed by the previous frame's `content_width` could see
    // narrower fitted cards, conclude everything fits, hide the chevrons, widen
    // the cards again, and flip back - a shelf that flickers forever at exactly
    // one window width. Derived from the entry count and the preferred width,
    // the answer is a pure function of the row's own width and cannot oscillate.
    let preferred_card_width = card_width;
    let content_at_preferred =
        (entries.len() as f32 * (preferred_card_width + spacing) - spacing).max(0.0);
    let show_controls = content_at_preferred > full_width + SHELF_EDGE_EPSILON;
    outcome.controls_visible = show_controls;
    outcome.previous_enabled = show_controls && state.metrics.can_page_left();
    outcome.next_enabled = show_controls && state.metrics.can_page_right();

    // The space the strip may occupy once both chevrons have been paid for -
    // both, not one, because the row must lay out identically whichever end it
    // is scrolled to. From that, a card width that divides the strip exactly.
    //
    // This is the whole fix for the reported defect. The strip used to be given
    // whatever width happened to remain, which almost never divided evenly by a
    // card, so the rightmost card was sliced by the strip's clip edge with the
    // chevron sitting 8px beyond it - measured at 14-124px of card cut off,
    // depending on window width. A person reads a card cut off flush against a
    // button as a card hidden *underneath* that button.
    let usable_strip_width = if show_controls {
        (full_width - 2.0 * shelf_chevron_reserve(spacing)).max(PLATFORM_CARD_MIN_WIDTH)
    } else {
        full_width
    };
    let card_width = if show_controls {
        shelf_fitted_card_width(usable_strip_width, preferred_card_width, spacing)
    } else {
        preferred_card_width
    };

    // A selection change re-reveals the selected card. Detected here, before the
    // cards are drawn, because the scroll request has to be made as the card
    // itself is laid out.
    let selection_changed = state
        .last_selected
        .as_ref()
        .is_some_and(|last| last.as_deref() != selected);

    let mut requested: Option<ShelfScroll> = None;
    let mut focusable: Vec<egui::Id> = Vec::new();
    let mut card_rects: Vec<egui::Rect> = Vec::new();
    let mut card_ids: Vec<egui::Id> = Vec::new();

    // Reserve exactly the shelf's own height, then draw inside it.
    //
    // Both halves matter. Reserving the height up front keeps the shelf's
    // vertical boundary exactly where it was, so nothing downstream can be
    // pushed into or overlapped - and unlike `allocate_ui_with_layout`, which
    // shrinks to its content, it also keeps the strip's scrollbar inside the
    // shelf rather than letting it hang below.
    //
    // `Align::Min` is equally essential: `ScrollArea` builds its content `Ui`
    // from the *parent's* layout, so a vertically centred parent made the cards
    // centre themselves against the whole remaining page height - over 1000px at
    // 1080p - and spill far below the strip. That is exactly how the cards came
    // to overlap the game list.
    let (row_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), shelf_height),
        egui::Sense::hover(),
    );
    outcome.geometry.row = row_rect;
    let mut row_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    {
        let ui = &mut row_ui;
        if show_controls {
            let response = shelf_chevron(
                ui,
                SHELF_PREVIOUS_GLYPH,
                "Previous platforms",
                outcome.previous_enabled,
            );
            focusable.push(response.id);
            outcome.geometry.previous = Some(response.rect);
            if response.clicked() {
                requested = Some(ShelfScroll::PageLeft);
            }
        }

        // The leading chevron has already consumed its own width, so only the
        // trailing one is still to be paid for here.
        let reserved = if show_controls {
            shelf_chevron_reserve(spacing)
        } else {
            0.0
        };
        let remaining = (ui.available_width() - reserved).max(0.0);
        let strip_width = if show_controls {
            shelf_strip_width(remaining, card_width, spacing)
        } else {
            remaining.max(card_width)
        };

        let output = egui::ScrollArea::horizontal()
            .id_salt("gamer_view_platform_shelf")
            .max_height(shelf_height)
            .max_width(strip_width)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                // A press recorded last frame is applied here, inside the scroll
                // area, which is where egui binds an animated scroll request.
                // The sign is inverted because a scroll delta describes moving
                // the content, not the viewport.
                if let Some(scroll) = state.pending.take() {
                    let distance = state.metrics.scroll_distance(scroll);
                    if distance != 0.0 {
                        ui.scroll_with_delta(egui::vec2(-distance, 0.0));
                    }
                }
                ui.horizontal(|ui| {
                    for entry in entries {
                        let is_selected = entry.platform == selected;
                        let response = show_platform_shelf_item(
                            ui,
                            is_selected,
                            &entry.asset_id,
                            entry.label,
                            entry.count,
                            card_width,
                            artwork,
                        );
                        focusable.push(response.id);
                        card_rects.push(response.rect);
                        card_ids.push(response.id);
                        if is_selected && selection_changed {
                            // Minimal movement: bring it just into view rather
                            // than recentring, so the shelf does not lurch.
                            ui.scroll_to_rect(response.rect, None);
                        }
                        if response.clicked() && !is_selected {
                            outcome.chosen = Some(entry.platform.map(str::to_owned));
                        }
                    }
                });
            });

        if show_controls {
            // Any width the strip declined to use (only when the readable
            // minimum clamp bound) becomes a gutter here, so the trailing
            // chevron stays flush with the row's right edge and the gap falls
            // between the last card and the button rather than beyond it.
            ui.add_space((remaining - strip_width).max(0.0));
            let response =
                shelf_chevron(ui, SHELF_NEXT_GLYPH, "Next platforms", outcome.next_enabled);
            focusable.push(response.id);
            outcome.geometry.next = Some(response.rect);
            if response.clicked() {
                requested = Some(ShelfScroll::PageRight);
            }
        }

        // Re-measured every frame, so a window resize, a filter change and a
        // hand-scroll all update the button states with no extra bookkeeping.
        state.metrics = PlatformShelfMetrics {
            offset_x: output.state.offset.x,
            content_width: output.content_size.x,
            viewport_width: output.inner_rect.width(),
            card_stride: card_width + spacing,
        };
        outcome.metrics = state.metrics;
        outcome.geometry.strip = output.inner_rect;
    }

    // Keyboard and D-pad, active only while focus is inside the shelf, so these
    // keys keep their meaning everywhere else in the window.
    if let Some(scroll) = shelf_keyboard_scroll(ui.ctx(), &focusable) {
        requested = Some(scroll);
    }

    outcome.geometry.cards = card_rects;
    outcome.geometry.card_ids = card_ids;

    if let Some(scroll) = requested
        && state.metrics.can_scroll(scroll)
    {
        state.pending = Some(scroll);
        // The animation runs over several frames, and the offset it lands on is
        // what re-enables or disables each chevron.
        ui.ctx().request_repaint();
    }
    state.last_selected = Some(selected.map(str::to_owned));
    state.geometry = outcome.geometry.clone();
    set_platform_shelf_state(ui.ctx(), state_id, state);
    outcome
}

/// One navigation chevron.
///
/// Sized to exactly one platform card - `PLATFORM_CARD_HEIGHT`, not the shelf
/// height - so it lines up with the cards beside it and can never be the thing
/// that makes the row taller. Top-aligned by the row's layout, so it shares the
/// cards' top edge.
///
/// Disabled rather than hidden at its edge: the slot stays where it is, so the
/// strip does not resize under the pointer and a D-pad user does not lose the
/// control they were aiming at. The hover text doubles as the accessible name,
/// matching the convention `show_platform_shelf_item` documents.
pub(crate) fn shelf_chevron(
    ui: &mut egui::Ui,
    glyph: &str,
    accessible_name: &str,
    enabled: bool,
) -> egui::Response {
    // The slot is allocated exactly, and the button is centred inside it: an
    // `egui::Button` positions its label with the surrounding `Ui`'s alignment,
    // so in this top-aligned row the glyph would otherwise sit against the top
    // edge rather than beside the card artwork.
    ui.allocate_ui_with_layout(
        egui::vec2(SHELF_CHEVRON_WIDTH, PLATFORM_CARD_HEIGHT),
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        |ui| {
            ui.add_enabled(enabled, egui::Button::new(glyph))
                .on_hover_text(accessible_name)
        },
    )
    .inner
}

/// The scroll a key press asks for, when focus is on one of `focusable` - the
/// chevrons or any platform card.
///
/// Scoped to the shelf's own widgets deliberately: Left/Right/Home/End all mean
/// something else elsewhere, so they are only consumed while the shelf really
/// has focus. That is also what makes this work from a TV remote or a game pad
/// mapped to arrow keys.
pub(crate) fn shelf_keyboard_scroll(
    ctx: &egui::Context,
    focusable: &[egui::Id],
) -> Option<ShelfScroll> {
    let focused = ctx.memory(|memory| memory.focused())?;
    if !focusable.contains(&focused) {
        return None;
    }
    ctx.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
            Some(ShelfScroll::PageRight)
        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
            Some(ShelfScroll::PageLeft)
        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
            Some(ShelfScroll::Start)
        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::End) {
            Some(ShelfScroll::End)
        } else {
            None
        }
    })
}

pub(crate) fn show_platform_shelf_item(
    ui: &mut egui::Ui,
    selected: bool,
    asset_id: &str,
    label: &str,
    count: usize,
    card_width: f32,
    artwork: &mut PlatformShelfArtwork<'_>,
) -> egui::Response {
    const ARTWORK_SIZE: f32 = 108.0;
    // Dedicated assets already name the exact platform; a category
    // fallback additionally names *what kind of thing* the glyph is
    // meant to evoke, so a screen-reader user gets the same context a
    // sighted user reads from a recognisable shape.
    let accessible_name = if bundled_platform_artwork(asset_id).is_some() || asset_id == "unknown" {
        format!("{label}, {count} games")
    } else {
        let category = platform_asset_category(label).accessible_label();
        format!("{label} ({category}), {count} games")
    };
    let response = ui
        .add(
            egui::Button::new("")
                .min_size(egui::vec2(card_width, PLATFORM_CARD_HEIGHT))
                .selected(selected),
        )
        .on_hover_text(accessible_name.clone());
    // AccessKit's exposed name for a widget with no text label defaults
    // to whatever hover text/label is attached - `egui::Response` doesn't
    // expose a way to set the accessible name in this egui version
    // without a full custom widget, so the hover text above doubles as
    // that label, matching "accessible labels for visuals."
    let artwork_center = response.rect.center() - egui::vec2(0.0, 15.0);
    let color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().text_color().gamma_multiply(0.8)
    };
    let fallback_asset_id = match asset_id {
        "console" | "handheld" | "computer" | "arcade" | "optical-disc" | "cartridge"
        | "unknown" => asset_id,
        _ => platform_asset_category(label).asset_id(),
    };
    paint_platform_artwork_at(
        ui,
        artwork.cache,
        artwork.directory,
        PlatformArtworkPaint {
            center: artwork_center,
            size: ARTWORK_SIZE,
            color,
            asset_id,
            fallback_asset_id,
        },
    );
    let text_pos = response.rect.center() + egui::vec2(0.0, 59.0);
    let truncated_label = compact_platform_label(label, card_width);
    ui.painter().text(
        text_pos,
        egui::Align2::CENTER_CENTER,
        format!("{truncated_label}\n{count}"),
        egui::FontId::proportional(10.0),
        ui.visuals().text_color(),
    );
    response
}
