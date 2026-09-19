//! Task-first Simple Mode, with retained Gamer and Advanced views.
//!
//! One enum, one default, one on-disk preference. `GuiMode` is the identity
//! the running application switches on; `gui_mode.txt` is the only place it
//! is persisted, holding `simple`, `gamer`, or `advanced`. Existing explicit
//! preferences are preserved; fresh or unreadable preferences use Simple Mode.

use std::path::PathBuf;

/// Simple Mode is the novice default; the older two modes remain available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GuiMode {
    #[default]
    Simple,
    GamerView,
    AdvancedView,
}

/// A dedicated on-disk preference file, following the same precedent the
/// design (§1.1) points to: `~/.config/archivefs/emulator_profiles.toml`
/// is its own small file rather than a new `Config`/`config.toml` field,
/// specifically to avoid coupling unrelated persistence together. Mode is
/// a GUI-layer-only concept (never read by `archivefs-core` or the CLI),
/// so it lives in the GUI crate rather than in core.
fn gui_mode_config_path() -> Option<PathBuf> {
    archivefs_core::app_dirs::config_path("gui_mode.txt").ok()
}

fn parse_gui_mode(contents: &str) -> GuiMode {
    match contents.trim() {
        "advanced" => GuiMode::AdvancedView,
        "gamer" => GuiMode::GamerView,
        _ => GuiMode::Simple,
    }
}

fn gui_mode_file_contents(mode: GuiMode) -> &'static str {
    match mode {
        GuiMode::Simple => "simple",
        GuiMode::GamerView => "gamer",
        GuiMode::AdvancedView => "advanced",
    }
}

/// A missing or unreadable file means "nothing chosen yet" - falls back
/// to the default (`Simple`), never an error.
pub(crate) fn load_gui_mode() -> GuiMode {
    gui_mode_config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|contents| parse_gui_mode(&contents))
        .unwrap_or_default()
}

/// Best-effort: a failure to persist the chosen mode (e.g. a read-only
/// home directory) never blocks the mode switch itself from taking
/// effect for the rest of the session - it just won't survive a restart.
pub(crate) fn save_gui_mode(mode: GuiMode) {
    let Some(path) = gui_mode_config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, gui_mode_file_contents(mode));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_simple_view() {
        assert_eq!(GuiMode::default(), GuiMode::Simple);
    }

    #[test]
    fn persisted_values_round_trip() {
        for mode in [GuiMode::Simple, GuiMode::GamerView, GuiMode::AdvancedView] {
            assert_eq!(parse_gui_mode(gui_mode_file_contents(mode)), mode);
        }
        assert_eq!(gui_mode_file_contents(GuiMode::GamerView), "gamer");
        assert_eq!(gui_mode_file_contents(GuiMode::AdvancedView), "advanced");
    }

    #[test]
    fn surrounding_whitespace_is_ignored_when_reading_the_file() {
        assert_eq!(parse_gui_mode("  advanced\n"), GuiMode::AdvancedView);
        assert_eq!(parse_gui_mode("\tgamer  "), GuiMode::GamerView);
    }

    #[test]
    fn anything_unrecognised_means_nothing_chosen_yet_not_an_error() {
        // Deliberately lenient, unlike a strict parser: an empty, damaged or
        // future-written file must fall back to the unconditional default
        // rather than fail the launch. Display labels are not accepted as
        // persisted values.
        for contents in [
            "",
            "   ",
            "unknown",
            "Advanced View",
            "Gamer View",
            "ADVANCED",
        ] {
            assert_eq!(parse_gui_mode(contents), GuiMode::Simple);
        }
    }
}
