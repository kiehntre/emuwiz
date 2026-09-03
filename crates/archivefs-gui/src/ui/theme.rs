use eframe::egui;

pub(crate) const CONTENT_MAX_WIDTH: f32 = 1080.0;
pub(crate) const WIDE_CONTENT_MAX_WIDTH: f32 = 1560.0;
pub(crate) const PAGE_GUTTER: f32 = 24.0;
pub(crate) const SECTION_GAP: f32 = SPACE_XL;

pub(crate) const SPACE_XS: f32 = 4.0;
pub(crate) const SPACE_SM: f32 = 8.0;
pub(crate) const SPACE_MD: f32 = 12.0;
pub(crate) const SPACE_LG: f32 = 16.0;
pub(crate) const SPACE_XL: f32 = 24.0;
pub(crate) const SPACE_2XL: f32 = 32.0;

pub(crate) const DISPLAY_SIZE: f32 = 34.0;
pub(crate) const PAGE_TITLE_SIZE: f32 = 27.0;
pub(crate) const SECTION_TITLE_SIZE: f32 = 19.0;
pub(crate) const BODY_SIZE: f32 = 16.0;
pub(crate) const METADATA_SIZE: f32 = 14.0;
pub(crate) const TECHNICAL_SIZE: f32 = 12.0;

/// Restrained amber for the one primary action and selected-content emphasis.
/// It is deliberately darker than the source Stitch swatch so light interface
/// text remains readable and amber does not become the application's chrome.
pub(crate) const APP_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(13, 21, 19);
pub(crate) const DEEP_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(8, 16, 14);
pub(crate) const CARD_SURFACE: egui::Color32 = egui::Color32::from_rgb(22, 29, 27);
pub(crate) const RAISED_SURFACE: egui::Color32 = egui::Color32::from_rgb(30, 40, 37);
pub(crate) const BORDER_SUBTLE: egui::Color32 = egui::Color32::from_rgb(35, 50, 45);
pub(crate) const BORDER_FOCUS: egui::Color32 = egui::Color32::from_rgb(59, 82, 74);
pub(crate) const PRIMARY_TEXT: egui::Color32 = egui::Color32::from_rgb(241, 245, 249);
pub(crate) const SECONDARY_TEXT: egui::Color32 = egui::Color32::from_rgb(148, 163, 184);
pub(crate) const TECHNICAL_TEXT: egui::Color32 = egui::Color32::from_rgb(100, 116, 139);
pub(crate) const TEAL: egui::Color32 = egui::Color32::from_rgb(3, 198, 178);
/// Primary actions and selected-content emphasis. Keep this role sparse.
pub(crate) const AMBER: egui::Color32 = egui::Color32::from_rgb(143, 97, 31);
/// Preserved semantic API name: ACCENT is the primary action role.
pub(crate) const ACCENT: egui::Color32 = AMBER;
pub(crate) const ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(170, 119, 43);
pub(crate) const SUCCESS: egui::Color32 = egui::Color32::from_rgb(16, 185, 129);
pub(crate) const WARNING: egui::Color32 = AMBER;
/// Lightened from (214, 82, 88) (2026-08-22, live-QA Phase 7 contrast
/// audit): against this theme's actual `panel_fill`/`faint_bg_color`
/// (both far darker than egui's stock dark theme), the old value measured
/// ~4.26:1 - just under the 4.5:1 WCAG AA threshold for normal text. This
/// measures ~5.3:1 against both.
pub(crate) const DANGER: egui::Color32 = egui::Color32::from_rgb(239, 68, 68);
pub(crate) const INFO: egui::Color32 = TEAL;
/// Muted/secondary/disabled text - hint text, secondary labels, status
/// captions. Explicit rather than delegating to egui's
/// `Visuals::weak_text_color()` (2026-08-22, live-QA Phase 7 contrast
/// audit): that default is tuned for egui's stock dark background, not
/// this theme's darker custom `panel_fill`/`faint_bg_color`/
/// `extreme_bg_color`, so it under-contrasted against them. This measures
/// ~6.9:1 against `panel_fill`, comfortably above the 4.5:1 WCAG AA
/// threshold for normal text.
pub(crate) const MUTED_TEXT: egui::Color32 = SECONDARY_TEXT;

pub(crate) fn apply(context: &egui::Context) {
    context.style_mut(|style| {
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(PAGE_TITLE_SIZE, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(BODY_SIZE, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(METADATA_SIZE, egui::FontFamily::Proportional),
        );
        style.spacing.item_spacing = egui::vec2(SPACE_MD, SPACE_SM);
        style.spacing.button_padding = egui::vec2(SPACE_MD, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.menu_margin = egui::Margin::same(SPACE_SM as i8);
        // Selection is a dark teal system state. Amber is reserved for
        // explicit primary actions and attention, rather than every selected
        // navigation/tab widget.
        style.visuals.selection.bg_fill = egui::Color32::from_rgb(12, 74, 69);
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(15, 92, 85);
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(28, 67, 61);
        style.visuals.widgets.open.bg_fill = egui::Color32::from_rgb(15, 92, 85);
        style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.widgets.noninteractive.bg_stroke =
            egui::Stroke::new(1.0_f32, egui::Color32::from_gray(66));
        style.visuals.panel_fill = APP_BACKGROUND;
        style.visuals.faint_bg_color = CARD_SURFACE;
        style.visuals.extreme_bg_color = DEEP_BACKGROUND;
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
    let _ = ui;
    CARD_SURFACE
}

pub(crate) fn border(ui: &egui::Ui) -> egui::Stroke {
    let _ = ui;
    egui::Stroke::new(1.0_f32, BORDER_SUBTLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_palette_keeps_roles_distinct() {
        assert_eq!(ACCENT, AMBER);
        assert_ne!(TEAL, AMBER);
        assert_ne!(SUCCESS, DANGER);
        assert_ne!(PRIMARY_TEXT, TECHNICAL_TEXT);
    }

    #[test]
    fn primary_amber_keeps_light_action_text_contrastable() {
        fn luminance(color: egui::Color32) -> f32 {
            let channel = |value: u8| {
                let value = value as f32 / 255.0;
                if value <= 0.03928 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
        }
        let contrast = (luminance(egui::Color32::WHITE) + 0.05) / (luminance(AMBER) + 0.05);
        assert!(
            contrast >= 4.5,
            "amber/light-text contrast was {contrast:.2}:1"
        );
    }
}
