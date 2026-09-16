use std::path::PathBuf;
use std::{fmt, str::FromStr};

/// The two supported GUI experiences. Rendering and navigation deliberately
/// live elsewhere; this type is only the stable mode identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ViewMode {
    #[default]
    Gamer,
    Advanced,
}

impl ViewMode {
    pub const ALL: [Self; 2] = [Self::Gamer, Self::Advanced];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Gamer => "Gamer View",
            Self::Advanced => "Advanced View",
        }
    }

    /// Stable lower-case value suitable for eframe or config persistence.
    pub const fn persisted(self) -> &'static str {
        match self {
            Self::Gamer => "gamer",
            Self::Advanced => "advanced",
        }
    }

    pub fn from_persisted(value: &str) -> Option<Self> {
        value.parse().ok()
    }
}

impl fmt::Display for ViewMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseViewModeError;

impl fmt::Display for ParseViewModeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected 'gamer' or 'advanced'")
    }
}

impl std::error::Error for ParseViewModeError {}

impl FromStr for ViewMode {
    type Err = ParseViewModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "gamer" => Ok(Self::Gamer),
            "advanced" => Ok(Self::Advanced),
            _ => Err(ParseViewModeError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_gamer_view() {
        assert_eq!(ViewMode::default(), ViewMode::Gamer);
    }

    #[test]
    fn labels_are_plain_and_distinct() {
        assert_eq!(ViewMode::Gamer.label(), "Gamer View");
        assert_eq!(ViewMode::Advanced.label(), "Advanced View");
    }

    #[test]
    fn persisted_values_round_trip() {
        for mode in ViewMode::ALL {
            assert_eq!(ViewMode::from_persisted(mode.persisted()), Some(mode));
        }
        assert_eq!(ViewMode::from_persisted("Gamer View"), None);
        assert_eq!(ViewMode::from_persisted("unknown"), None);
    }
}

/// Decision 5 (docs/GUI_NAVIGATION_RESET_DESIGN.md §9): exactly these two
/// modes, no alternate labels. `GamerView` is the unconditional default
/// for a fresh profile/first launch (decision matches §1.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GuiMode {
    #[default]
    GamerView,
    AdvancedView,
}

/// A dedicated on-disk preference file, following the same precedent the
/// design (§1.1) points to: `~/.config/archivefs/emulator_profiles.toml`
/// is its own small file rather than a new `Config`/`config.toml` field,
/// specifically to avoid coupling unrelated persistence together. Mode is
/// a GUI-layer-only concept (never read by `archivefs-core` or the CLI),
/// so it lives in the GUI crate rather than in core.
pub(crate) fn gui_mode_config_path() -> Option<PathBuf> {
    archivefs_core::app_dirs::config_path("gui_mode.txt").ok()
}

pub(crate) fn parse_gui_mode(contents: &str) -> GuiMode {
    match contents.trim() {
        "advanced" => GuiMode::AdvancedView,
        _ => GuiMode::GamerView,
    }
}

pub(crate) fn gui_mode_file_contents(mode: GuiMode) -> &'static str {
    match mode {
        GuiMode::GamerView => "gamer",
        GuiMode::AdvancedView => "advanced",
    }
}

/// A missing or unreadable file means "nothing chosen yet" - falls back
/// to the unconditional default (`GamerView`), never an error.
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
