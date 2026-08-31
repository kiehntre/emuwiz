//! One-shot navigation hint for the Emulator Setup page.
//!
//! When a person arrives at Emulator Setup *because* one specific
//! emulator's repair is what they came for - today only from Gamer View's
//! `NeedsSetup` card - this records which repair card to scroll into view
//! on the first render. `ArchiveFsApp` consumes it with `Option::take`, so
//! it fires exactly once: the first frame may scroll, every later frame
//! leaves the scroll position alone and the user can scroll freely.
//!
//! This is a navigation hint, not an emulator registry. There is one
//! variant because there is one caller; add another only when a real
//! caller needs it. Reaching Emulator Setup through the sidebar or Home
//! leaves the hint `None` and nothing scrolls.

/// Which repair card `show_emulator_setup_page` should bring into view once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmulatorSetupFocus {
    /// The RetroArch core-folder repair card. Gamer View's blocked-launch
    /// path is structurally RetroArch-only: `gamer_readiness` yields
    /// `NeedsSetup` when the shared launch plan has no safe RetroArch core
    /// candidate for the game's platform (see `launch_readiness_page`), and
    /// no other emulator feeds that state - so the "Open Emulator Setup"
    /// action always means "take me to the RetroArch repair".
    RetroArch,
}
