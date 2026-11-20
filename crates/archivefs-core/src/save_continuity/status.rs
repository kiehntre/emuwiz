//! What is, and is not, known about a snapshot.
//!
//! Three separate questions, kept in three separate fields, because a "yes"
//! to one says nothing about the others:
//!
//! 1. **Capture safety** ([`super::store::CaptureBoundary`]): was the copy
//!    taken while nothing could be writing the save? This is about the
//!    *copy*: it matches the file on disk exactly.
//! 2. **Structure** ([`StructureCheck`]): does the file have the shape its
//!    format requires (a memory card both readers can walk, a GameCube save
//!    of the right length)? Only where EmuWiz has such a check.
//! 3. **Game-level integrity** ([`GameIntegrity`]): is the save's own data
//!    valid *as the game sees it* (its internal checksum)? EmuWiz has no such
//!    check for any game today, and says so on every snapshot.
//!
//! And one piece of context:
//!
//! 4. **How the emulator last ended** ([`PrecedingSession`]). A crash or a
//!    forced stop in the middle of saving leaves a damaged file on disk, and
//!    a perfectly safe copy of a damaged file is still damaged. This is known
//!    only when a recorded session can be tied to this exact save; otherwise
//!    it is [`PrecedingSession::Unknown`].
//!
//! Nothing here ever says "known good". The strongest statement available is
//! "copied safely, structure checked, last session ended normally, game data
//! not checked".

use serde::{Deserialize, Serialize};

/// Whether the file has the shape its format requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum StructureCheck {
    /// A structural check exists for this format and passed. `what` says
    /// exactly what was checked, so it cannot be read as more.
    Passed { what: String },
    /// A structural check exists and did not pass. The snapshot is still
    /// kept (it may be the only copy), clearly marked.
    Problems { what: String },
    /// EmuWiz has no structural check for this kind of save.
    NotChecked { why: String },
}

/// Whether the save's own data is valid as the game sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum GameIntegrity {
    /// The only value EmuWiz produces today. There is deliberately no
    /// "verified" variant: none can be earned yet.
    NotChecked { why: String },
}

impl GameIntegrity {
    pub fn not_checked() -> Self {
        Self::NotChecked {
            why: "EmuWiz does not check any game's own save checksum; a save can be copied perfectly and still not load".to_string(),
        }
    }
}

/// How the emulator ended the last time it could have written this save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum PrecedingSession {
    /// The emulator itself exited with status 0.
    CleanExit { session_id: String },
    /// It died on a fault (segmentation fault, abort, bus error...).
    Crashed { session_id: String, signal: i32 },
    /// It was stopped from outside (killed or terminated) rather than closed.
    ForcedStop { session_id: String, signal: i32 },
    /// It exited by itself with an error status.
    ErrorExit { session_id: String, code: i32 },
    /// A session was running and EmuWiz did not see it end (EmuWiz was
    /// closed, the machine shut down), or only a launcher's exit was seen.
    ExitNotSeen { session_id: String, detail: String },
    /// No recorded session can be tied to this save with certainty.
    Unknown { why: String },
}

/// How loudly a snapshot's context should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Advisory {
    /// The last session that could have written the save ended normally.
    None,
    /// How the emulator last ended is not known.
    Caution,
    /// The emulator crashed, was stopped, exited with an error, or its exit
    /// was not seen. The save may be incomplete.
    Warning,
}

impl PrecedingSession {
    pub fn unknown(why: &str) -> Self {
        Self::Unknown {
            why: why.to_string(),
        }
    }

    pub fn advisory(&self) -> Advisory {
        match self {
            Self::CleanExit { .. } => Advisory::None,
            Self::Unknown { .. } => Advisory::Caution,
            Self::Crashed { .. }
            | Self::ForcedStop { .. }
            | Self::ErrorExit { .. }
            | Self::ExitNotSeen { .. } => Advisory::Warning,
        }
    }

    /// The emulator's last exit was a normal one, as far as can be known.
    pub fn clean(&self) -> bool {
        matches!(self, Self::CleanExit { .. })
    }

    /// One sentence for a person. `None` only after a clean exit.
    pub fn warning(&self) -> Option<String> {
        Some(match self {
            Self::CleanExit { .. } => return None,
            Self::Crashed { signal, .. } => format!(
                "WARNING: the emulator crashed (signal {signal}) in the session before this snapshot. The save may have been half-written; this is an exact copy of whatever was on disk"
            ),
            Self::ForcedStop { signal, .. } => format!(
                "WARNING: the emulator was stopped from outside (signal {signal}) in the session before this snapshot, not closed normally. The save may be incomplete"
            ),
            Self::ErrorExit { code, .. } => format!(
                "WARNING: the emulator exited with an error (code {code}) in the session before this snapshot. The save may be incomplete"
            ),
            Self::ExitNotSeen { detail, .. } => format!(
                "WARNING: how the emulator ended before this snapshot was not seen ({detail}). The save may be incomplete"
            ),
            Self::Unknown { why } => format!(
                "Not known how the emulator last ended: {why}. Nothing suggests a problem, and nothing rules one out"
            ),
        })
    }
}

/// Signals that mean the program itself failed.
const CRASH_SIGNALS: [i32; 6] = [
    4,  // SIGILL
    6,  // SIGABRT
    7,  // SIGBUS
    8,  // SIGFPE
    11, // SIGSEGV
    31, // SIGSYS
];

/// Sorts a terminating signal into "crashed" or "stopped from outside".
pub fn is_crash_signal(signal: i32, core_dumped: bool) -> bool {
    core_dumped || CRASH_SIGNALS.contains(&signal)
}

/// The three statuses and the context, as they are shown together. There is
/// no combined field on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusSummary {
    pub capture_safety: String,
    pub structure: String,
    pub game_integrity: String,
    pub preceding_session: String,
    pub advisory: Advisory,
}

pub fn describe_structure(structure: Option<&StructureCheck>) -> String {
    match structure {
        Some(StructureCheck::Passed { what }) => format!("Structure checked: {what}"),
        Some(StructureCheck::Problems { what }) => format!("STRUCTURE PROBLEMS: {what}"),
        Some(StructureCheck::NotChecked { why }) => format!("Structure not checked: {why}"),
        None => "Structure: not recorded for this snapshot".to_string(),
    }
}

pub fn describe_integrity(integrity: Option<&GameIntegrity>) -> String {
    match integrity {
        Some(GameIntegrity::NotChecked { why }) => format!("Game data not checked: {why}"),
        None => "Game data not checked".to_string(),
    }
}

pub fn describe_preceding(preceding: Option<&PrecedingSession>) -> String {
    match preceding {
        Some(PrecedingSession::CleanExit { .. }) => {
            "The emulator closed normally in the session before this snapshot".to_string()
        }
        Some(other) => other.warning().unwrap_or_default(),
        None => "How the emulator last ended was not recorded for this snapshot".to_string(),
    }
}
