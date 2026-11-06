//! Whether a Ready projection has a launch request that can actually be built.
//!
//! The projector reads the launch plan, which can say Ready for a candidate
//! whose adapter has no Play route (Mesen, Snes9x, FBNeo, Fuse...) or whose
//! request cannot be assembled (media the route does not accept). This module
//! asks the one authority Play already uses,
//! [`launch_readiness_page::gamer_play_action`], so no second compatibility
//! table exists. A constructible request is "ready to attempt"; it proves
//! nothing about the emulator running the game.
//!
//! Building a request can stat files, re-find emulator profiles and, for a
//! few adapters, hash a cartridge, so it never runs on the GUI thread. A
//! single worker computes it for the selected game; the answer is kept under
//! a key (see [`Key`]) and, until a result for the *current* key arrives, the
//! game reads as Checking, never as a remembered Ready. A cached answer only
//! changes what is displayed: Play still routes to the Launch page, which
//! builds a fresh request and whose preflight rechecks files, identity and
//! configuration at click time.

use super::{
    GameReadinessSummary, ReadinessAction, ReadinessPresentationState, human_emulator_name,
    selected_candidate,
};
use crate::launch_readiness_page::{
    GamerBlocker, GamerBlockerKind, GamerPlayAction, LaunchReadinessInput,
    NO_SAFE_CANDIDATE_DETAIL, gamer_play_action,
};
use archivefs_core::launch::{LaunchCandidate, LaunchContainerKind, LaunchPlan, LaunchTarget};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::SystemTime;

/// Launch routes that are offered by their own Launch-page panel with their
/// own request, preflight and state, not through `gamer_play_action`. Their
/// readiness is left exactly as the planner reported it.
const PANEL_ROUTED_ADAPTERS: [&str; 2] = ["tsugaru", "amiberry_cd"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RouteVerdict {
    /// A launch request can be built. Says nothing about gameplay.
    Constructible,
    /// Not a launch route; the reason is ready to show.
    Refused(RouteRefusal),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RouteRefusal {
    status: ReadinessPresentationState,
    headline: String,
    explanation: String,
    action: ReadinessAction,
}

/// Ask the Play authority. Not cached; see [`RouteCache`].
pub(crate) fn assess(input: &LaunchReadinessInput) -> RouteVerdict {
    let LaunchReadinessInput::Plan { plan, .. } = input else {
        return RouteVerdict::Constructible;
    };
    let Some(candidate) = selected_candidate(plan).or_else(|| plan.candidates.first()) else {
        return RouteVerdict::Constructible;
    };
    if let LaunchTarget::Standalone { adapter_id, .. } = &candidate.target
        && PANEL_ROUTED_ADAPTERS.contains(adapter_id)
    {
        return RouteVerdict::Constructible;
    }
    match gamer_play_action(input) {
        GamerPlayAction::Launch(_) => RouteVerdict::Constructible,
        GamerPlayAction::BlockedTyped(blocker) => {
            RouteVerdict::Refused(refusal_for(plan, candidate, blocker))
        }
    }
}

fn refusal_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    blocker: GamerBlocker,
) -> RouteRefusal {
    let name = match &candidate.target {
        LaunchTarget::Standalone { adapter_id, .. } => human_emulator_name(adapter_id),
        LaunchTarget::RetroArchCore { .. } => "RetroArch".to_string(),
    };
    let refuse = |status, headline: String, explanation: String, action| RouteRefusal {
        status,
        headline,
        explanation,
        action,
    };
    match blocker.kind {
        GamerBlockerKind::CheckingGame | GamerBlockerKind::EmulatorNotChecked => refuse(
            ReadinessPresentationState::Checking,
            "Still checking whether this can start".into(),
            blocker.detail,
            ReadinessAction::RecheckReadiness,
        ),
        GamerBlockerKind::MultipleChoices => refuse(
            ReadinessPresentationState::NeedsEmulator,
            "Choose how to play this game".into(),
            blocker.detail,
            ReadinessAction::ChooseEmulator,
        ),
        GamerBlockerKind::EmulatorNotInstalled | GamerBlockerKind::EmulatorSetupIncomplete => {
            let name = blocker.emulator.unwrap_or(name);
            refuse(
                ReadinessPresentationState::NeedsEmulator,
                format!("{name} needs setting up"),
                blocker.detail,
                ReadinessAction::SetUpEmulator,
            )
        }
        // Everything else: the Ready candidate has no request. Say which of
        // "media this route cannot take" and "no route yet" it is, keeping a
        // specific reason from the shared authority when it gave one.
        _ => {
            let specific = (blocker.detail != NO_SAFE_CANDIDATE_DETAIL).then_some(blocker.detail);
            // Nearly every request builder (SameBoy, mGBA, RMG, Stella,
            // DeSmuME, Dolphin, PCSX2, RetroArch, the generic standalone
            // builder...) refuses a candidate that carries a warning, so a
            // route exists but is held back. Say that, with the warning,
            // rather than claiming there is no route.
            let notes: Vec<&str> = candidate
                .warnings
                .iter()
                .map(|warning| warning.detail.as_str())
                .collect();
            if !notes.is_empty() {
                return refuse(
                    ReadinessPresentationState::Blocked,
                    format!("{name} is held back by a warning"),
                    specific.unwrap_or_else(|| {
                        format!(
                            "EmuWiz only starts {name} from a clean check, and this one has a \
                             note: {}. Resolve it, or choose another emulator. Nothing was \
                             launched.",
                            notes.join("; ")
                        )
                    }),
                    ReadinessAction::ReviewProblem,
                );
            }
            if let Some(media) = unsupported_media(candidate) {
                return refuse(
                    ReadinessPresentationState::Blocked,
                    format!("{name} cannot start this kind of file"),
                    specific.unwrap_or_else(|| {
                        format!("{name} needs a plain game file, but this game is {media}.")
                    }),
                    ReadinessAction::ReviewProblem,
                );
            }
            // An explicit alternative is offered, never substituted.
            let another = plan.candidates.iter().any(|other| {
                other.target != candidate.target
                    && other.blockers.is_empty()
                    && other.readiness != archivefs_core::launch::LaunchReadiness::Blocked
            });
            refuse(
                ReadinessPresentationState::Blocked,
                format!("No Play route for {name} yet"),
                specific.unwrap_or_else(|| {
                    format!(
                        "EmuWiz recognises this game and {name} is set up, but it cannot start \
                         {name} from here yet. Nothing was launched, and no other emulator is \
                         used instead."
                    )
                }),
                if another {
                    ReadinessAction::ChooseEmulator
                } else {
                    ReadinessAction::ReviewProblem
                },
            )
        }
    }
}

fn unsupported_media(candidate: &LaunchCandidate) -> Option<&'static str> {
    if candidate.content.requires_mount {
        return Some("inside a container that must be mounted first");
    }
    match candidate.content.container {
        Some(LaunchContainerKind::PlainFile) | None => None,
        Some(LaunchContainerKind::Archive) => Some("inside an archive"),
        Some(LaunchContainerKind::Chd) => Some("a CHD image"),
        Some(LaunchContainerKind::CueBin) => Some("a CUE/BIN image"),
        Some(LaunchContainerKind::Unknown) => Some("in a format EmuWiz does not recognise"),
    }
}

/// A Ready answer is not Ready unless a launch request can be built.
/// Only Ready projections that offer Play are touched.
pub(crate) fn apply(summary: &mut GameReadinessSummary, verdict: &RouteVerdict) {
    let RouteVerdict::Refused(refusal) = verdict else {
        return;
    };
    if !matches!(
        summary.status,
        ReadinessPresentationState::Ready | ReadinessPresentationState::ReadyWithWarnings
    ) || summary.primary_action != Some(ReadinessAction::Play)
    {
        return;
    }
    summary.status = refusal.status;
    summary.headline = refusal.headline.clone();
    summary.explanation = refusal.explanation.clone();
    summary.primary_action = Some(refusal.action);
    summary.warnings.clear();
    summary
        .details
        .push("A Ready plan is not enough: EmuWiz could not build a launch request for it.".into());
}

/// Pending summary: Play is not offered until the current answer is in.
pub(crate) fn apply_pending(summary: &mut GameReadinessSummary) {
    if !matches!(
        summary.status,
        ReadinessPresentationState::Ready | ReadinessPresentationState::ReadyWithWarnings
    ) || summary.primary_action != Some(ReadinessAction::Play)
    {
        return;
    }
    summary.status = ReadinessPresentationState::Checking;
    summary.headline = "Checking that Play can start".into();
    summary.explanation =
        "EmuWiz is confirming it can build a launch request for this game. Nothing is launched by checking."
            .into();
    summary.primary_action = None;
    summary.warnings.clear();
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

/// Everything an answer depends on. Any difference starts a new check.
/// The plan carries the game, selected emulator, profile, media path and
/// every blocker/warning; the stamps cover the media file and the profile
/// file the plan names, which the plan itself cannot show changing.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Key {
    game_id: i64,
    path: PathBuf,
    plan: LaunchPlan,
    media: Option<FileStamp>,
    profile: Option<FileStamp>,
}

impl Key {
    /// Plan compare plus two `stat` calls; no hashing or discovery.
    fn of(game_id: i64, path: &Path, plan: &LaunchPlan) -> Self {
        let candidate = selected_candidate(plan).or_else(|| plan.candidates.first());
        Self {
            game_id,
            path: path.to_path_buf(),
            plan: plan.clone(),
            media: candidate
                .and_then(|c| c.content.resolved_path.as_deref())
                .and_then(stamp),
            profile: candidate
                .and_then(|c| match &c.target {
                    LaunchTarget::Standalone { profile_path, .. } => profile_path.as_deref(),
                    LaunchTarget::RetroArchCore { .. } => None,
                })
                .and_then(stamp),
        }
    }
}

type Assessor = fn(&LaunchReadinessInput) -> RouteVerdict;

/// What the current key's answer is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Answer {
    Pending,
    Known(RouteVerdict),
}

/// The selected game's route answer, computed by at most one worker.
pub(crate) struct RouteCache {
    key: Option<Key>,
    generation: u64,
    verdict: Option<RouteVerdict>,
    in_flight: bool,
    assessor: Assessor,
    tx: Sender<(u64, RouteVerdict)>,
    rx: Receiver<(u64, RouteVerdict)>,
}

impl Default for RouteCache {
    fn default() -> Self {
        Self::with_assessor(assess)
    }
}

impl RouteCache {
    pub(crate) fn with_assessor(assessor: Assessor) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            key: None,
            generation: 0,
            verdict: None,
            in_flight: false,
            assessor,
            tx,
            rx,
        }
    }

    /// Take finished work. A result for an older key is dropped.
    fn drain(&mut self) {
        while let Ok((generation, verdict)) = self.rx.try_recv() {
            self.in_flight = false;
            if generation == self.generation {
                self.verdict = Some(verdict);
            }
        }
    }

    /// Move to the current key, forgetting the answer if it changed.
    fn track(&mut self, key: Key) {
        if self.key.as_ref() != Some(&key) {
            self.generation += 1;
            self.verdict = None;
            self.key = Some(key);
        }
    }

    /// Per-frame entry. Cheap on a repaint with nothing changed (a plan
    /// compare and two `stat`s); starts the worker only when the current key
    /// has no answer and none is running. `repaint` wakes the GUI on result.
    pub(crate) fn poll(
        &mut self,
        game_id: i64,
        path: &Path,
        input: &LaunchReadinessInput,
        repaint: Option<&eframe::egui::Context>,
    ) -> Answer {
        let LaunchReadinessInput::Plan { plan, .. } = input else {
            return Answer::Known(RouteVerdict::Constructible);
        };
        self.drain();
        self.track(Key::of(game_id, path, plan));
        if let Some(verdict) = &self.verdict {
            return Answer::Known(verdict.clone());
        }
        if !self.in_flight {
            self.in_flight = true;
            let (generation, assessor, tx) = (self.generation, self.assessor, self.tx.clone());
            let input = input.clone();
            let repaint = repaint.cloned();
            std::thread::spawn(move || {
                let _ = tx.send((generation, assessor(&input)));
                if let Some(context) = repaint {
                    context.request_repaint();
                }
            });
        }
        Answer::Pending
    }

    /// The answer for the current key without starting anything.
    pub(crate) fn peek(
        &mut self,
        game_id: i64,
        path: &Path,
        input: &LaunchReadinessInput,
    ) -> Answer {
        let LaunchReadinessInput::Plan { plan, .. } = input else {
            return Answer::Known(RouteVerdict::Constructible);
        };
        self.drain();
        self.track(Key::of(game_id, path, plan));
        self.verdict.clone().map_or(Answer::Pending, Answer::Known)
    }
}

fn stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

#[cfg(test)]
mod tests;
