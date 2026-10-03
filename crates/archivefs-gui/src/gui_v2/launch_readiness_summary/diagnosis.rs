//! "Why can't I play this?" - plain-language findings derived only from the
//! blockers, warnings and firmware state the launch plan already carries, plus
//! a small record of the last launch attempt.
//!
//! Nothing here probes the machine, plans a launch or starts a process. Where
//! the plan cannot tell two situations apart (for example an emulator that is
//! installed but not detected versus not installed), this module says so
//! rather than guessing. Known backend evidence gaps are listed in
//! `docs/GUI_V2_LAUNCH_DIAGNOSIS.md`.

use super::ReadinessAction;
use archivefs_core::launch::{
    FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchCandidate, LaunchReadiness,
};
use std::time::{Duration, Instant};

/// What is in the way, in the order a person should deal with it.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Cause {
    GameFileUnavailable,
    DiscSetProblem,
    DiscSetNeedsReview,
    IdentityNotConfirmed,
    EmulatorMissing,
    EmulatorSetupIncomplete,
    FirmwareMissing,
    FormatUnsupported,
    PlatformUnsupported,
    ChoiceNeeded,
    SafetyRefusal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Finding {
    pub(crate) cause: Cause,
    pub(crate) title: &'static str,
    pub(crate) why: &'static str,
    pub(crate) fix: Option<ReadinessAction>,
    /// Raw evidence for the Details disclosure only.
    pub(crate) technical: Vec<String>,
}

fn classify(kind: LaunchBlockerKind) -> Cause {
    use LaunchBlockerKind::*;
    match kind {
        ContentNotResolved => Cause::GameFileUnavailable,
        MediaTopologyBlocked => Cause::DiscSetProblem,
        MediaTopologyReviewRequired => Cause::DiscSetNeedsReview,
        IdentityUnresolved | IdentityConflict => Cause::IdentityNotConfirmed,
        NoInstallationCandidate => Cause::EmulatorMissing,
        ProfileIneligible
        | CoreMissing
        | RetroArchProfileMissing
        | AmbiguousRetroArchProfile
        | RetroArchCoreMismatch
        | RetroArchExecutableMissing
        | AmbiguousRetroArchExecutable
        | RetroArchPathNotExact => Cause::EmulatorSetupIncomplete,
        AmbiguousCore => Cause::ChoiceNeeded,
        RequiredFirmwareMissing => Cause::FirmwareMissing,
        other => classify_adapter_blocker(&format!("{other:?}")),
    }
}

/// The per-adapter blockers (about a hundred) follow a consistent naming
/// scheme, so they are grouped by what they mean instead of being listed one
/// by one. Anything unrecognised becomes a safety refusal: EmuWiz declined to
/// build a launch and says so, instead of inventing a reason.
fn classify_adapter_blocker(name: &str) -> Cause {
    let has = |part: &str| name.contains(part);
    if has("Firmware") || has("Bios") || has("BootRom") || has("Keys") {
        Cause::FirmwareMissing
    } else if has("ContentFormatUnsupported") || has("FormatUnsupported") {
        Cause::FormatUnsupported
    } else if has("PlatformMismatch") {
        Cause::PlatformUnsupported
    } else if has("BindingUnavailable")
        || has("ProfileUnavailable")
        || has("ExecutableMissing")
        || has("ExecutableUnavailable")
    {
        Cause::EmulatorSetupIncomplete
    } else if has("SerialMissing") || has("IdMissing") || has("IdentityMissing") {
        Cause::IdentityNotConfirmed
    } else {
        Cause::SafetyRefusal
    }
}

fn describe(cause: Cause) -> (&'static str, &'static str, Option<ReadinessAction>) {
    match cause {
        Cause::GameFileUnavailable => (
            "The game file can't be found",
            "The library remembers this game, but its file or drive can't be reached right now. It may have moved, or its drive may be disconnected.",
            Some(ReadinessAction::CheckGamesFolder),
        ),
        Cause::DiscSetProblem => (
            "There is a problem with this game's disc set",
            "EmuWiz found a problem with the discs or files that make up this game, so it won't choose a starting disc.",
            Some(ReadinessAction::ReviewDiscSet),
        ),
        Cause::DiscSetNeedsReview => (
            "The disc set needs your review",
            "EmuWiz can't safely choose which disc to start from until you review the set.",
            Some(ReadinessAction::ReviewDiscSet),
        ),
        Cause::IdentityNotConfirmed => (
            "This exact game isn't confirmed yet",
            "EmuWiz can't confirm which release this is, and launching needs that to choose safe settings.",
            Some(ReadinessAction::ReviewIdentity),
        ),
        Cause::EmulatorMissing => (
            "No emulator is set up for this system",
            "EmuWiz found no usable emulator for this game. If you already installed one, choose its program in Emulator Setup; EmuWiz can't tell an uninstalled emulator from one it simply didn't detect.",
            Some(ReadinessAction::SetUpEmulator),
        ),
        Cause::EmulatorSetupIncomplete => (
            "The emulator setup is incomplete",
            "An emulator was found, but EmuWiz can't use it safely yet. Its program or settings may have moved or be unsupported.",
            Some(ReadinessAction::SetUpEmulator),
        ),
        Cause::FirmwareMissing => (
            "Console startup software (BIOS / firmware) is needed",
            "This emulator needs the console's own startup software before it can run this game. Add your own legally obtained files in BIOS Setup; a matching filename alone doesn't prove it's the right file.",
            Some(ReadinessAction::CheckFirmware),
        ),
        Cause::FormatUnsupported => (
            "This kind of game file isn't supported for launching yet",
            "The emulator may be able to run it, but EmuWiz can't launch this file format safely. Your file is fine; nothing is wrong with it.",
            Some(ReadinessAction::ReviewProblem),
        ),
        Cause::PlatformUnsupported => (
            "Launching isn't supported for this system yet",
            "EmuWiz can inspect this game, but it doesn't launch this system's games through this emulator.",
            None,
        ),
        Cause::ChoiceNeeded => (
            "More than one emulator could run this",
            "EmuWiz won't guess between them. Choose the one you want.",
            Some(ReadinessAction::ChooseEmulator),
        ),
        Cause::SafetyRefusal => (
            "EmuWiz can't safely prepare this launch",
            "A safety check declined to build a launch for this game. EmuWiz can't determine more than that from what it knows; the technical details list exactly what was refused.",
            Some(ReadinessAction::ReviewProblem),
        ),
    }
}

/// One finding for a cause that is known without a candidate (no emulator
/// at all, or an identity that is not confirmed).
pub(crate) fn single(cause: Cause) -> Vec<Finding> {
    let (title, why, fix) = describe(cause);
    vec![Finding {
        cause,
        title,
        why,
        fix,
        technical: Vec::new(),
    }]
}

fn blocker_line(blocker: &LaunchBlocker) -> String {
    format!("{:?}: {}", blocker.kind, blocker.detail)
}

/// Every distinct thing blocking this candidate, most actionable first.
/// Empty when nothing blocks it.
pub(crate) fn findings_for(candidate: &LaunchCandidate) -> Vec<Finding> {
    let mut by_cause: Vec<(Cause, Vec<String>)> = Vec::new();
    let mut push = |cause: Cause, line: String| {
        if let Some((_, lines)) = by_cause.iter_mut().find(|(c, _)| *c == cause) {
            lines.push(line);
        } else {
            by_cause.push((cause, vec![line]));
        }
    };
    for blocker in &candidate.blockers {
        push(classify(blocker.kind), blocker_line(blocker));
    }
    if candidate.firmware == FirmwareReadiness::Missing
        && candidate.readiness == LaunchReadiness::Blocked
    {
        push(
            Cause::FirmwareMissing,
            "Firmware state: missing".to_string(),
        );
    }
    by_cause.sort_by_key(|(cause, _)| *cause);
    by_cause
        .into_iter()
        .map(|(cause, technical)| {
            let (title, why, fix) = describe(cause);
            Finding {
                cause,
                title,
                why,
                fix,
                technical,
            }
        })
        .collect()
}

/// What happened the last time the person pressed Play.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Attempt {
    /// The launch plan was accepted but the emulator could not be started.
    CouldNotStart {
        cause: StartFailure,
        technical: String,
    },
    /// The emulator started and closed again almost at once.
    EndedQuickly { seconds: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartFailure {
    ProgramMissing,
    PermissionDenied,
    Refused,
    Other,
}

/// Sorts a failed start into a *hint* from the rendered error text. The launch
/// workers currently hand back a rendered error string, not a typed spawn
/// result, so this is a best-effort reading of that text, never a confirmed
/// diagnosis; the UI words it as "may" and keeps the original text in Details.
/// (Backend evidence gap, see the launch-diagnosis doc.)
pub(crate) fn classify_start_failure(technical: &str) -> StartFailure {
    if technical.contains("NotFound") {
        StartFailure::ProgramMissing
    } else if technical.contains("PermissionDenied") {
        StartFailure::PermissionDenied
    } else if technical.contains("Preflight") {
        StartFailure::Refused
    } else {
        StartFailure::Other
    }
}

impl Attempt {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Self::CouldNotStart { .. } => "The emulator could not be started.",
            Self::EndedQuickly { .. } => "The emulator closed shortly after it was started.",
        }
    }

    /// What EmuWiz actually knows, with no cause implied.
    pub(crate) fn why(&self) -> &'static str {
        match self {
            Self::CouldNotStart { .. } => {
                "EmuWiz tried to start the emulator and it did not start."
            }
            Self::EndedQuickly { .. } => {
                "EmuWiz cannot yet tell why it closed. The emulator's own messages or settings may say more."
            }
        }
    }

    /// An optional, hedged pointer taken from the error text. Not authoritative.
    pub(crate) fn hint(&self) -> Option<&'static str> {
        let Self::CouldNotStart { cause, .. } = self else {
            return None;
        };
        match cause {
            StartFailure::ProgramMissing => Some(
                "The error text suggests the emulator's program may not be where EmuWiz expected it; it may have been moved or removed. This is a hint, not a confirmed diagnosis.",
            ),
            StartFailure::PermissionDenied => Some(
                "The error text suggests the system may not have allowed that program to run. This is a hint, not a confirmed diagnosis.",
            ),
            StartFailure::Refused => Some(
                "The error text suggests EmuWiz's final pre-launch check declined to start it. This is a hint, not a confirmed diagnosis.",
            ),
            StartFailure::Other => None,
        }
    }

    pub(crate) fn fix(&self) -> ReadinessAction {
        match self {
            Self::CouldNotStart { .. } => ReadinessAction::SetUpEmulator,
            Self::EndedQuickly { .. } => ReadinessAction::OpenActivity,
        }
    }

    pub(crate) fn technical(&self) -> String {
        match self {
            Self::CouldNotStart { technical, .. } => technical.clone(),
            Self::EndedQuickly { seconds } => format!(
                "Startup observation window: {} s (an EmuWiz display rule, not a backend result). Observed about {seconds} s. No error was reported.",
                QUICK_EXIT.as_secs()
            ),
        }
    }
}

/// Startup observation window: a process that stops sooner than this after
/// starting is shown as "closed shortly after it was started". It is an
/// EmuWiz display rule, not a backend success/failure boundary.
const QUICK_EXIT: Duration = Duration::from_secs(5);

/// Remembers the last launch attempt for the game it belongs to.
#[derive(Debug, Default)]
pub(crate) struct AttemptTracker {
    started: Option<Instant>,
    last: Option<(i64, Attempt)>,
}

impl AttemptTracker {
    pub(crate) fn started(&mut self) {
        self.started = Some(Instant::now());
        self.last = None;
    }

    pub(crate) fn finished(&mut self, game: Option<i64>, failure: Option<String>) {
        let elapsed = self.started.take().map(|at| at.elapsed());
        let Some(game) = game else {
            return;
        };
        self.last = match (failure, elapsed) {
            (Some(technical), _) => Some((
                game,
                Attempt::CouldNotStart {
                    cause: classify_start_failure(&technical),
                    technical,
                },
            )),
            (None, Some(elapsed)) if elapsed < QUICK_EXIT => Some((
                game,
                Attempt::EndedQuickly {
                    seconds: elapsed.as_secs(),
                },
            )),
            _ => None,
        };
    }

    /// Only an attempt for this very game is ever shown.
    pub(crate) fn for_game(&self, game: i64) -> Option<&Attempt> {
        self.last
            .as_ref()
            .filter(|(owner, _)| *owner == game)
            .map(|(_, attempt)| attempt)
    }
}
