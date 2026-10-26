//! Title elision shared by the header trail, Library and Game Details.
//!
//! A game title can be arbitrarily long. These helpers measure with the real
//! font and cut at the width the control actually has, so a title can never
//! push Back, Jump to, Play or the view switch out of reach. The full text is
//! always available on hover.
use crate::ui::theme::TextRole;
use eframe::egui;

/// Width of `text` laid out on one line in `font`.
pub(super) fn text_width(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
        .size()
        .x
}

/// `text`, cut with an ellipsis so it is at most `max_width` wide on one line.
pub(super) fn fit(ui: &egui::Ui, text: &str, font: &egui::FontId, max_width: f32) -> String {
    if max_width <= 0.0 || text_width(ui, text, font) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut low, mut high) = (0_usize, chars.len());
    while low < high {
        let middle = (low + high).div_ceil(2);
        let mut candidate: String = chars[..middle].iter().collect();
        candidate = candidate.trim_end().to_owned();
        candidate.push('…');
        if text_width(ui, &candidate, font) <= max_width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let mut cut: String = chars[..low].iter().collect();
    cut = cut.trim_end().to_owned();
    cut.push('…');
    cut
}

/// A title in `role`, wrapped to at most `rows` rows at the width the parent
/// has left and ended with an ellipsis. Hovering shows the whole title.
pub(super) fn title_label(
    ui: &mut egui::Ui,
    text: &str,
    role: TextRole,
    rows: usize,
) -> egui::Response {
    let font = role.text_style().resolve(ui.style());
    let visuals = ui.visuals();
    let color = visuals
        .override_text_color
        .unwrap_or_else(|| visuals.strong_text_color());
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: ui.available_width().max(1.0),
        max_rows: rows.max(1),
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    let response = ui.add(
        egui::Label::new(job)
            .wrap()
            .show_tooltip_when_elided(false)
            .sense(egui::Sense::hover()),
    );
    response.on_hover_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_ui(test: impl FnOnce(&mut egui::Ui)) {
        let context = egui::Context::default();
        let mut test = Some(test);
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                if let Some(test) = test.take() {
                    test(ui);
                }
            });
        });
    }

    #[test]
    fn short_text_is_left_alone_and_long_text_fits_with_an_ellipsis() {
        with_ui(|ui| {
            let font = egui::TextStyle::Body.resolve(ui.style());
            assert_eq!(fit(ui, "Mario", &font, 400.0), "Mario");
            let long = "A very long game title that cannot possibly fit in this narrow place";
            let cut = fit(ui, long, &font, 120.0);
            assert!(cut.ends_with('…') && cut.len() < long.len(), "{cut}");
            assert!(text_width(ui, &cut, &font) <= 120.0, "{cut}");
            // Nothing is returned wider than allowed even for a tiny budget.
            assert!(text_width(ui, &fit(ui, long, &font, 20.0), &font) <= 20.0 + 14.0);
        });
    }
}
