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
use std::time::{Duration, Instant, SystemTime};

/// Launch routes that are offered by their own Launch-page panel with their
/// own request, preflight and state, not through `gamer_play_action`. Their
/// readiness is left exactly as the planner reported it.
const PANEL_ROUTED_ADAPTERS: [&str; 2] = ["tsugaru", "amiberry_cd"];

/// Wording only (the verdict itself always comes from `gamer_play_action`):
/// adapters whose request builder accepts a warned candidate, so a warning is
/// never the reason they have no request.
const WARNING_TOLERANT: [&str; 4] = ["azahar", "vita3k", "xroar", "shadps4"];

/// Wording only: adapters with no request builder at all, as opposed to a
/// builder that declined this game.
const NO_BUILDER: [&str; 5] = ["mesen", "snes9x", "hatari", "fbneo", "fuse"];

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
        // "media this route cannot take", "a specific reason from the shared
        // authority", "held back by a warning" and "no Play route" it is.
        _ => {
            let specific = (blocker.detail != NO_SAFE_CANDIDATE_DETAIL).then_some(blocker.detail);
            let adapter = adapter_id(candidate);
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
            // The authority's own reason (multi-disc, sandbox, resolver...)
            // always wins over a guess.
            if let Some(reason) = specific {
                return refuse(
                    ReadinessPresentationState::Blocked,
                    format!("{name} cannot start this game yet"),
                    reason,
                    ReadinessAction::ReviewProblem,
                );
            }
            // Most request builders refuse a candidate that carries a
            // warning, so the route exists but is held back. Builders that
            // accept warnings are excluded: for them a warning is not the
            // reason.
            let notes: Vec<&str> = candidate
                .warnings
                .iter()
                .map(|warning| warning.detail.as_str())
                .collect();
            if !notes.is_empty() && !adapter.is_some_and(|id| WARNING_TOLERANT.contains(&id)) {
                return refuse(
                    ReadinessPresentationState::Blocked,
                    format!("{name} is held back by a warning"),
                    format!(
                        "EmuWiz only starts {name} from a clean check, and this one has a \
                         note: {}. Resolve it, or choose another emulator. Nothing was \
                         launched.",
                        notes.join("; ")
                    ),
                    ReadinessAction::ReviewProblem,
                );
            }
            // An explicit alternative is offered, never substituted.
            let another = plan.candidates.iter().any(|other| {
                other.target != candidate.target
                    && other.blockers.is_empty()
                    && other.readiness != archivefs_core::launch::LaunchReadiness::Blocked
            });
            let action = if another {
                ReadinessAction::ChooseEmulator
            } else {
                ReadinessAction::ReviewProblem
            };
            if adapter.is_some_and(|id| NO_BUILDER.contains(&id)) {
                return refuse(
                    ReadinessPresentationState::Blocked,
                    format!("No Play route for {name} yet"),
                    format!(
                        "EmuWiz recognises this game and {name} is set up, but it cannot start \
                         {name} from here yet. Nothing was launched, and no other emulator is \
                         used instead."
                    ),
                    action,
                );
            }
            refuse(
                ReadinessPresentationState::Blocked,
                format!("{name} cannot start this game yet"),
                format!(
                    "EmuWiz could not build a launch request for {name} from the current \
                     checks. Recheck, or review the emulator setup. Nothing was launched, and \
                     no other emulator is used instead."
                ),
                action,
            )
        }
    }
}

fn adapter_id(candidate: &LaunchCandidate) -> Option<&'static str> {
    match &candidate.target {
        LaunchTarget::Standalone { adapter_id, .. } => Some(*adapter_id),
        LaunchTarget::RetroArchCore { .. } => None,
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

/// Size and modification time of the media and profile files the plan names.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Stamps {
    media: Option<FileStamp>,
    profile: Option<FileStamp>,
}

/// Everything an answer depends on that is cheap to read on the GUI thread.
/// Any difference starts a new check. The plan carries the game, selected
/// emulator, profile, media path and every blocker/warning; `shape` records
/// the scan flags and which emulator contexts exist, which `gamer_play_action`
/// reads but the plan does not hold. File stamps are *not* here: reading them
/// could block on a slow mount, so a worker revalidates them (see
/// [`RouteCache::poll`]).
#[derive(Clone, Debug, Eq, PartialEq)]
struct Key {
    game_id: i64,
    path: PathBuf,
    plan: LaunchPlan,
    shape: u32,
}

impl Key {
    fn of(game_id: i64, path: &Path, input: &LaunchReadinessInput) -> Option<Self> {
        let LaunchReadinessInput::Plan { plan, .. } = input else {
            return None;
        };
        Some(Self {
            game_id,
            path: path.to_path_buf(),
            plan: plan.clone(),
            shape: shape(input),
        })
    }
}

/// Which non-plan inputs exist, as bits: the scan flags and each context.
fn shape(input: &LaunchReadinessInput) -> u32 {
    let LaunchReadinessInput::Plan {
        retroarch,
        retroarch_scanned,
        standalone_scans_complete,
        dolphin,
        pcsx2,
        duckstation,
        ppsspp,
        rpcs3,
        xemu,
        xenia,
        melonds,
        cemu,
        flycast,
        xroar,
        azahar,
        vita3k,
        vice,
        amiga_whdload,
        ..
    } = input
    else {
        return 0;
    };
    let bits = [
        *retroarch_scanned,
        *standalone_scans_complete,
        retroarch.is_some(),
        dolphin.is_some(),
        pcsx2.is_some(),
        duckstation.is_some(),
        ppsspp.is_some(),
        rpcs3.is_some(),
        xemu.is_some(),
        xenia.is_some(),
        melonds.is_some(),
        cemu.is_some(),
        flycast.is_some(),
        xroar.is_some(),
        azahar.is_some(),
        vita3k.is_some(),
        vice.is_some(),
        amiga_whdload.is_some(),
        #[cfg(target_os = "linux")]
        matches!(
            input,
            LaunchReadinessInput::Plan {
                shadps4: Some(_),
                ..
            }
        ),
    ];
    bits.iter()
        .enumerate()
        .fold(0, |acc, (index, bit)| acc | (u32::from(*bit) << index))
}

/// The files whose change makes an answer stale. Worker-side only.
fn stamp_paths(plan: &LaunchPlan) -> (Option<PathBuf>, Option<PathBuf>) {
    let candidate = selected_candidate(plan).or_else(|| plan.candidates.first());
    (
        candidate.and_then(|c| c.content.resolved_path.clone()),
        candidate.and_then(|c| match &c.target {
            LaunchTarget::Standalone { profile_path, .. } => profile_path.clone(),
            LaunchTarget::RetroArchCore { .. } => None,
        }),
    )
}

fn stamps_of(paths: &(Option<PathBuf>, Option<PathBuf>)) -> Stamps {
    Stamps {
        media: paths.0.as_deref().and_then(stamp),
        profile: paths.1.as_deref().and_then(stamp),
    }
}

fn stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// When a worker panics the game must not read Checking for ever.
fn failed_check() -> RouteVerdict {
    RouteVerdict::Refused(RouteRefusal {
        status: ReadinessPresentationState::Blocked,
        headline: "Could not check whether Play can start".into(),
        explanation: "The check stopped unexpectedly. Recheck this game. Nothing was launched."
            .into(),
        action: ReadinessAction::RecheckReadiness,
    })
}

type Assessor = fn(&LaunchReadinessInput) -> RouteVerdict;

/// What the current key's answer is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Answer {
    Pending,
    Known(RouteVerdict),
}

/// How often a settled answer's files are looked at again, and how soon an
/// unfinished ("still checking") answer is asked again.
const REVALIDATE: Duration = Duration::from_secs(3);

enum Done {
    /// A finished assessment and the file stamps seen when it ran.
    Verdict(RouteVerdict, Stamps),
    /// The files' current stamps, from a revalidation.
    Stamps(Stamps),
}

/// The selected game's route answer, computed by at most one worker.
pub(crate) struct RouteCache {
    key: Option<Key>,
    generation: u64,
    verdict: Option<RouteVerdict>,
    stamps: Option<Stamps>,
    /// When the answer was last confirmed current.
    confirmed: Instant,
    in_flight: bool,
    revalidate: Duration,
    assessor: Assessor,
    tx: Sender<(u64, Done)>,
    rx: Receiver<(u64, Done)>,
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
            stamps: None,
            confirmed: Instant::now(),
            in_flight: false,
            revalidate: REVALIDATE,
            assessor,
            tx,
            rx,
        }
    }

    #[cfg(test)]
    fn with_revalidate(mut self, interval: Duration) -> Self {
        self.revalidate = interval;
        self
    }

    /// Take finished work. A result for an older key is dropped.
    fn drain(&mut self) {
        while let Ok((generation, done)) = self.rx.try_recv() {
            self.in_flight = false;
            if generation != self.generation {
                continue;
            }
            self.confirmed = Instant::now();
            match done {
                Done::Verdict(verdict, stamps) => {
                    self.verdict = Some(verdict);
                    self.stamps = Some(stamps);
                }
                Done::Stamps(now) => {
                    // A changed file makes the answer stale: ask again.
                    if self.stamps.as_ref() != Some(&now) {
                        self.verdict = None;
                        self.stamps = None;
                    }
                }
            }
        }
    }

    /// Move to the current key, forgetting the answer if it changed.
    fn track(&mut self, key: Key) {
        if self.key.as_ref() != Some(&key) {
            self.generation += 1;
            self.verdict = None;
            self.stamps = None;
            self.key = Some(key);
        }
    }

    /// An unfinished answer ("still checking") is never final.
    fn unfinished(&self) -> bool {
        matches!(
            &self.verdict,
            Some(RouteVerdict::Refused(refusal))
                if refusal.status == ReadinessPresentationState::Checking
        )
    }

    /// Per-frame entry. A repaint with nothing changed costs a plan compare;
    /// no file access happens on this thread. The worker is started only when
    /// the current key has no answer, when an unfinished answer is due again,
    /// or when a settled answer's files are due for a look; at most one runs.
    /// `repaint` wakes the GUI on result.
    pub(crate) fn poll(
        &mut self,
        game_id: i64,
        path: &Path,
        input: &LaunchReadinessInput,
        repaint: Option<&eframe::egui::Context>,
    ) -> Answer {
        let Some(key) = Key::of(game_id, path, input) else {
            return Answer::Known(RouteVerdict::Constructible);
        };
        self.drain();
        self.track(key);
        let due = self.confirmed.elapsed() >= self.revalidate;
        if self.unfinished()
            && let Some(context) = repaint
        {
            // Nothing else may repaint while the card says Checking.
            context.request_repaint_after(self.revalidate);
        }
        if self.unfinished() && due {
            self.verdict = None;
        }
        if !self.in_flight {
            let LaunchReadinessInput::Plan { plan, .. } = input else {
                unreachable!("Key::of only accepts plans")
            };
            let (generation, tx) = (self.generation, self.tx.clone());
            let repaint = repaint.cloned();
            if self.verdict.is_none() {
                self.in_flight = true;
                let (assessor, input) = (self.assessor, input.clone());
                let paths = stamp_paths(plan);
                std::thread::spawn(move || {
                    let stamps = stamps_of(&paths);
                    let verdict =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assessor(&input)))
                            .unwrap_or_else(|_| failed_check());
                    let _ = tx.send((generation, Done::Verdict(verdict, stamps)));
                    if let Some(context) = repaint {
                        context.request_repaint();
                    }
                });
            } else if due {
                self.in_flight = true;
                let paths = stamp_paths(plan);
                std::thread::spawn(move || {
                    let _ = tx.send((generation, Done::Stamps(stamps_of(&paths))));
                    if let Some(context) = repaint {
                        context.request_repaint();
                    }
                });
            }
        }
        self.verdict.clone().map_or(Answer::Pending, Answer::Known)
    }

    /// The answer for the current key without starting anything.
    pub(crate) fn peek(
        &mut self,
        game_id: i64,
        path: &Path,
        input: &LaunchReadinessInput,
    ) -> Answer {
        let Some(key) = Key::of(game_id, path, input) else {
            return Answer::Known(RouteVerdict::Constructible);
        };
        self.drain();
        self.track(key);
        self.verdict.clone().map_or(Answer::Pending, Answer::Known)
    }
}

#[cfg(test)]
mod tests;
