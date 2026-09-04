//! The dedicated Gamer View layout model.
//!
//! Gamer View is composed as a vertical stack of regions - a compact platform
//! collection strip, a dominant selected-game stage, and a subordinate
//! browsing rail beneath it - rather than the old list-left / detail-right
//! split. [`GamerStageLayout`] turns the space the central panel was actually
//! given into the heights and internal proportions each region should use.
//!
//! It is deliberately pure: identical input always produces identical output,
//! and nothing here reads or writes egui state, touches a disk, or measures a
//! previous frame. That is what lets the composition be reasoned about and
//! unit-tested directly, and it replaces the previous "measure the action
//! block, then shrink the artwork from whatever is left" heuristic with an
//! explicit, deterministic budget.

use eframe::egui;

/// Region heights and the stage's internal split for one Gamer View frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GamerStageLayout {
    /// Height reserved for the compact platform collection strip (Region 2).
    pub(crate) strip_height: f32,
    /// Height reserved for the dominant selected-game stage (Region 3). This
    /// is always the larger share of the space below the strip so the stage
    /// reads as the primary surface.
    pub(crate) stage_height: f32,
    /// Width of the stage's media plate. The stage's text column takes the
    /// remaining width (up to [`Self::STAGE_TEXT_MAX_WIDTH`]).
    pub(crate) stage_media_width: f32,
    /// `true` when the stage lays its media plate beside the text column;
    /// `false` on a window too narrow for that, where the stage stacks the
    /// media above the text instead.
    pub(crate) stage_side_by_side: bool,
    /// Minimum height the browsing rail keeps even on the shortest supported
    /// window (Region 4). The rail scrolls its card grid within whatever it
    /// is given.
    pub(crate) rail_min_height: f32,
    /// Number of columns in the browsing rail's card grid.
    pub(crate) rail_columns: usize,
    /// Vertical gap between regions.
    pub(crate) region_gap: f32,
}

impl GamerStageLayout {
    /// Smallest the stage is ever drawn - below this the selected game stops
    /// reading as a presentation surface.
    pub(crate) const MIN_STAGE_HEIGHT: f32 = 264.0;
    /// Largest the stage is drawn, so a tall 1440p/4K window does not turn one
    /// cover into a wall-sized poster with acres of empty card around it.
    pub(crate) const MAX_STAGE_HEIGHT: f32 = 460.0;
    /// Fixed height of the platform collection strip. A single known value
    /// keeps the space below it predictable regardless of how many platforms
    /// the library holds.
    pub(crate) const STRIP_HEIGHT: f32 = 150.0;
    /// Vertical rhythm between the three regions.
    pub(crate) const REGION_GAP: f32 = 20.0;
    /// The rail keeps at least this much height on the shortest window; it
    /// scrolls rather than pushing the stage smaller. Raised from the
    /// original 150px so that even a cramped 1100x720 physical window still
    /// shows a couple of full card rows plus a hint of the next one, rather
    /// than a sliver that barely reads as scrollable.
    pub(crate) const RAIL_MIN_HEIGHT: f32 = 220.0;
    /// Share of the stage+rail budget the stage is given before being
    /// clamped into its min/max band. Tuned so the stage reads as dominant
    /// without eating space the rail needs to be usable at the real 1100x720
    /// physical target - see `physical_target_1100x720_...` below.
    const STAGE_SHARE: f32 = 0.52;
    /// Target width one browsing-rail card wants. The column count is how many
    /// of these fit across the available width, clamped to a sane range.
    pub(crate) const RAIL_CARD_TARGET_WIDTH: f32 = 340.0;
    /// The stage's text column never stretches wider than this, however wide
    /// the window; extra width goes to the media plate and margins instead of
    /// a single 1500px line of body text. Shares the value the previous
    /// featured panel used for the same purpose.
    pub(crate) const STAGE_TEXT_MAX_WIDTH: f32 =
        crate::gamer_artwork::GAMER_FEATURED_CONTENT_MAX_WIDTH;
    /// Below this available width the stage stacks (media above text) rather
    /// than placing them side by side.
    pub(crate) const SIDE_BY_SIDE_MIN_WIDTH: f32 = 720.0;
    /// Combined vertical padding inside the stage card (top + bottom).
    const STAGE_INNER_PADDING: f32 = 48.0;

    /// Computes the layout from the space the Gamer View central panel was
    /// given, *after* the platform strip's own height has already been
    /// accounted for by the caller having reserved it.
    pub(crate) fn compute(available: egui::Vec2) -> Self {
        let width = available.x.max(320.0);
        let height = available.y.max(360.0);

        let region_gap = Self::REGION_GAP;
        let strip_height = Self::STRIP_HEIGHT;

        // `available` is the central panel's remaining space *after* the
        // caller has already drawn the platform strip and its trailing gap
        // (see the call site in `gamer_view.rs`, which captures
        // `ui.available_height()` only once the shelf is behind it). So the
        // only budget left to divide here is the stage, the rail, and the
        // single gap between those two - not a second helping of the strip.
        let remaining = (height - region_gap).max(180.0);

        // The stage takes the majority of that space so it is unmistakably the
        // primary region, then is clamped into a sensible band and finally
        // held back far enough to leave the rail its minimum height whenever
        // the window is tall enough to afford it.
        let stage_height = (remaining * Self::STAGE_SHARE)
            .clamp(Self::MIN_STAGE_HEIGHT, Self::MAX_STAGE_HEIGHT)
            .min((remaining - Self::RAIL_MIN_HEIGHT).max(Self::MIN_STAGE_HEIGHT.min(remaining)));

        let rail_min_height = (remaining - stage_height).max(Self::RAIL_MIN_HEIGHT);

        let stage_side_by_side = width >= Self::SIDE_BY_SIDE_MIN_WIDTH;
        let inner_height = (stage_height - Self::STAGE_INNER_PADDING).max(120.0);
        let stage_media_width = if stage_side_by_side {
            // A portrait plate sized off the stage height, never so wide it
            // crowds the text column out on a modest window. Raised from an
            // 0.72/150-300 budget that under-supplied width relative to the
            // stage's own height: `featured_cover_box` picks the smaller of
            // this width-derived bound and the height budget, so a column
            // narrower than `height_budget * cover_aspect` was silently
            // capping a real cover below what the stage's height already
            // allowed - the actual cause of covers reading as too small at
            // the 1100x720 physical target once the stage/rail height split
            // itself was correct.
            (inner_height * 0.95).clamp(180.0, 420.0).min(width * 0.42)
        } else {
            (width - Self::STAGE_INNER_PADDING).clamp(150.0, 320.0)
        };

        let rail_columns = ((width / Self::RAIL_CARD_TARGET_WIDTH).floor() as usize).clamp(2, 5);

        Self {
            strip_height,
            stage_height,
            stage_media_width,
            stage_side_by_side,
            rail_min_height,
            rail_columns,
            region_gap,
        }
    }

    /// The width the stage's text column should use given the full stage
    /// width, honouring [`Self::STAGE_TEXT_MAX_WIDTH`].
    pub(crate) fn stage_text_width(&self, stage_width: f32) -> f32 {
        if self.stage_side_by_side {
            (stage_width - self.stage_media_width - Self::REGION_GAP)
                .clamp(240.0, Self::STAGE_TEXT_MAX_WIDTH)
        } else {
            stage_width.min(Self::STAGE_TEXT_MAX_WIDTH)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(w: f32, h: f32) -> GamerStageLayout {
        GamerStageLayout::compute(egui::vec2(w, h))
    }

    #[test]
    fn physical_target_1100x720_puts_the_stage_in_the_majority_and_keeps_a_usable_rail() {
        // `compute` is called with `ui.available_height()` captured *after*
        // the app has already drawn the search bar, the maintenance-action
        // row, and the platform strip with its trailing gap (see the call
        // site comment in `gamer_view.rs`), so it never sees the full 720px
        // window - ~680 wide is representative of what is actually left at
        // the real 1100x720 physical target.
        let l = layout(1052.0, 680.0);
        assert_eq!(l.strip_height, GamerStageLayout::STRIP_HEIGHT);
        // Stage is the dominant region but no longer swallows space the rail
        // needs: tuned to land near 330-350px rather than the ~390px a naive
        // 60% share (plus the old double-counted strip subtraction) used to
        // produce.
        assert!(
            (325.0..=355.0).contains(&l.stage_height),
            "stage should land in the 330-350px band at the physical target, got {}",
            l.stage_height
        );
        assert!(
            l.stage_height > l.rail_min_height,
            "stage must dominate the rail"
        );
        // Rail now gets a comfortably scrollable minimum - enough for two
        // full card rows plus a hint of the next one - rather than a sliver.
        assert!(
            l.rail_min_height >= 220.0,
            "rail should get at least ~220px at the physical target, got {}",
            l.rail_min_height
        );
        // Three columns around the real physical width.
        assert_eq!(l.rail_columns, 3);
        assert!(l.stage_side_by_side);
    }

    #[test]
    fn stage_height_is_bounded_on_a_tall_window() {
        let l = layout(1600.0, 1400.0);
        assert!(l.stage_height <= GamerStageLayout::MAX_STAGE_HEIGHT);
        // The extra vertical space flows to the rail, not an ever-taller hero.
        assert!(l.rail_min_height > l.stage_height);
    }

    #[test]
    fn window_1024x600_remains_usable() {
        let l = layout(1024.0, 600.0);
        assert!(l.stage_height >= GamerStageLayout::MIN_STAGE_HEIGHT - 8.0);
        assert!(l.stage_height > l.rail_min_height, "stage stays dominant");
        assert!(
            l.rail_min_height >= 200.0,
            "rail is still usable when short"
        );
        assert_eq!(l.rail_columns, 3);
    }

    #[test]
    fn window_1440x900_stays_balanced_and_generous() {
        let l = layout(1440.0, 900.0);
        // The stage may grow again on a taller window, but stays capped.
        assert!(l.stage_height <= GamerStageLayout::MAX_STAGE_HEIGHT);
        // The rail is not capped and keeps growing with the window.
        assert!(
            l.rail_min_height > 380.0,
            "rail should stay generous on a bigger window, got {}",
            l.rail_min_height
        );
        assert_eq!(l.rail_columns, 4);
    }

    #[test]
    fn wider_windows_add_rail_columns_without_a_separate_composition() {
        assert_eq!(layout(1052.0, 720.0).rail_columns, 3);
        assert_eq!(layout(1400.0, 900.0).rail_columns, 4);
        assert_eq!(layout(1800.0, 1000.0).rail_columns, 5);
        // Capped: this slice is a browsing rail, not a cover wall.
        assert_eq!(layout(3200.0, 1600.0).rail_columns, 5);
    }

    #[test]
    fn a_narrow_window_stacks_the_stage_and_never_drops_below_two_columns() {
        let l = layout(680.0, 720.0);
        assert!(!l.stage_side_by_side);
        assert_eq!(l.rail_columns, 2);
        assert!(l.stage_media_width >= 150.0);
    }

    #[test]
    fn the_media_plate_is_wide_enough_for_a_real_cover_to_fill_its_height_budget() {
        // The media column has to be at least `inner_height * cover_aspect`
        // wide, or `featured_cover_box` clips the cover's height to fit the
        // column instead of the stage's actual height budget - which is
        // exactly what made real covers measure far smaller than the stage
        // height allowed before this fix.
        let l = layout(1052.0, 680.0);
        let inner_height = l.stage_height - 48.0; // GamerStageLayout::STAGE_INNER_PADDING
        let real_cover_aspect = 3.0 / 4.0; // gamer_artwork::FEATURED_COVER_ASPECT
        assert!(
            l.stage_media_width >= inner_height * real_cover_aspect,
            "media column ({}) is narrower than a real cover at the stage's own \
             height budget needs ({})",
            l.stage_media_width,
            inner_height * real_cover_aspect
        );
    }

    #[test]
    fn the_media_plate_grows_on_a_wider_window() {
        let modest = layout(1052.0, 680.0).stage_media_width;
        let wider = layout(1440.0, 900.0).stage_media_width;
        assert!(
            wider > modest,
            "media plate did not grow on a wider window: {modest} -> {wider}"
        );
    }

    #[test]
    fn the_shortest_supported_window_still_leaves_both_regions_present() {
        let l = layout(1052.0, 600.0);
        assert!(l.stage_height >= GamerStageLayout::MIN_STAGE_HEIGHT - 8.0);
        assert!(l.rail_min_height >= 200.0);
    }

    #[test]
    fn stage_height_never_drops_below_its_floor_on_a_degenerate_window() {
        // `compute` clamps its inputs, so even a pathologically tiny window
        // still gets a stage no smaller than what `MIN_STAGE_HEIGHT` and the
        // available space can support - the rail-minimum subtraction must
        // never push it below that floor.
        let l = layout(50.0, 50.0);
        assert!(l.stage_height > 0.0);
        assert!(l.stage_height <= GamerStageLayout::MIN_STAGE_HEIGHT);
    }

    #[test]
    fn the_text_column_is_capped_on_a_very_wide_stage() {
        let l = layout(2400.0, 1000.0);
        let text = l.stage_text_width(1800.0);
        assert!(text <= GamerStageLayout::STAGE_TEXT_MAX_WIDTH);
    }

    #[test]
    fn output_is_pure_and_stable_for_the_same_input() {
        assert_eq!(layout(1280.0, 800.0), layout(1280.0, 800.0));
    }
}
