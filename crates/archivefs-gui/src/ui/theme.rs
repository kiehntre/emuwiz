use eframe::egui;

pub(crate) const CONTENT_MAX_WIDTH: f32 = 1080.0;
pub(crate) const WIDE_CONTENT_MAX_WIDTH: f32 = 1560.0;
pub(crate) const PAGE_GUTTER: f32 = 24.0;
pub(crate) const SECTION_GAP: f32 = 20.0;

/// Darkened from (74, 126, 232) (2026-08-22, live-QA Phase 8 contrast
/// audit): this is also `selection.bg_fill` and `widgets.active.bg_fill`,
/// so it sits directly behind primary-button and selected-row text. White
/// text on the old value measured ~3.86:1 - under the 4.5:1 WCAG AA
/// threshold for normal-size text. This measures ~5.58:1.
pub(crate) const ACCENT: egui::Color32 = egui::Color32::from_rgb(58, 99, 196);
/// Darkened from (91, 143, 248) alongside [`ACCENT`] for the same reason;
/// the old value measured ~3.12:1 with white text, this measures ~4.9:1.
pub(crate) const ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(64, 109, 205);
pub(crate) const SUCCESS: egui::Color32 = egui::Color32::from_rgb(70, 176, 118);
pub(crate) const WARNING: egui::Color32 = egui::Color32::from_rgb(221, 166, 62);
/// Lightened from (214, 82, 88) (2026-08-22, live-QA Phase 7 contrast
/// audit): against this theme's actual `panel_fill`/`faint_bg_color`
/// (both far darker than egui's stock dark theme), the old value measured
/// ~4.26:1 - just under the 4.5:1 WCAG AA threshold for normal text. This
/// measures ~5.3:1 against both.
pub(crate) const DANGER: egui::Color32 = egui::Color32::from_rgb(224, 105, 110);
pub(crate) const INFO: egui::Color32 = egui::Color32::from_rgb(86, 154, 214);
/// Muted/secondary/disabled text - hint text, secondary labels, status
/// captions. Explicit rather than delegating to egui's
/// `Visuals::weak_text_color()` (2026-08-22, live-QA Phase 7 contrast
/// audit): that default is tuned for egui's stock dark background, not
/// this theme's darker custom `panel_fill`/`faint_bg_color`/
/// `extreme_bg_color`, so it under-contrasted against them. This measures
/// ~6.9:1 against `panel_fill`, comfortably above the 4.5:1 WCAG AA
/// threshold for normal text.
pub(crate) const MUTED_TEXT: egui::Color32 = egui::Color32::from_rgb(158, 165, 176);

pub(crate) fn apply(context: &egui::Context) {
    context.style_mut(|style| {
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(27.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(16.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        );
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.menu_margin = egui::Margin::same(8);
        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.widgets.active.bg_fill = ACCENT;
        style.visuals.widgets.hovered.bg_fill = ACCENT_HOVER;
        style.visuals.widgets.open.bg_fill = ACCENT;
        // Text/border color on top of the accent fills above. egui's stock
        // defaults for these states are tuned for its own lighter dark
        // theme and were never overridden here, so blue primary buttons,
        // selected sidebar rows/tabs, and open dropdowns rendered with a
        // faint or barely-visible selection border (2026-08-22, live-QA
        // Phase 8 contrast audit). Pure white against `ACCENT`/`ACCENT_HOVER`
        // is the highest-contrast choice available without changing the
        // accent hue itself; see the ratios noted on those constants.
        style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.widgets.noninteractive.bg_stroke =
            egui::Stroke::new(1.0_f32, egui::Color32::from_gray(66));
        style.visuals.panel_fill = egui::Color32::from_rgb(24, 27, 33);
        style.visuals.faint_bg_color = egui::Color32::from_rgb(31, 35, 43);
        style.visuals.extreme_bg_color = egui::Color32::from_rgb(18, 21, 26);
        // Disabled controls (blocked actions, unavailable buttons) render
        // with egui's stock disabled-text gray by default, which under-
        // contrasted against this theme's darker custom backgrounds - the
        // same issue `MUTED_TEXT` fixes for hint/secondary text (2026-08-22,
        // live-QA Phase 7 contrast audit).
        style.visuals.widgets.noninteractive.fg_stroke =
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(210, 214, 221));
        style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, MUTED_TEXT);
    });
}

pub(crate) fn muted(ui: &egui::Ui) -> egui::Color32 {
    let _ = ui;
    MUTED_TEXT
}

pub(crate) fn card_fill(ui: &egui::Ui) -> egui::Color32 {
    ui.visuals().faint_bg_color
}

pub(crate) fn border(ui: &egui::Ui) -> egui::Stroke {
    egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color)
}
