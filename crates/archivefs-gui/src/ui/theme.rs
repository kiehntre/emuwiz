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

/// The four typography roles. Each is an egui `TextStyle` held in the context
/// style, so a profile (Classic, Blue) re-sizes a role by overwriting that one
/// slot and every page that asks for the role follows it. `Body` is egui's own
/// `TextStyle::Body`; the other three are named styles registered by [`apply`]
/// and by the Blue profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextRole {
    PageTitle,
    SectionTitle,
    CardTitle,
    Body,
}

impl TextRole {
    pub(crate) fn text_style(self) -> egui::TextStyle {
        match self {
            // The page title is egui's heading, so `ui.heading` and the role
            // can never disagree.
            Self::PageTitle => egui::TextStyle::Heading,
            Self::SectionTitle => egui::TextStyle::Name("emuwiz-section-title".into()),
            Self::CardTitle => egui::TextStyle::Name("emuwiz-card-title".into()),
            Self::Body => egui::TextStyle::Body,
        }
    }
}

/// Classic role sizes. Card titles keep the size existing cards already use
/// (egui's heading) so Classic is unchanged at 100%; Blue sets its own scale.
pub(crate) fn set_role_sizes(style: &mut egui::Style, page: f32, section: f32, card: f32) {
    let font = |size: f32| egui::FontId::new(size, egui::FontFamily::Proportional);
    style
        .text_styles
        .insert(TextRole::PageTitle.text_style(), font(page));
    style
        .text_styles
        .insert(TextRole::SectionTitle.text_style(), font(section));
    style
        .text_styles
        .insert(TextRole::CardTitle.text_style(), font(card));
}

/// Page title text: the largest role, once per page.
pub(crate) fn page_title(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .text_style(TextRole::PageTitle.text_style())
        .strong()
}

/// A titled block within a page ("Things you can do", "Screenshots").
pub(crate) fn section_title(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .text_style(TextRole::SectionTitle.text_style())
        .strong()
}

/// The title of one card or panel. Smaller than a section title in Blue; in
/// Classic it keeps the size cards have always had.
pub(crate) fn card_title(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .text_style(TextRole::CardTitle.text_style())
        .strong()
}

/// Shared v2 application surfaces and semantic colors. Normal routes use this
/// module rather than page-local chrome tokens.
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
/// Semantic amber remains available for warnings and attention states. It is
/// deliberately darker than the source Stitch swatch so light interface text
/// remains readable; it is not application chrome.
pub(crate) const AMBER: egui::Color32 = egui::Color32::from_rgb(143, 97, 31);
/// The single blue v2 primary-action role used by native and embedded pages.
pub(crate) const PRIMARY_ACTION: egui::Color32 = egui::Color32::from_rgb(30, 85, 137);
pub(crate) const PRIMARY_ACTION_HOVER: egui::Color32 = egui::Color32::from_rgb(45, 110, 175);
/// Preserved semantic API name: ACCENT is the shared primary action role.
pub(crate) const ACCENT: egui::Color32 = PRIMARY_ACTION;
pub(crate) const ACCENT_HOVER: egui::Color32 = PRIMARY_ACTION_HOVER;
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

/// The approved EmuWiz Blue colour tokens (design handoff
/// `DESIGN_TOKENS_AND_COMPONENTS.md`). The opt-in Simple shell installs them
/// through its style profile; shared widgets read them only while that
/// profile is active ([`blue_profile`]), so Classic is untouched.
pub(crate) mod blue {
    use eframe::egui::Color32;

    pub(crate) const APP_BACKGROUND: Color32 = Color32::from_rgb(0x0a, 0x12, 0x24);
    pub(crate) const DEEP_BACKGROUND: Color32 = Color32::from_rgb(0x08, 0x0f, 0x1f);
    pub(crate) const CARD_SURFACE: Color32 = Color32::from_rgb(0x0e, 0x1a, 0x33);
    pub(crate) const CARD_SURFACE_TOP: Color32 = Color32::from_rgb(0x11, 0x20, 0x42);
    pub(crate) const INSET: Color32 = Color32::from_rgb(0x0b, 0x16, 0x30);
    pub(crate) const RAISED_SURFACE: Color32 = Color32::from_rgb(0x1a, 0x2d, 0x52);
    pub(crate) const BORDER_SUBTLE: Color32 = Color32::from_rgb(0x1f, 0x33, 0x58);
    pub(crate) const BORDER_STRONG: Color32 = Color32::from_rgb(0x2e, 0x4a, 0x7d);
    pub(crate) const SELECTED_FILL: Color32 = Color32::from_rgb(0x17, 0x35, 0x6d);
    pub(crate) const SELECTED_OUTLINE: Color32 = Color32::from_rgb(0x4c, 0x95, 0xff);
    pub(crate) const PRIMARY_TEXT: Color32 = Color32::from_rgb(0xf5, 0xf9, 0xff);
    pub(crate) const BODY_TEXT: Color32 = Color32::from_rgb(0xc6, 0xd2, 0xea);
    pub(crate) const SECONDARY_TEXT: Color32 = Color32::from_rgb(0xae, 0xbc, 0xd8);
    pub(crate) const DIM_TEXT: Color32 = Color32::from_rgb(0x84, 0x96, 0xb8);
    /// The approved `#2F7BFF`, darkened (same hue) until primary text on it
    /// reaches 4.5:1, as the token table allows.
    pub(crate) const PRIMARY_ACTION: Color32 = Color32::from_rgb(0x29, 0x6c, 0xe0);
    pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x6a, 0xb0, 0xff);
    pub(crate) const OK: Color32 = Color32::from_rgb(0x34, 0xd3, 0x99);
    pub(crate) const WARN: Color32 = Color32::from_rgb(0xf5, 0xb8, 0x3d);
    pub(crate) const BAD: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);
    pub(crate) const BAD_TEXT: Color32 = Color32::from_rgb(0xfc, 0xa5, 0xa5);
    /// Only for an Undo that is genuinely available and verified.
    pub(crate) const UNDO: Color32 = Color32::from_rgb(0x6b, 0x5c, 0xf0);
    pub(crate) const UNDO_TEXT: Color32 = Color32::from_rgb(0xb4, 0xad, 0xff);

    /// Keyboard focus ring: 3 px [`ACCENT`] separated from the control by a
    /// 3 px gap.
    pub(crate) const FOCUS_RING_WIDTH: f32 = 3.0;
    pub(crate) const FOCUS_RING_GAP: f32 = 3.0;
}

/// Whether the Blue profile owns this context's style. The Simple shell
/// installs `blue::CARD_SURFACE` as `faint_bg_color`; nothing else does.
pub(crate) fn blue_profile(ui: &egui::Ui) -> bool {
    ui.visuals().faint_bg_color == blue::CARD_SURFACE
}

/// The decorative accent: Classic teal, or the Blue accent in Blue.
pub(crate) fn accent(ui: &egui::Ui) -> egui::Color32 {
    if blue_profile(ui) { blue::ACCENT } else { TEAL }
}

/// Fill of the one primary action on a surface.
pub(crate) fn primary_action_fill(ui: &egui::Ui) -> egui::Color32 {
    if blue_profile(ui) {
        blue::PRIMARY_ACTION
    } else {
        PRIMARY_ACTION
    }
}

/// Fill of a confirm button whose action destroys or replaces something.
pub(crate) fn destructive_fill(ui: &egui::Ui) -> egui::Color32 {
    if blue_profile(ui) {
        // Darkened red so primary text on it stays readable.
        egui::Color32::from_rgb(0xb4, 0x2a, 0x2a)
    } else {
        DANGER
    }
}

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
        set_role_sizes(style, PAGE_TITLE_SIZE, SECTION_TITLE_SIZE, PAGE_TITLE_SIZE);
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

/// Whether the context carries the stock v2 palette set by [`apply`]. A scoped
/// style profile (the opt-in Simple v1 shell) replaces these slots; the
/// surface helpers below then follow it so page cards match the shell.
fn stock_profile(ui: &egui::Ui) -> bool {
    ui.visuals().faint_bg_color == CARD_SURFACE
}

/// Card surface. Exactly [`CARD_SURFACE`] under the stock palette.
pub(crate) fn card_fill(ui: &egui::Ui) -> egui::Color32 {
    ui.visuals().faint_bg_color
}

/// Raised/hovered surface. Exactly [`RAISED_SURFACE`] under the stock palette.
pub(crate) fn raised_fill(ui: &egui::Ui) -> egui::Color32 {
    if stock_profile(ui) {
        RAISED_SURFACE
    } else {
        ui.visuals().widgets.inactive.weak_bg_fill
    }
}

/// Recessed plate. Exactly [`DEEP_BACKGROUND`] under the stock palette.
pub(crate) fn deep_fill(ui: &egui::Ui) -> egui::Color32 {
    if stock_profile(ui) {
        DEEP_BACKGROUND
    } else {
        ui.visuals().extreme_bg_color
    }
}

/// Subtle border colour. Exactly [`BORDER_SUBTLE`] under the stock palette.
pub(crate) fn border_color(ui: &egui::Ui) -> egui::Color32 {
    if stock_profile(ui) {
        BORDER_SUBTLE
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    }
}

/// Emphasised border. Exactly [`BORDER_FOCUS`] under the stock palette.
pub(crate) fn focus_border_color(ui: &egui::Ui) -> egui::Color32 {
    if stock_profile(ui) {
        BORDER_FOCUS
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    }
}

pub(crate) fn border(ui: &egui::Ui) -> egui::Stroke {
    egui::Stroke::new(1.0_f32, border_color(ui))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_helpers_are_the_stock_constants_under_the_stock_palette() {
        let context = egui::Context::default();
        apply(&context);
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                assert_eq!(card_fill(ui), CARD_SURFACE);
                assert_eq!(raised_fill(ui), RAISED_SURFACE);
                assert_eq!(deep_fill(ui), DEEP_BACKGROUND);
                assert_eq!(border_color(ui), BORDER_SUBTLE);
                assert_eq!(focus_border_color(ui), BORDER_FOCUS);
                assert_eq!(border(ui), egui::Stroke::new(1.0_f32, BORDER_SUBTLE));
            });
        });
    }

    fn with_blue_ui(check: impl FnOnce(&mut egui::Ui)) {
        let context = egui::Context::default();
        apply(&context);
        context.style_mut(|style| style.visuals.faint_bg_color = blue::CARD_SURFACE);
        let mut check = Some(check);
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                if let Some(check) = check.take() {
                    check(ui);
                }
            });
        });
    }

    #[test]
    fn blue_profile_is_detected_only_from_the_blue_card_surface() {
        let context = egui::Context::default();
        apply(&context);
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                assert!(!blue_profile(ui));
                assert_eq!(primary_action_fill(ui), PRIMARY_ACTION);
            });
        });
        with_blue_ui(|ui| {
            assert!(blue_profile(ui));
            assert_eq!(primary_action_fill(ui), blue::PRIMARY_ACTION);
        });
    }

    #[test]
    fn blue_focus_ring_is_drawn_once_for_a_focused_custom_widget() {
        with_blue_ui(|ui| {
            let response = ui.button("Focus me");
            assert!(!focus_ring_painted(ui.ctx(), response.id));
            // Not focused: nothing recorded.
            paint_focus_ring(ui, &response, response.rect, 10.0);
            assert!(!focus_ring_painted(ui.ctx(), response.id));
            response.request_focus();
            let response = ui.ctx().read_response(response.id).unwrap();
            paint_focus_ring(ui, &response, response.rect, 10.0);
            assert!(focus_ring_painted(ui.ctx(), response.id));
        });
    }

    #[test]
    fn blue_status_tones_keep_their_meaning() {
        use crate::ui::components::StatusTone;
        with_blue_ui(|ui| {
            assert_eq!(StatusTone::Success.color(ui), blue::OK);
            assert_eq!(StatusTone::Warning.color(ui), blue::WARN);
            assert_eq!(StatusTone::Blocked.color(ui), blue::BAD);
            // Red only for real failures; purple never a status.
            for tone in [
                StatusTone::Success,
                StatusTone::Warning,
                StatusTone::Pending,
                StatusTone::Active,
                StatusTone::Info,
            ] {
                assert_ne!(tone.color(ui), blue::BAD);
                assert_ne!(tone.color(ui), blue::UNDO);
            }
        });
    }

    #[test]
    fn a_disabled_action_shows_its_reason_and_cannot_be_clicked() {
        with_blue_ui(|ui| {
            let response = crate::ui::components::disabled_action_with_reason(
                ui,
                "Restore",
                "Not available yet: restore is not supported for this emulator.",
            );
            assert!(!response.enabled());
            assert!(!response.clicked());
        });
    }

    #[test]
    fn safety_tags_carry_words_not_only_colour() {
        use crate::ui::components::SafetyTag;
        assert_eq!(SafetyTag::ReadOnly.label(), "Read-only");
        assert_eq!(SafetyTag::AsksFirst.label(), "Asks first");
        assert_eq!(
            SafetyTag::RequiresConfirmation.label(),
            "Requires confirmation"
        );
        with_blue_ui(|ui| {
            assert_eq!(SafetyTag::ReadOnly.color(ui), blue::ACCENT);
            assert_eq!(SafetyTag::AsksFirst.color(ui), blue::WARN);
        });
    }

    #[test]
    fn semantic_palette_keeps_roles_distinct() {
        assert_eq!(ACCENT, PRIMARY_ACTION);
        assert_ne!(ACCENT, AMBER);
        assert_ne!(TEAL, AMBER);
        assert_ne!(SUCCESS, DANGER);
        assert_ne!(PRIMARY_TEXT, TECHNICAL_TEXT);
    }

    #[test]
    fn primary_blue_keeps_light_action_text_contrastable() {
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
        let contrast =
            (luminance(egui::Color32::WHITE) + 0.05) / (luminance(PRIMARY_ACTION) + 0.05);
        assert!(
            contrast >= 4.5,
            "blue/light-text contrast was {contrast:.2}:1"
        );
    }
}

/// Keyboard-focus ring for custom-painted cards and chips (egui draws one
/// only for its own widgets). Mouse appearance is untouched.
pub(crate) fn paint_focus_ring(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: egui::Rect,
    radius: f32,
) {
    if !response.has_focus() {
        return;
    }
    if blue_profile(ui) {
        paint_blue_focus_ring(&ui.ctx().layer_painter(response.layer_id), rect, radius);
        mark_focus_ring_painted(ui.ctx(), response.id);
    } else {
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(2.5_f32, TEAL),
            egui::StrokeKind::Inside,
        );
    }
}

/// The Blue focus ring around `rect`: a 3 px gap, then 3 px of accent.
pub(crate) fn paint_blue_focus_ring(painter: &egui::Painter, rect: egui::Rect, radius: f32) {
    let offset = blue::FOCUS_RING_GAP;
    painter.rect_stroke(
        rect.expand(offset),
        radius + offset,
        egui::Stroke::new(blue::FOCUS_RING_WIDTH, blue::ACCENT),
        egui::StrokeKind::Outside,
    );
}

fn focus_ring_marker() -> egui::Id {
    egui::Id::new("emuwiz-blue-focus-ring-painted")
}

/// Records that a custom widget already drew its own ring this pass, so the
/// shell's generic ring for egui's own widgets does not draw a second one.
fn mark_focus_ring_painted(context: &egui::Context, id: egui::Id) {
    let pass = context.cumulative_pass_nr();
    context.data_mut(|data| data.insert_temp(focus_ring_marker(), (id, pass)));
}

/// Whether [`paint_focus_ring`] already drew a ring for `id` in this pass.
pub(crate) fn focus_ring_painted(context: &egui::Context, id: egui::Id) -> bool {
    let pass = context.cumulative_pass_nr();
    context
        .data(|data| data.get_temp::<(egui::Id, u64)>(focus_ring_marker()))
        .is_some_and(|marked| marked == (id, pass))
}
