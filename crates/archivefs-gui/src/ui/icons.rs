//! The friendly visual language: a small, consistent text icon set.
//!
//! The application uses egui's embedded default fonts; it does not depend on
//! a host emoji font. These therefore use small, monochrome Unicode symbols
//! rather than colourful emoji or fake keyboard-looking ASCII placeholders.
//! The test below verifies every symbol against that actual embedded font
//! stack, so a release build cannot silently turn one into a missing-glyph
//! square on Linux.
//!
//! A larger hand-drawn illustration pass can follow after more beta feedback;
//! this establishes the visual language today without new artwork.

pub(crate) const HOME: &str = "⊞";

// Primary concepts (used on Home cards and the matching page headers).
pub(crate) const GAMES: &str = "■"; // My Games / Library
pub(crate) const ORGANISE: &str = "▪"; // Organise / Canonical Organisation
pub(crate) const CHECK: &str = "○"; // Check Library / Doctor
pub(crate) const CHEATS: &str = "★"; // Cheats & Mods
pub(crate) const VERIFY: &str = "⊞"; // Verify Games / DAT verification
pub(crate) const SETTINGS: &str = "⚙";

// Secondary concepts.
pub(crate) const SOURCES: &str = "⊞";
pub(crate) const MOUNT: &str = "▣";
pub(crate) const ARTWORK: &str = "■";
pub(crate) const HISTORY: &str = "▪";
pub(crate) const RECENT: &str = "○";
pub(crate) const SELECTED: &str = "★";
pub(crate) const ABOUT: &str = "i";
pub(crate) const ROMM: &str = "○";
pub(crate) const CLEAN_UP: &str = "▪";
pub(crate) const SEARCH: &str = "?";

/// The restrained retro cheat-code motif used once (Home or Cheats header) as
/// decoration only - never the primary label.
pub(crate) const CHEAT_CODE: &str = "UP UP DOWN DOWN LEFT RIGHT";

/// `"{glyph} {label}"` - the standard way to put an icon next to a label.
#[must_use]
pub(crate) fn with_icon(glyph: &str, label: &str) -> String {
    format!("{glyph} {label}")
}

/// Whether the embedded egui proportional font stack can render this symbol.
/// This deliberately checks the same non-system font configuration used by
/// the app, rather than assuming a desktop emoji fallback exists.
#[cfg(test)]
use eframe::egui;

#[cfg(test)]
pub(crate) fn is_font_stack_safe(glyph: &str) -> bool {
    let context = egui::Context::default();
    let mut supported = false;
    let _ = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            supported =
                ui.fonts_mut(|fonts| fonts.has_glyphs(&egui::FontId::proportional(16.0), glyph));
        });
    });
    supported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_renderable_by_the_embedded_font_stack() {
        let unsupported: Vec<_> = [
            HOME, GAMES, ORGANISE, CHECK, CHEATS, VERIFY, SETTINGS, SOURCES, MOUNT, ARTWORK,
            HISTORY, RECENT, SELECTED, ABOUT, ROMM, CLEAN_UP, SEARCH,
        ]
        .into_iter()
        .filter(|icon| !is_font_stack_safe(icon))
        .collect();
        assert!(unsupported.is_empty(), "unsupported icons: {unsupported:?}");
    }
}
