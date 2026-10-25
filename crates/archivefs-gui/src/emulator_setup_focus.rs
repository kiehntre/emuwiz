//! One-shot navigation hint for the Emulator Setup page.
//!
//! When a person arrives at Emulator Setup *because* one specific
//! emulator's repair is what they came for - from Gamer View's `NeedsSetup`
//! card - this records which repair target the page should present first.
//! `ArchiveFsApp` consumes it with `Option::take`, so it fires exactly once.
//!
//! This is a navigation hint, not an emulator registry. Reaching Emulator
//! Setup through the sidebar or Home leaves the hint `None`.

/// Which repair card `show_emulator_setup_page` should bring into view once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EmulatorSetupFocus {
    /// The RetroArch core-folder repair card.
    RetroArch,
    /// A standalone emulator named by a launch blocker. The setup page uses
    /// this to preserve which emulator the user was trying to repair, even
    /// though its shared Doctor summary remains the source of truth.
    Emulator(String),
}
