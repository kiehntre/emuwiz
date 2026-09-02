//! The Launch Readiness panel on the Selected page.
//!
//! Read-only rendering of an [`archivefs_core::launch::LaunchPlan`] the
//! caller has already built from existing, already-verified evidence via
//! [`archivefs_core::launch::canonical_identity_from_game_report`]/
//! [`archivefs_core::launch::launch_content_ref_from_archive_record`] and
//! [`archivefs_core::launch::build_launch_plan`]. This module never calls
//! any of those itself - see `main.rs`'s `MainView::Selected` branch for
//! where the plan is assembled and handed in as [`LaunchReadinessInput`].
//!
//! # Typed launches
//!
//! Gamer View and the Selected page expose only typed launch requests for
//! candidates whose existing adapter and preflight path can execute them.
//! RetroArch, Dolphin, PCSX2, and the supported standalone adapters each
//! retain their own typed request/state/executor path. The click itself is
//! the user's authorization; preflight and spawn happen on background
//! threads and are polled non-blockingly. This module never builds a shell
//! command or trusts cached readiness as execution authority; each adapter
//! re-validates everything fresh.
//!
//! # What this module is not
//!
//! - It never calls [`archivefs_core::launch::build_retroarch_command_plan`]
//!   or any ES-DE export/write function itself - the exact argv always
//!   comes from core's own fresh preflight, never reconstructed here.
//! - It never resolves identity, mounts an archive, or guesses an inner
//!   archive member - see the module doc comment on
//!   [`archivefs_core::launch::evidence_bridge`] for where those honest
//!   fail-closed rules actually live; this module only renders whatever
//!   that bridge already decided.
//! - It never launches a Flatpak/AppImage candidate, archive/mounted
//!   content, or a `ReadyWithWarnings`/`Blocked` candidate. Standalone
//!   candidates are limited to adapters with an existing typed GUI executor.
//! - It never exposes a Stop/Kill action and never automatically relaunches
//!   a process that has exited.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord;
use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::emulator_environment::retroarch::{
    DiscoveryEnvironment, ProfileKind, ProfileRef,
};
use archivefs_core::launch::{
    CandidatePreference, DOLPHIN_SUPPORTED_PLATFORM_ID, DolphinLaunchExecutionError,
    DolphinLaunchExitReport, DolphinLaunchPreflightErrorKind, DolphinLaunchRequest,
    DolphinLaunchSpawnError, FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchCandidate,
    LaunchContainerKind, LaunchExecutionError, LaunchExitReport, LaunchPlan,
    LaunchPreflightErrorKind, LaunchReadiness, LaunchSpawnError, LaunchTarget, LaunchWarning,
    LaunchWarningKind, LaunchedDolphinProcess, LaunchedPcsx2Process, LaunchedRetroArchProcess,
    PCSX2_SUPPORTED_PLATFORM_ID, Pcsx2LaunchExecutionError, Pcsx2LaunchExitReport,
    Pcsx2LaunchPreflightErrorKind, Pcsx2LaunchRequest, Pcsx2LaunchSpawnError,
    RetroArchLaunchRequest, preflight_and_launch_dolphin, preflight_and_launch_pcsx2,
    preflight_and_launch_retroarch,
};
use archivefs_core::launch::{
    DuckStationLaunchExecutionError, DuckStationLaunchRequest, LaunchedDuckStationProcess,
    LaunchedPpssppProcess, LaunchedRpcs3Process, LaunchedXemuProcess, LaunchedXeniaProcess,
    PpssppLaunchExecutionError, PpssppLaunchRequest, Rpcs3LaunchExecutionError, Rpcs3LaunchRequest,
    XemuLaunchExecutionError, XemuLaunchRequest, XeniaLaunchExecutionError, XeniaLaunchRequest,
    preflight_and_launch_duckstation, preflight_and_launch_ppsspp, preflight_and_launch_rpcs3,
    preflight_and_launch_xemu, preflight_and_launch_xenia,
};
use archivefs_core::patch_manager::{
    DolphinLocalDiscoveryRoots, DolphinLocalProfileDiscovery, Pcsx2ProfileDiscovery,
    Pcsx2ProfileDiscoveryRoots, resolve_dolphin_native_launch_binding,
    resolve_pcsx2_native_launch_binding,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// Everything [`show_launch_readiness_panel`] needs, gathered by the caller
/// from existing App/MainView state before this module ever runs. Every
/// non-[`Self::Plan`] variant is a prerequisite the caller checked *without*
/// calling the planner - see each variant's own doc comment for exactly
/// which check it stands for.
// Built once per frame, borrowed immediately by the panel renderer, then
// dropped; the `Plan` payload is the real launch plan it must carry. The
// size gap vs the marker variants has no bearing on a per-frame temporary.
#[allow(clippy::large_enum_variant)]
pub(crate) enum LaunchReadinessInput {
    /// `SelectedEvidenceState` is not `Ready` for the focused archive yet.
    /// The planner is never called in this state.
    EvidenceNotLoaded,
    /// `RetroArchProfilesState` is not `Ready` - RetroArch profiles/cores
    /// have never been scanned. The planner is never called in this state,
    /// and this panel never triggers a scan itself.
    #[allow(dead_code)]
    RetroArchNotScanned,
    /// `CanonicalIdentityStatus::Unknown` - identity could not be resolved
    /// at all.
    IdentityUnknown,
    /// `CanonicalIdentityStatus::Conflicting` - identity evidence conflicts.
    IdentityConflicting,
    /// Identity was resolved and a real [`LaunchPlan`] was built.
    Plan {
        plan: LaunchPlan,
        /// Whether the RetroArch environment was actually discovered. An
        /// unscanned RetroArch lane must not hide a ready standalone lane.
        retroarch_scanned: bool,
        /// Whether the standalone lane relevant to this platform has
        /// completed its existing discovery check.
        standalone_scans_complete: bool,
        /// The already-discovered Dolphin profile data (and the roots that
        /// discovery ran against) `plan`'s Dolphin standalone candidate, if
        /// any, was built from - `None` while that discovery has not
        /// completed yet, which simply means no Dolphin candidate exists
        /// in `plan` (never blocks the whole panel the way an unscanned
        /// RetroArch environment does; see `main.rs`'s
        /// `build_launch_readiness_input`). Needed here, not fabricated in
        /// this module, to look up the exact profile a Dolphin candidate
        /// names and compute its real launch binding for the "Launch
        /// Dolphin" button - see [`dolphin_launch_request`].
        dolphin: Option<DolphinLaunchContext>,
        /// The already-discovered PCSX2 profile data (roots included) and
        /// the already-resolved PS2 firmware evidence `plan`'s PCSX2
        /// standalone candidate, if any, was built from - `None` while
        /// discovery has not completed yet, the same additive-not-blocking
        /// shape as `dolphin` above. Needed here, not fabricated in this
        /// module, to look up the exact profile a PCSX2 candidate names,
        /// compute its real launch binding, and pass real firmware
        /// evidence through to core's own fresh BIOS verification - see
        /// [`pcsx2_launch_request`].
        pcsx2: Option<Pcsx2LaunchContext>,
        duckstation: Option<DuckStationLaunchContext>,
        ppsspp: Option<PpssppLaunchContext>,
        rpcs3: Option<Rpcs3LaunchContext>,
        xemu: Option<XemuLaunchContext>,
        xenia: Option<XeniaLaunchContext>,
    },
}

/// One exact adapter request selected by the shared launch plan. Gamer View
/// carries this typed value to the existing executor; it never constructs a
/// shell command or re-selects an emulator at click time.
#[derive(Clone)]
pub(crate) enum TypedLaunchRequest {
    RetroArch(RetroArchLaunchRequest),
    Dolphin(DolphinLaunchRequest),
    Pcsx2(Pcsx2LaunchRequest, Vec<FirmwareIdentityRecord>),
    Standalone(StandaloneLaunchRequest),
}

impl TypedLaunchRequest {
    pub(crate) fn adapter_name(&self) -> &'static str {
        match self {
            Self::RetroArch(_) => "RetroArch",
            Self::Dolphin(_) => "Dolphin",
            Self::Pcsx2(_, _) => "PCSX2",
            Self::Standalone(request) => request.adapter_name(),
        }
    }

    pub(crate) fn start(
        self,
        retroarch: &mut RetroArchLaunchState,
        dolphin: &mut DolphinLaunchState,
        pcsx2: &mut Pcsx2LaunchState,
        standalone: &mut StandaloneLaunchState,
    ) {
        match self {
            Self::RetroArch(request) => retroarch.start(request),
            Self::Dolphin(request) => dolphin.start(request),
            Self::Pcsx2(request, evidence) => pcsx2.start(request, evidence),
            Self::Standalone(request) => standalone.start(request),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GamerBlockerKind {
    CheckingGame,
    UnknownSystem,
    ConflictingIdentity,
    ContentNeedsPreparation,
    EmulatorNotInstalled,
    EmulatorSetupIncomplete,
    EmulatorNotChecked,
    NoSafeEmulator,
    MultipleChoices,
    LaunchPlanInvalid,
}

/// Structured Gamer View blocker presentation. `detail` is the planner's
/// technical reason; `kind` and `emulator` determine the novice heading and
/// next action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GamerBlocker {
    pub(crate) kind: GamerBlockerKind,
    pub(crate) emulator: Option<String>,
    pub(crate) detail: String,
}

impl GamerBlocker {
    fn new(kind: GamerBlockerKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            emulator: None,
            detail: detail.into(),
        }
    }

    fn for_emulator(kind: GamerBlockerKind, emulator: &str, detail: impl Into<String>) -> Self {
        Self {
            kind,
            emulator: Some(emulator.to_string()),
            detail: detail.into(),
        }
    }

    pub(crate) fn heading(&self) -> String {
        match (&self.kind, self.emulator.as_deref()) {
            (GamerBlockerKind::CheckingGame, _) => "Checking this game".into(),
            (GamerBlockerKind::UnknownSystem, _) => "Identify game system".into(),
            (GamerBlockerKind::ConflictingIdentity, _) => "Resolve conflicting identity".into(),
            (GamerBlockerKind::ContentNeedsPreparation, _) => "Prepare game".into(),
            (GamerBlockerKind::EmulatorNotInstalled, Some(name)) => {
                format!("{name} is not installed")
            }
            (GamerBlockerKind::EmulatorSetupIncomplete, Some(name)) => {
                format!("{name} needs setup")
            }
            (GamerBlockerKind::EmulatorNotChecked, _) => "Check emulators".into(),
            (GamerBlockerKind::NoSafeEmulator, _) => "No safe emulator available".into(),
            (GamerBlockerKind::MultipleChoices, _) => "Choose how to play".into(),
            (GamerBlockerKind::LaunchPlanInvalid, _) => "Launch plan invalid".into(),
            (_, None) => "Can’t play yet".into(),
        }
    }
}

pub(crate) enum GamerPlayAction {
    Launch(Box<TypedLaunchRequest>),
    BlockedTyped(GamerBlocker),
}

/// Projects the same shared launch plan used by Advanced View into Gamer
/// View's single primary action. This is presentation only: a Ready result
/// carries the exact typed request that the selected adapter preflights again
/// before spawning anything.
pub(crate) fn gamer_play_action(input: &LaunchReadinessInput) -> GamerPlayAction {
    let LaunchReadinessInput::Plan {
        plan,
        retroarch_scanned,
        standalone_scans_complete,
        dolphin,
        pcsx2,
        duckstation,
        ppsspp,
        rpcs3,
        xemu,
        xenia,
        ..
    } = input
    else {
        return GamerPlayAction::BlockedTyped(match input {
            LaunchReadinessInput::EvidenceNotLoaded => GamerBlocker::new(
                GamerBlockerKind::CheckingGame,
                "Identity evidence is still loading.",
            ),
            LaunchReadinessInput::RetroArchNotScanned => GamerBlocker::new(
                GamerBlockerKind::EmulatorNotChecked,
                "RetroArch has not been checked yet.",
            ),
            LaunchReadinessInput::IdentityUnknown => GamerBlocker::new(
                GamerBlockerKind::UnknownSystem,
                "Game identity could not be resolved.",
            ),
            LaunchReadinessInput::IdentityConflicting => GamerBlocker::new(
                GamerBlockerKind::ConflictingIdentity,
                "Game identity evidence conflicts.",
            ),
            LaunchReadinessInput::Plan { .. } => unreachable!(),
        });
    };

    let has_remembered = plan
        .candidates
        .iter()
        .any(|candidate| candidate.preference == CandidatePreference::Remembered);
    let has_sole_eligible = plan
        .candidates
        .iter()
        .any(|candidate| candidate.preference == CandidatePreference::SoleEligible);
    let requests: Vec<TypedLaunchRequest> = plan
        .candidates
        .iter()
        .filter(|candidate| {
            (!has_remembered && !has_sole_eligible)
                || (has_remembered && candidate.preference == CandidatePreference::Remembered)
                || (!has_remembered
                    && has_sole_eligible
                    && candidate.preference == CandidatePreference::SoleEligible)
        })
        .filter_map(|candidate| {
            typed_launch_request(
                plan,
                candidate,
                dolphin.as_ref(),
                pcsx2.as_ref(),
                duckstation.as_ref(),
                ppsspp.as_ref(),
                rpcs3.as_ref(),
                xemu.as_ref(),
                xenia.as_ref(),
            )
        })
        .collect();
    if requests.len() == 1 {
        return GamerPlayAction::Launch(Box::new(
            requests.into_iter().next().expect("one request"),
        ));
    }
    if requests.len() > 1 {
        return GamerPlayAction::BlockedTyped(GamerBlocker::new(
            GamerBlockerKind::MultipleChoices,
            "More than one safe emulator choice is available; choose one explicitly.",
        ));
    }
    GamerPlayAction::BlockedTyped(gamer_blocker_for_plan(
        plan,
        *retroarch_scanned,
        *standalone_scans_complete,
    ))
}

pub(crate) struct DuckStationLaunchContext {
    pub(crate) discovery: archivefs_core::patch_manager::DuckStationProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::DuckStationProfileDiscoveryRoots,
    pub(crate) firmware_evidence: Vec<FirmwareIdentityRecord>,
    pub(crate) verified_ps1_serial: Option<String>,
}
pub(crate) struct PpssppLaunchContext {
    pub(crate) discovery: archivefs_core::patch_manager::PpssppProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::PpssppProfileDiscoveryRoots,
    pub(crate) verified_psp_disc_id: Option<String>,
}
pub(crate) struct Rpcs3LaunchContext {
    pub(crate) discovery: archivefs_core::patch_manager::Rpcs3ProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::Rpcs3ProfileDiscoveryRoots,
    pub(crate) verified_ps3_title_id: Option<String>,
}
pub(crate) struct XemuLaunchContext {
    pub(crate) discovery: archivefs_core::patch_manager::XemuProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::XemuProfileDiscoveryRoots,
    pub(crate) verified_xbox_title_id: Option<String>,
}
pub(crate) struct XeniaLaunchContext {
    pub(crate) discovery: archivefs_core::patch_manager::XeniaProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::XeniaProfileDiscoveryRoots,
    pub(crate) verified_xex_title_id: Option<String>,
    pub(crate) verified_xex_media_id: Option<String>,
}

enum StandaloneProcess {
    DuckStation(LaunchedDuckStationProcess),
    Ppsspp(LaunchedPpssppProcess),
    Rpcs3(LaunchedRpcs3Process),
    Xemu(LaunchedXemuProcess),
    Xenia(LaunchedXeniaProcess),
}

enum StandaloneLaunchStage {
    Starting(Receiver<Result<StandaloneProcess, String>>),
    Running(StandaloneProcess),
    // Retain the process wrapper until the terminal state is replaced so its
    // normal drop/reaping behavior remains unchanged.
    #[allow(dead_code)]
    Exited(StandaloneProcess),
    Failed(String),
}

#[derive(Default)]
pub(crate) struct StandaloneLaunchState {
    tracked: Option<(PathBuf, String, StandaloneLaunchStage)>,
}

impl StandaloneLaunchState {
    pub(crate) fn poll(&mut self) -> bool {
        let Some((path, adapter, stage)) = self.tracked.take() else {
            return false;
        };
        let (next, changed) = match stage {
            StandaloneLaunchStage::Starting(receiver) => match receiver.try_recv() {
                Ok(Ok(process)) => (StandaloneLaunchStage::Running(process), true),
                Ok(Err(error)) => (StandaloneLaunchStage::Failed(error), true),
                Err(TryRecvError::Empty) => (StandaloneLaunchStage::Starting(receiver), false),
                Err(TryRecvError::Disconnected) => (
                    StandaloneLaunchStage::Failed("launch worker stopped unexpectedly".into()),
                    true,
                ),
            },
            StandaloneLaunchStage::Running(mut process) => {
                let exited = match &mut process {
                    StandaloneProcess::DuckStation(p) => p.poll().is_some(),
                    StandaloneProcess::Ppsspp(p) => p.poll().is_some(),
                    StandaloneProcess::Rpcs3(p) => p.poll().is_some(),
                    StandaloneProcess::Xemu(p) => p.poll().is_some(),
                    StandaloneProcess::Xenia(p) => p.poll().is_some(),
                };
                (
                    if exited {
                        StandaloneLaunchStage::Exited(process)
                    } else {
                        StandaloneLaunchStage::Running(process)
                    },
                    exited,
                )
            }
            other @ (StandaloneLaunchStage::Exited(_) | StandaloneLaunchStage::Failed(_)) => {
                (other, false)
            }
        };
        self.tracked = Some((path, adapter, next));
        changed
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.tracked,
            Some((
                _,
                _,
                StandaloneLaunchStage::Starting(_) | StandaloneLaunchStage::Running(_)
            ))
        )
    }

    fn start(&mut self, request: StandaloneLaunchRequest) {
        let (path, adapter) = request.key();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = request.execute().map_err(|error| format!("{error:?}"));
            let _ = sender.send(result);
        });
        self.tracked = Some((path, adapter, StandaloneLaunchStage::Starting(receiver)));
    }
}

#[derive(Clone)]
pub(crate) enum StandaloneLaunchRequest {
    DuckStation(
        DuckStationLaunchRequest,
        archivefs_core::patch_manager::DuckStationProfileDiscoveryRoots,
        Vec<FirmwareIdentityRecord>,
    ),
    Ppsspp(
        PpssppLaunchRequest,
        archivefs_core::patch_manager::PpssppProfileDiscoveryRoots,
    ),
    Rpcs3(
        Rpcs3LaunchRequest,
        archivefs_core::patch_manager::Rpcs3ProfileDiscoveryRoots,
    ),
    Xemu(
        XemuLaunchRequest,
        archivefs_core::patch_manager::XemuProfileDiscoveryRoots,
    ),
    Xenia(
        XeniaLaunchRequest,
        archivefs_core::patch_manager::XeniaProfileDiscoveryRoots,
    ),
}

impl StandaloneLaunchRequest {
    fn adapter_name(&self) -> &'static str {
        match self {
            Self::DuckStation(_, _, _) => "DuckStation",
            Self::Ppsspp(_, _) => "PPSSPP",
            Self::Rpcs3(_, _) => "RPCS3",
            Self::Xemu(_, _) => "xemu",
            Self::Xenia(_, _) => "Xenia",
        }
    }

    fn key(&self) -> (PathBuf, String) {
        match self {
            Self::DuckStation(r, _, _) => (r.selected_content_path.clone(), "duckstation".into()),
            Self::Ppsspp(r, _) => (r.selected_content_path.clone(), "ppsspp".into()),
            Self::Rpcs3(r, _) => (r.selected_content_path.clone(), "rpcs3".into()),
            Self::Xemu(r, _) => (r.selected_content_path.clone(), "xemu".into()),
            Self::Xenia(r, _) => (r.selected_content_path.clone(), "xenia".into()),
        }
    }
    fn execute(self) -> Result<StandaloneProcess, String> {
        match self {
            Self::DuckStation(r, roots, firmware) => {
                preflight_and_launch_duckstation(&r, &roots, &firmware)
                    .map(StandaloneProcess::DuckStation)
                    .map_err(|e: DuckStationLaunchExecutionError| format!("{e:?}"))
            }
            Self::Ppsspp(r, roots) => preflight_and_launch_ppsspp(&r, &roots)
                .map(StandaloneProcess::Ppsspp)
                .map_err(|e: PpssppLaunchExecutionError| format!("{e:?}")),
            Self::Rpcs3(r, roots) => preflight_and_launch_rpcs3(&r, &roots)
                .map(StandaloneProcess::Rpcs3)
                .map_err(|e: Rpcs3LaunchExecutionError| format!("{e:?}")),
            Self::Xemu(r, roots) => preflight_and_launch_xemu(&r, &roots)
                .map(StandaloneProcess::Xemu)
                .map_err(|e: XemuLaunchExecutionError| format!("{e:?}")),
            Self::Xenia(r, roots) => preflight_and_launch_xenia(&r, &roots)
                .map(StandaloneProcess::Xenia)
                .map_err(|e: XeniaLaunchExecutionError| format!("{e:?}")),
        }
    }
}

/// The real, already-gathered Dolphin discovery this panel needs to compute
/// a launch binding - never (re)discovered by this module itself. See
/// `main.rs`'s `DolphinLocalProfilesState`.
pub(crate) struct DolphinLaunchContext {
    pub(crate) discovery: DolphinLocalProfileDiscovery,
    pub(crate) roots: DolphinLocalDiscoveryRoots,
}

/// The real, already-gathered PCSX2 profile discovery and PS2 firmware
/// evidence this panel needs to compute a launch binding and pass to core's
/// own fresh BIOS verification - never (re)discovered or (re)parsed by this
/// module itself. See `main.rs`'s `Pcsx2LaunchProfilesState` and
/// `Pcsx2FirmwareEvidenceState`.
pub(crate) struct Pcsx2LaunchContext {
    pub(crate) discovery: Pcsx2ProfileDiscovery,
    pub(crate) roots: Pcsx2ProfileDiscoveryRoots,
    /// PS2 BIOS evidence resolved from the user's registered DAT sources -
    /// this module never downloads, parses, or invents any of it; see
    /// `main.rs`'s `pcsx2_firmware_evidence_from_registry`. An empty slice
    /// is an honest "no evidence available" state, never `Verified`.
    pub(crate) firmware_evidence: Vec<FirmwareIdentityRecord>,
    /// The verified PS2 serial for the currently focused game, when one
    /// exists - taken from `VerifiedIdentityFact::Ps2Serial`, never
    /// derived from `plan.game_key` alone (which may instead be a verified
    /// executable CRC when no serial was verified) - see
    /// [`pcsx2_launch_request`].
    pub(crate) verified_ps2_serial: Option<String>,
}

// ---------------------------------------------------------------------------
// Launch RetroArch state
// ---------------------------------------------------------------------------

/// Identifies exactly which candidate a tracked launch belongs to - the
/// selected content path (the archive itself, for the direct-loose-file
/// slice this supports) plus the exact requested RetroArch profile/core.
/// Compared against the candidate currently being rendered so a result for
/// one selection is never displayed as belonging to a different one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RetroArchLaunchKey {
    content_path: PathBuf,
    profile: ProfileRef,
    core_stem: String,
}

impl RetroArchLaunchKey {
    fn from_request(request: &RetroArchLaunchRequest) -> Self {
        Self {
            content_path: request.selected_content_path.clone(),
            profile: request.profile,
            core_stem: request.core_stem.clone(),
        }
    }
}

enum RetroArchLaunchStage {
    Starting {
        receiver: Receiver<Result<LaunchedRetroArchProcess, LaunchExecutionError>>,
    },
    Running {
        process: LaunchedRetroArchProcess,
    },
    Exited {
        process: LaunchedRetroArchProcess,
    },
    Failed {
        error: LaunchExecutionError,
    },
}

/// Owned presentation snapshot for the launch currently associated with one
/// exact request. Both Advanced View and Gamer View render this same state,
/// so a preflight/spawn failure cannot disappear merely because the launch
/// was initiated from the simpler front door.
pub(crate) enum RetroArchLaunchDisplay {
    Idle,
    Starting,
    Running {
        pid: u32,
    },
    Exited {
        success: bool,
        status_detail: String,
        stderr_tail: Option<String>,
    },
    Failed {
        message: &'static str,
        detail: String,
    },
}

/// Phase 1 tracks at most one launch at a time, regardless of which
/// candidate/selection is currently rendered - see the module doc comment.
/// [`Self::poll`] always advances whatever is tracked so a running process
/// is reaped even after the user selects a different game; rendering only
/// ever shows it when the currently displayed candidate's key matches.
#[derive(Default)]
pub(crate) struct RetroArchLaunchState {
    tracked: Option<(RetroArchLaunchKey, RetroArchLaunchStage)>,
}

impl RetroArchLaunchState {
    /// Non-blocking. Returns whether anything changed (a repaint hint).
    pub(crate) fn poll(&mut self) -> bool {
        let Some((key, stage)) = self.tracked.take() else {
            return false;
        };
        match stage {
            RetroArchLaunchStage::Starting { receiver } => match receiver.try_recv() {
                Ok(Ok(process)) => {
                    self.tracked = Some((key, RetroArchLaunchStage::Running { process }));
                    true
                }
                Ok(Err(error)) => {
                    self.tracked = Some((key, RetroArchLaunchStage::Failed { error }));
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.tracked = Some((key, RetroArchLaunchStage::Starting { receiver }));
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.tracked = Some((
                        key,
                        RetroArchLaunchStage::Failed {
                            error: LaunchExecutionError::Spawn(LaunchSpawnError::Spawn(
                                std::io::Error::other(
                                    "the launch worker stopped without reporting a result",
                                ),
                            )),
                        },
                    ));
                    true
                }
            },
            RetroArchLaunchStage::Running { mut process } => {
                if process.poll().is_some() {
                    self.tracked = Some((key, RetroArchLaunchStage::Exited { process }));
                    true
                } else {
                    self.tracked = Some((key, RetroArchLaunchStage::Running { process }));
                    false
                }
            }
            other @ (RetroArchLaunchStage::Exited { .. } | RetroArchLaunchStage::Failed { .. }) => {
                self.tracked = Some((key, other));
                false
            }
        }
    }

    /// Whether the caller should keep repainting - a `Starting` preflight
    /// or a `Running` process can change state without any user input.
    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.tracked,
            Some((
                _,
                RetroArchLaunchStage::Starting { .. } | RetroArchLaunchStage::Running { .. }
            ))
        )
    }

    pub(crate) fn start(&mut self, request: RetroArchLaunchRequest) {
        let key = RetroArchLaunchKey::from_request(&request);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let filesystem = HostReadOnlyFilesystem;
            let environment = DiscoveryEnvironment::from_process_environment();
            let result = preflight_and_launch_retroarch(&request, &filesystem, &environment);
            let _ = sender.send(result);
        });
        self.tracked = Some((key, RetroArchLaunchStage::Starting { receiver }));
    }

    pub(crate) fn display_for(
        &mut self,
        request: &RetroArchLaunchRequest,
    ) -> RetroArchLaunchDisplay {
        let this_key = RetroArchLaunchKey::from_request(request);
        match self.tracked.as_mut() {
            Some((key, stage)) if *key == this_key => match stage {
                RetroArchLaunchStage::Starting { .. } => RetroArchLaunchDisplay::Starting,
                RetroArchLaunchStage::Running { process } => {
                    RetroArchLaunchDisplay::Running { pid: process.pid }
                }
                RetroArchLaunchStage::Exited { process } => {
                    let report: &LaunchExitReport = process
                        .poll()
                        .expect("Exited stage always has a cached exit report");
                    let success = matches!(&report.status, Ok(status) if status.success());
                    let status_detail = match &report.status {
                        Ok(status) => format!("{status}"),
                        Err(error) => format!("wait() failed: {error}"),
                    };
                    let stderr_tail = (!success && !report.stderr.is_empty())
                        .then(|| String::from_utf8_lossy(&report.stderr).into_owned());
                    RetroArchLaunchDisplay::Exited {
                        success,
                        status_detail,
                        stderr_tail,
                    }
                }
                RetroArchLaunchStage::Failed { error } => {
                    let (message, detail) = launch_error_message(error);
                    RetroArchLaunchDisplay::Failed { message, detail }
                }
            },
            _ => RetroArchLaunchDisplay::Idle,
        }
    }
}

// ---------------------------------------------------------------------------
// Launch Dolphin state
// ---------------------------------------------------------------------------

/// Identifies exactly which candidate a tracked Dolphin launch belongs to -
/// the selected content path plus the exact requested Dolphin profile id.
/// Compared against the candidate currently being rendered so a result for
/// one selection is never displayed as belonging to a different one - the
/// same reasoning as [`RetroArchLaunchKey`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct DolphinLaunchKey {
    content_path: PathBuf,
    profile_id: String,
}

impl DolphinLaunchKey {
    fn from_request(request: &DolphinLaunchRequest) -> Self {
        Self {
            content_path: request.selected_content_path.clone(),
            profile_id: request.profile_id.clone(),
        }
    }
}

enum DolphinLaunchStage {
    Starting {
        receiver: Receiver<Result<LaunchedDolphinProcess, DolphinLaunchExecutionError>>,
    },
    Running {
        process: LaunchedDolphinProcess,
    },
    Exited {
        process: LaunchedDolphinProcess,
    },
    Failed {
        error: DolphinLaunchExecutionError,
    },
}

/// A very small sibling of [`RetroArchLaunchState`] rather than a shared
/// generic tracker - the two processes/errors/requests are distinct core
/// types with no shared trait today, and genericizing this over both would
/// add real risk to the already-working RetroArch path for no benefit this
/// phase needs. Same "tracks at most one launch, reaped regardless of the
/// currently rendered selection" contract as [`RetroArchLaunchState`] - see
/// its own doc comment.
#[derive(Default)]
pub(crate) struct DolphinLaunchState {
    tracked: Option<(DolphinLaunchKey, DolphinLaunchStage)>,
}

impl DolphinLaunchState {
    /// Non-blocking. Returns whether anything changed (a repaint hint).
    pub(crate) fn poll(&mut self) -> bool {
        let Some((key, stage)) = self.tracked.take() else {
            return false;
        };
        match stage {
            DolphinLaunchStage::Starting { receiver } => match receiver.try_recv() {
                Ok(Ok(process)) => {
                    self.tracked = Some((key, DolphinLaunchStage::Running { process }));
                    true
                }
                Ok(Err(error)) => {
                    self.tracked = Some((key, DolphinLaunchStage::Failed { error }));
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.tracked = Some((key, DolphinLaunchStage::Starting { receiver }));
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.tracked = Some((
                        key,
                        DolphinLaunchStage::Failed {
                            error: DolphinLaunchExecutionError::Spawn(
                                DolphinLaunchSpawnError::Spawn(std::io::Error::other(
                                    "the launch worker stopped without reporting a result",
                                )),
                            ),
                        },
                    ));
                    true
                }
            },
            DolphinLaunchStage::Running { mut process } => {
                if process.poll().is_some() {
                    self.tracked = Some((key, DolphinLaunchStage::Exited { process }));
                    true
                } else {
                    self.tracked = Some((key, DolphinLaunchStage::Running { process }));
                    false
                }
            }
            other @ (DolphinLaunchStage::Exited { .. } | DolphinLaunchStage::Failed { .. }) => {
                self.tracked = Some((key, other));
                false
            }
        }
    }

    /// Whether the caller should keep repainting - a `Starting` preflight
    /// or a `Running` process can change state without any user input.
    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.tracked,
            Some((
                _,
                DolphinLaunchStage::Starting { .. } | DolphinLaunchStage::Running { .. }
            ))
        )
    }

    /// Re-derives the Dolphin discovery roots fresh from the environment
    /// inside the background thread (never the roots captured at button-
    /// render time) - the same "never trust cached readiness as execution
    /// authority" reasoning [`preflight_dolphin_launch`] itself documents.
    /// A `HOME`-unavailable failure has no dedicated
    /// [`DolphinLaunchPreflightErrorKind`] (roots are a precondition
    /// `preflight_dolphin_launch` never constructs itself), so it is
    /// reported through the same `Spawn` bucket the disconnected-worker
    /// case above already uses.
    fn start(&mut self, request: DolphinLaunchRequest) {
        let key = DolphinLaunchKey::from_request(&request);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = match DolphinLocalDiscoveryRoots::from_environment() {
                Ok(roots) => preflight_and_launch_dolphin(&request, &roots),
                Err(error) => Err(DolphinLaunchExecutionError::Spawn(
                    DolphinLaunchSpawnError::Spawn(std::io::Error::other(error.to_string())),
                )),
            };
            let _ = sender.send(result);
        });
        self.tracked = Some((key, DolphinLaunchStage::Starting { receiver }));
    }
}

// ---------------------------------------------------------------------------
// Launch PCSX2 state
// ---------------------------------------------------------------------------

/// Identifies exactly which candidate a tracked PCSX2 launch belongs to -
/// the selected content path plus the exact requested PCSX2 profile id.
/// Same reasoning as [`DolphinLaunchKey`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pcsx2LaunchKey {
    content_path: PathBuf,
    profile_id: String,
}

impl Pcsx2LaunchKey {
    fn from_request(request: &Pcsx2LaunchRequest) -> Self {
        Self {
            content_path: request.selected_content_path.clone(),
            profile_id: request.profile_id.clone(),
        }
    }
}

enum Pcsx2LaunchStage {
    Starting {
        receiver: Receiver<Result<LaunchedPcsx2Process, Pcsx2LaunchExecutionError>>,
    },
    Running {
        process: LaunchedPcsx2Process,
    },
    Exited {
        process: LaunchedPcsx2Process,
    },
    Failed {
        error: Pcsx2LaunchExecutionError,
    },
}

/// A small sibling of [`DolphinLaunchState`] - same "tracks at most one
/// launch, reaped regardless of the currently rendered selection" contract,
/// same reasoning for staying a distinct, non-generic tracker rather than
/// unifying all three emulators' launch state into one generic type.
#[derive(Default)]
pub(crate) struct Pcsx2LaunchState {
    tracked: Option<(Pcsx2LaunchKey, Pcsx2LaunchStage)>,
}

impl Pcsx2LaunchState {
    /// Non-blocking. Returns whether anything changed (a repaint hint).
    pub(crate) fn poll(&mut self) -> bool {
        let Some((key, stage)) = self.tracked.take() else {
            return false;
        };
        match stage {
            Pcsx2LaunchStage::Starting { receiver } => match receiver.try_recv() {
                Ok(Ok(process)) => {
                    self.tracked = Some((key, Pcsx2LaunchStage::Running { process }));
                    true
                }
                Ok(Err(error)) => {
                    self.tracked = Some((key, Pcsx2LaunchStage::Failed { error }));
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.tracked = Some((key, Pcsx2LaunchStage::Starting { receiver }));
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.tracked = Some((
                        key,
                        Pcsx2LaunchStage::Failed {
                            error: Pcsx2LaunchExecutionError::Spawn(Pcsx2LaunchSpawnError::Spawn(
                                std::io::Error::other(
                                    "the launch worker stopped without reporting a result",
                                ),
                            )),
                        },
                    ));
                    true
                }
            },
            Pcsx2LaunchStage::Running { mut process } => {
                if process.poll().is_some() {
                    self.tracked = Some((key, Pcsx2LaunchStage::Exited { process }));
                    true
                } else {
                    self.tracked = Some((key, Pcsx2LaunchStage::Running { process }));
                    false
                }
            }
            other @ (Pcsx2LaunchStage::Exited { .. } | Pcsx2LaunchStage::Failed { .. }) => {
                self.tracked = Some((key, other));
                false
            }
        }
    }

    /// Whether the caller should keep repainting - a `Starting` preflight
    /// or a `Running` process can change state without any user input.
    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.tracked,
            Some((
                _,
                Pcsx2LaunchStage::Starting { .. } | Pcsx2LaunchStage::Running { .. }
            ))
        )
    }

    /// Re-derives the PCSX2 discovery roots fresh from the environment
    /// inside the background thread (never the roots captured at button-
    /// render time) - the same "never trust cached readiness as execution
    /// authority" reasoning as [`DolphinLaunchState::start`]. `evidence` is
    /// the already-loaded PS2 firmware evidence snapshot (see
    /// `main.rs`'s `Pcsx2FirmwareEvidenceState`) - core's own preflight
    /// still re-hashes the actual BIOS file fresh against it at this exact
    /// moment, so a stale on-disk BIOS is never silently accepted; only the
    /// evidence *records themselves* are not re-parsed from the DAT source
    /// on every click, the same way this whole panel does not re-scan
    /// RetroArch/Dolphin/PCSX2 profiles on every click either.
    fn start(&mut self, request: Pcsx2LaunchRequest, evidence: Vec<FirmwareIdentityRecord>) {
        let key = Pcsx2LaunchKey::from_request(&request);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = match Pcsx2ProfileDiscoveryRoots::from_environment() {
                Ok(roots) => preflight_and_launch_pcsx2(&request, &roots, &evidence),
                Err(error) => Err(Pcsx2LaunchExecutionError::Spawn(
                    Pcsx2LaunchSpawnError::Spawn(std::io::Error::other(error.to_string())),
                )),
            };
            let _ = sender.send(result);
        });
        self.tracked = Some((key, Pcsx2LaunchStage::Starting { receiver }));
    }
}

/// Whether `path` has a directly launchable Dolphin disc-image extension
/// (case-insensitive). Kept as a
/// small local re-derivation rather than reaching into
/// `archivefs_core::launch::dolphin_command`'s private extension check: this
/// is only ever used to decide whether to *show* the button, never to
/// authorize a launch - core's own preflight re-checks this independently
/// and is the only thing that can actually refuse a launch on this basis.
fn is_direct_dolphin_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["iso", "gcm", "rvz", "ciso", "wbfs"]
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn is_wbfs(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wbfs"))
}

/// The single eligibility rule for the "Launch Dolphin" button, and the
/// exact facts sent to core when it is clicked - one function so the button
/// can never show for a request core's own preflight would refuse. `Some`
/// only when every current Dolphin Phase-2 condition holds:
/// [`LaunchTarget::Standalone`] for adapter id `"dolphin"`, the resolved
/// plan platform is `GameCube` or `Wii`,
/// strictly [`LaunchReadiness::Ready`], direct loose/plain content
/// ([`LaunchContainerKind::PlainFile`], `requires_mount == false`, a direct
/// Dolphin extension), no blockers, no warnings, the named profile is
/// still present in `context.discovery`, and
/// [`resolve_dolphin_native_launch_binding`] proves a real launch binding
/// for it right now (this is what actually excludes Flatpak/AppImage/
/// portable Dolphin installs and any other binding-refusing state - see its
/// own doc comment - never re-derived or guessed here).
fn dolphin_launch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    context: &DolphinLaunchContext,
) -> Option<DolphinLaunchRequest> {
    let (Some(platform_id), Some(game_key)) = (&plan.platform_id, &plan.game_key) else {
        return None;
    };
    if platform_id != DOLPHIN_SUPPORTED_PLATFORM_ID && platform_id != "Wii" {
        return None;
    }
    if candidate.readiness != LaunchReadiness::Ready {
        return None;
    }
    if !candidate.blockers.is_empty() || !candidate.warnings.is_empty() {
        return None;
    }
    if candidate.content.requires_mount {
        return None;
    }
    if candidate.content.container != Some(LaunchContainerKind::PlainFile) {
        return None;
    }
    let content_path = candidate.content.resolved_path.clone()?;
    if !is_direct_dolphin_extension(&content_path) {
        return None;
    }
    if is_wbfs(&content_path) && platform_id != "Wii" {
        return None;
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        return None;
    };
    if *adapter_id != "dolphin" {
        return None;
    }
    let profile = context
        .discovery
        .profiles
        .iter()
        .find(|profile| &profile.profile_id == profile_id)?;
    let binding = resolve_dolphin_native_launch_binding(profile, &context.roots).ok()?;
    Some(dolphin_launch_request_from_binding(
        profile.profile_id.clone(),
        game_key.clone(),
        content_path,
        binding,
    ))
}

/// The pure fact-carrying step of [`dolphin_launch_request`], split out so
/// it can be exercised directly against a hand-built
/// [`archivefs_core::patch_manager::DolphinNativeLaunchBinding`] without a
/// real filesystem fixture: proof that this module only ever *copies*
/// `binding.executable`/`binding.user_directory_mode` into the request's
/// facts, never reconstructs, reinterprets, or turns either into an argv
/// string (`-u`/`-e`) itself.
fn dolphin_launch_request_from_binding(
    profile_id: String,
    expected_game_id: String,
    selected_content_path: PathBuf,
    binding: archivefs_core::patch_manager::DolphinNativeLaunchBinding,
) -> DolphinLaunchRequest {
    DolphinLaunchRequest {
        selected_content_path,
        expected_game_id,
        profile_id,
        expected_executable: binding.executable,
        expected_user_directory_mode: binding.user_directory_mode,
    }
}

/// Translates a core Dolphin launch error into a short player-facing
/// message - the same reasoning as [`launch_error_message`].
fn dolphin_launch_error_message(error: &DolphinLaunchExecutionError) -> (&'static str, String) {
    match error {
        DolphinLaunchExecutionError::Preflight(preflight) => {
            let message = match preflight.kind {
                DolphinLaunchPreflightErrorKind::ContentNotFound
                | DolphinLaunchPreflightErrorKind::ContentIsSymlink
                | DolphinLaunchPreflightErrorKind::ContentNotRegularFile
                | DolphinLaunchPreflightErrorKind::ContentChangedBeforeSpawn => {
                    "This game's file is no longer available where it was last seen."
                }
                DolphinLaunchPreflightErrorKind::ContentRequiresMount => {
                    "This game's content needs to be mounted before it can launch."
                }
                DolphinLaunchPreflightErrorKind::ContentFormatUnsupported => {
                    "Only a direct GameCube .iso or .gcm file can be launched here."
                }
                DolphinLaunchPreflightErrorKind::IdentityUnresolved
                | DolphinLaunchPreflightErrorKind::IdentityMismatch => {
                    "This game's identity changed since it was last checked."
                }
                DolphinLaunchPreflightErrorKind::ProfileNotFound
                | DolphinLaunchPreflightErrorKind::BindingUnavailable
                | DolphinLaunchPreflightErrorKind::BindingDrift => {
                    "The selected Dolphin installation is no longer available the way it was \
                     last checked."
                }
                DolphinLaunchPreflightErrorKind::RequestedCandidateNotFound
                | DolphinLaunchPreflightErrorKind::CandidateNotReady
                | DolphinLaunchPreflightErrorKind::CandidateContentUnsupported => {
                    "This launch option changed since it was last checked - re-check readiness \
                     and try again."
                }
                DolphinLaunchPreflightErrorKind::CommandBlocked
                | DolphinLaunchPreflightErrorKind::CommandMissing
                | DolphinLaunchPreflightErrorKind::ExecutableMissing
                | DolphinLaunchPreflightErrorKind::ExecutableUnsafe
                | DolphinLaunchPreflightErrorKind::ExecutableNotExecutable
                | DolphinLaunchPreflightErrorKind::ExplicitRootInvalid => {
                    "Dolphin is no longer available where it was last found."
                }
                DolphinLaunchPreflightErrorKind::ContentPathNotAbsolute => {
                    "This game's file path is invalid."
                }
            };
            (
                message,
                format!("{:?}: {}", preflight.kind, preflight.detail),
            )
        }
        DolphinLaunchExecutionError::Spawn(DolphinLaunchSpawnError::Spawn(io_error)) => {
            ("Dolphin failed to launch.", io_error.to_string())
        }
    }
}

/// Whether `path`'s extension is `.iso` (case-insensitive) - the only
/// direct PS2 content this native launch slice supports (no CHD yet). Same
/// reasoning as [`is_direct_dolphin_extension`]: only ever decides whether
/// to *show* the button, never authorizes a launch.
fn is_direct_ps2_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("iso"))
}

/// The single eligibility rule for the "Launch PCSX2" button, and the exact
/// facts sent to core when it is clicked - one function so the button can
/// never show for a request core's own preflight would refuse. `Some` only
/// when every current PCSX2 first-slice condition holds:
/// [`LaunchTarget::Standalone`] for adapter id `"pcsx2"`, the resolved plan
/// platform is exactly [`PCSX2_SUPPORTED_PLATFORM_ID`] (`"PS2"`), a verified
/// PS2 serial actually exists for this game (never just any resolved
/// `game_key`, which may instead be a verified executable CRC - see
/// [`Pcsx2LaunchContext::verified_ps2_serial`]), strictly
/// [`LaunchReadiness::Ready`] with [`FirmwareReadiness::Verified`], direct loose/plain content
/// ([`LaunchContainerKind::PlainFile`], `requires_mount == false`, a `.iso`
/// extension), no blockers, no warnings, the named profile is still present
/// in `context.discovery`, and [`resolve_pcsx2_native_launch_binding`]
/// proves a real launch binding for it right now - the same reasoning as
/// [`dolphin_launch_request`], plus the additional PS2-serial requirement
/// core's own [`Pcsx2LaunchRequest`] needs that Dolphin's does not.
///
/// This function deliberately does not re-check firmware/BIOS verification
/// itself: `candidate.readiness`/`candidate.firmware` already reflect
/// whatever [`archivefs_core::launch::build_launch_plan`] computed from the
/// same `context.firmware_evidence` this function hands to the request, so
/// a candidate whose BIOS is not genuinely `Verified` is already `Blocked`
/// or `ReadyWithWarnings`, never `Ready` - see
/// `main.rs`'s `build_launch_readiness_input` for where that projection
/// happens.
fn pcsx2_launch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    context: &Pcsx2LaunchContext,
) -> Option<Pcsx2LaunchRequest> {
    let (Some(platform_id), Some(_game_key)) = (&plan.platform_id, &plan.game_key) else {
        return None;
    };
    if platform_id != PCSX2_SUPPORTED_PLATFORM_ID {
        return None;
    }
    let verified_ps2_serial = context.verified_ps2_serial.clone()?;
    if candidate.readiness != LaunchReadiness::Ready {
        return None;
    }
    if candidate.firmware != FirmwareReadiness::Verified {
        return None;
    }
    if !candidate.blockers.is_empty() || !candidate.warnings.is_empty() {
        return None;
    }
    if candidate.content.requires_mount {
        return None;
    }
    if candidate.content.container != Some(LaunchContainerKind::PlainFile) {
        return None;
    }
    let content_path = candidate.content.resolved_path.clone()?;
    if !is_direct_ps2_extension(&content_path) {
        return None;
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        return None;
    };
    if *adapter_id != "pcsx2" {
        return None;
    }
    let profile = context
        .discovery
        .profiles
        .iter()
        .find(|profile| &profile.profile_id == profile_id)?;
    let binding = resolve_pcsx2_native_launch_binding(profile, &context.roots).ok()?;
    Some(Pcsx2LaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: platform_id.clone(),
        expected_game_key: verified_ps2_serial.clone(),
        expected_ps2_serial: verified_ps2_serial,
        profile_id: profile.profile_id.clone(),
        expected_executable: binding.executable,
        expected_user_directory_mode: binding.user_directory_mode,
    })
}

/// Translates a core PCSX2 launch error into a short player-facing message -
/// the same reasoning as [`dolphin_launch_error_message`]. Core's own
/// [`Pcsx2LaunchPreflightErrorKind::CandidateNotReady`]/
/// `CandidateContentUnsupported`/`RequestedCandidateNotFound` cover *every*
/// readiness regression at click time - including the BIOS having stopped
/// verifying since the panel was last drawn - as one coarse bucket; core
/// does not report a separate "BIOS specifically regressed" kind, so this
/// function is honest about that rather than fabricating a BIOS-specific
/// message it cannot actually prove.
fn pcsx2_launch_error_message(error: &Pcsx2LaunchExecutionError) -> (&'static str, String) {
    match error {
        Pcsx2LaunchExecutionError::Preflight(preflight) => {
            let message = match preflight.kind {
                Pcsx2LaunchPreflightErrorKind::ContentNotFound
                | Pcsx2LaunchPreflightErrorKind::ContentIsSymlink
                | Pcsx2LaunchPreflightErrorKind::ContentNotRegularFile
                | Pcsx2LaunchPreflightErrorKind::ContentChangedBeforeSpawn => {
                    "Game file changed since readiness was checked."
                }
                Pcsx2LaunchPreflightErrorKind::ContentRequiresMount => {
                    "This game's content needs to be mounted before it can launch."
                }
                Pcsx2LaunchPreflightErrorKind::ContentFormatUnsupported => {
                    "Only a direct PS2 .iso file can be launched here."
                }
                Pcsx2LaunchPreflightErrorKind::IdentityUnresolved
                | Pcsx2LaunchPreflightErrorKind::IdentityMismatch
                | Pcsx2LaunchPreflightErrorKind::Ps2SerialUnavailable
                | Pcsx2LaunchPreflightErrorKind::Ps2SerialMismatch => {
                    "Game identity changed; refresh readiness."
                }
                Pcsx2LaunchPreflightErrorKind::DiscoveryFailed
                | Pcsx2LaunchPreflightErrorKind::ProfileNotFound
                | Pcsx2LaunchPreflightErrorKind::BindingUnavailable
                | Pcsx2LaunchPreflightErrorKind::BindingDrift => {
                    "PCSX2 installation changed; refresh readiness."
                }
                Pcsx2LaunchPreflightErrorKind::RequestedCandidateNotFound
                | Pcsx2LaunchPreflightErrorKind::CandidateNotReady
                | Pcsx2LaunchPreflightErrorKind::CandidateContentUnsupported => {
                    "PCSX2 is no longer ready to launch - re-check readiness and try again."
                }
                Pcsx2LaunchPreflightErrorKind::CommandBlocked
                | Pcsx2LaunchPreflightErrorKind::CommandMissing
                | Pcsx2LaunchPreflightErrorKind::ExecutableMissing
                | Pcsx2LaunchPreflightErrorKind::ExecutableUnsafe
                | Pcsx2LaunchPreflightErrorKind::ExecutableNotExecutable
                | Pcsx2LaunchPreflightErrorKind::DataPathRootInvalid => {
                    "PCSX2 executable is no longer available."
                }
                Pcsx2LaunchPreflightErrorKind::ContentPathNotAbsolute => {
                    "This game's file path is invalid."
                }
            };
            (
                message,
                format!("{:?}: {}", preflight.kind, preflight.detail),
            )
        }
        Pcsx2LaunchExecutionError::Spawn(Pcsx2LaunchSpawnError::Spawn(io_error)) => {
            ("PCSX2 failed to launch.", io_error.to_string())
        }
    }
}

/// The single eligibility rule for the "Launch RetroArch" button, and the
/// exact facts sent to core when it is clicked - one function so the button
/// can never show for a request core's own preflight would refuse. `Some`
/// only when every Phase-1 condition holds: [`LaunchTarget::RetroArchCore`],
/// [`ProfileKind::Native`], strictly [`LaunchReadiness::Ready`] (never
/// `ReadyWithWarnings`), direct loose/plain content
/// ([`LaunchContainerKind::PlainFile`], `requires_mount == false`), no
/// blockers, no warnings, and a resolved plan-level platform/game key.
fn retroarch_launch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<RetroArchLaunchRequest> {
    let (Some(platform_id), Some(game_key)) = (&plan.platform_id, &plan.game_key) else {
        return None;
    };
    if candidate.readiness != LaunchReadiness::Ready {
        return None;
    }
    if !candidate.blockers.is_empty() || !candidate.warnings.is_empty() {
        return None;
    }
    if candidate.content.requires_mount {
        return None;
    }
    if candidate.content.container != Some(LaunchContainerKind::PlainFile) {
        return None;
    }
    let content_path = candidate.content.resolved_path.clone()?;
    let LaunchTarget::RetroArchCore {
        profile, core_stem, ..
    } = &candidate.target
    else {
        return None;
    };
    if profile.profile_kind != ProfileKind::Native {
        return None;
    }
    Some(RetroArchLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: platform_id.clone(),
        expected_game_key: game_key.clone(),
        profile: *profile,
        core_stem: core_stem.clone(),
    })
}

/// Translates a core launch error into a short player-facing message. Never
/// says "no emulator installed" for an identity/content revalidation
/// failure - the detailed technical text stays available behind
/// [`widgets::technical_details`].
fn launch_error_message(error: &LaunchExecutionError) -> (&'static str, String) {
    match error {
        LaunchExecutionError::Preflight(preflight) => {
            let message = match preflight.kind {
                LaunchPreflightErrorKind::ContentNotFound
                | LaunchPreflightErrorKind::ContentIsSymlink
                | LaunchPreflightErrorKind::ContentNotRegularFile
                | LaunchPreflightErrorKind::ContentChangedBeforeSpawn => {
                    "This game's file is no longer available where it was last seen."
                }
                LaunchPreflightErrorKind::ContentRequiresMount => {
                    "This game's content needs to be mounted before it can launch."
                }
                LaunchPreflightErrorKind::IdentityUnresolved
                | LaunchPreflightErrorKind::IdentityMismatch => {
                    "This game's identity changed since it was last checked."
                }
                LaunchPreflightErrorKind::UnsupportedProfileKind => {
                    "The selected RetroArch profile is no longer a supported native install."
                }
                LaunchPreflightErrorKind::DiscoveryFailed => {
                    "RetroArch could not be re-checked on this machine."
                }
                LaunchPreflightErrorKind::RequestedCandidateNotFound
                | LaunchPreflightErrorKind::CandidateNotReady
                | LaunchPreflightErrorKind::CandidateContentUnsupported => {
                    "This launch option changed since it was last checked - re-check readiness \
                     and try again."
                }
                LaunchPreflightErrorKind::CommandBlocked
                | LaunchPreflightErrorKind::CommandMissing
                | LaunchPreflightErrorKind::ExecutableMissing
                | LaunchPreflightErrorKind::ExecutableUnsafe
                | LaunchPreflightErrorKind::ExecutableNotExecutable
                | LaunchPreflightErrorKind::CoreMissing
                | LaunchPreflightErrorKind::CoreUnsafe => {
                    "RetroArch or its core is no longer available where it was last found."
                }
                LaunchPreflightErrorKind::ContentPathNotAbsolute => {
                    "This game's file path is invalid."
                }
            };
            (
                message,
                format!("{:?}: {}", preflight.kind, preflight.detail),
            )
        }
        LaunchExecutionError::Spawn(LaunchSpawnError::Spawn(io_error)) => {
            ("RetroArch failed to launch.", io_error.to_string())
        }
    }
}

pub(crate) fn show_launch_readiness_panel(
    ui: &mut egui::Ui,
    input: &LaunchReadinessInput,
    retroarch_launch_state: &mut RetroArchLaunchState,
    dolphin_launch_state: &mut DolphinLaunchState,
    pcsx2_launch_state: &mut Pcsx2LaunchState,
    standalone_launch_state: &mut StandaloneLaunchState,
) {
    widgets::section_header(
        ui,
        "Play / Launch readiness",
        Some(
            "Ways this game can be played. Check emulator readiness, then launch only a command backed by verified identity and resolved content.",
        ),
    );

    match input {
        LaunchReadinessInput::EvidenceNotLoaded => {
            widgets::card(ui, |ui| {
                ui.label("Load ROM Identity & Evidence first.");
            });
        }
        LaunchReadinessInput::RetroArchNotScanned => {
            widgets::card(ui, |ui| {
                ui.label("Scan RetroArch profiles to check installed cores.");
            });
        }
        LaunchReadinessInput::IdentityUnknown => {
            widgets::banner(
                ui,
                "Identity unresolved",
                "This game's identity could not be verified, so no launch options can be \
                 safely planned yet.",
                widgets::StatusTone::Pending,
            );
        }
        LaunchReadinessInput::IdentityConflicting => {
            widgets::banner(
                ui,
                "Identity conflicts",
                "Evidence for this game's identity conflicts and needs resolution before \
                 launch options can be planned.",
                widgets::StatusTone::Warning,
            );
        }
        LaunchReadinessInput::Plan {
            plan,
            dolphin,
            pcsx2,
            duckstation,
            ppsspp,
            rpcs3,
            xemu,
            xenia,
            ..
        } => show_plan(
            ui,
            plan,
            dolphin.as_ref(),
            pcsx2.as_ref(),
            duckstation.as_ref(),
            ppsspp.as_ref(),
            rpcs3.as_ref(),
            xemu.as_ref(),
            xenia.as_ref(),
            retroarch_launch_state,
            dolphin_launch_state,
            pcsx2_launch_state,
            standalone_launch_state,
        ),
    }
}

// This is the UI boundary for the complete launch plan and its per-adapter
// state; keeping these inputs explicit makes ownership and dispatch visible.
#[allow(clippy::too_many_arguments)]
fn show_plan(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xemu: Option<&XemuLaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
    retroarch_launch_state: &mut RetroArchLaunchState,
    dolphin_launch_state: &mut DolphinLaunchState,
    pcsx2_launch_state: &mut Pcsx2LaunchState,
    standalone_launch_state: &mut StandaloneLaunchState,
) {
    if plan.candidates.is_empty() {
        widgets::empty_state(
            ui,
            "No launch options found",
            "No installed RetroArch core is a candidate for this game's platform yet.",
            None,
        );
        return;
    }
    for candidate in &plan.candidates {
        ui.add_space(6.0);
        show_candidate(
            ui,
            plan,
            candidate,
            dolphin,
            pcsx2,
            duckstation,
            ppsspp,
            rpcs3,
            xemu,
            xenia,
            retroarch_launch_state,
            dolphin_launch_state,
            pcsx2_launch_state,
            standalone_launch_state,
        );
    }
}

fn readiness_label_and_tone(readiness: LaunchReadiness) -> (&'static str, widgets::StatusTone) {
    match readiness {
        LaunchReadiness::Ready => ("Ready", widgets::StatusTone::Success),
        LaunchReadiness::ReadyWithWarnings => ("Ready with warnings", widgets::StatusTone::Warning),
        LaunchReadiness::Blocked => ("Blocked", widgets::StatusTone::Blocked),
    }
}

fn preference_label(preference: CandidatePreference) -> &'static str {
    match preference {
        CandidatePreference::Remembered => "Remembered",
        CandidatePreference::SoleEligible => "Sole eligible",
        CandidatePreference::Undetermined => "Choice needed",
    }
}

fn firmware_label_and_tone(firmware: FirmwareReadiness) -> (&'static str, widgets::StatusTone) {
    match firmware {
        FirmwareReadiness::Verified => ("Verified", widgets::StatusTone::Success),
        FirmwareReadiness::PresentUnverified => {
            ("Present but unverified", widgets::StatusTone::Warning)
        }
        FirmwareReadiness::Missing => ("Missing", widgets::StatusTone::Blocked),
        FirmwareReadiness::Unknown => ("Unknown", widgets::StatusTone::Pending),
        FirmwareReadiness::NotRequired => ("Not required", widgets::StatusTone::Info),
    }
}

/// `(name, profile description)` for one candidate's target - never the
/// command that would launch it, only what it is.
fn target_labels(target: &LaunchTarget) -> (String, String) {
    match target {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } => (adapter_id.to_string(), format!("Profile {profile_id}")),
        LaunchTarget::RetroArchCore {
            profile, core_stem, ..
        } => (
            core_stem.clone(),
            format!(
                "RetroArch ({:?} / {:?})",
                profile.profile_kind, profile.scope
            ),
        ),
    }
}

fn standalone_launch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xemu: Option<&XemuLaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
) -> Option<StandaloneLaunchRequest> {
    if candidate.readiness != LaunchReadiness::Ready
        || !candidate.blockers.is_empty()
        || !candidate.warnings.is_empty()
        || candidate.content.requires_mount
        || candidate.content.container != Some(LaunchContainerKind::PlainFile)
    {
        return None;
    }
    let path = candidate.content.resolved_path.clone()?;
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        return None;
    };
    let platform = plan.platform_id.clone()?;
    let game_key = plan.game_key.clone()?;
    match *adapter_id {
        "duckstation" => {
            let context = duckstation?;
            let serial = context.verified_ps1_serial.clone()?;
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?;
            let binding = archivefs_core::patch_manager::resolve_duckstation_native_launch_binding(
                profile,
                &context.roots,
            )
            .ok()?;
            Some(StandaloneLaunchRequest::DuckStation(
                DuckStationLaunchRequest {
                    selected_content_path: path,
                    expected_platform_id: platform,
                    expected_game_key: game_key,
                    expected_ps1_serial: serial,
                    profile_id: profile.profile_id.clone(),
                    expected_executable: binding.executable,
                    expected_user_directory_mode: binding.user_directory_mode,
                },
                context.roots.clone(),
                context.firmware_evidence.clone(),
            ))
        }
        "ppsspp" => {
            let context = ppsspp?;
            let disc_id = context.verified_psp_disc_id.clone()?;
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?;
            let binding =
                archivefs_core::patch_manager::resolve_ppsspp_native_launch_binding(profile)
                    .ok()?;
            Some(StandaloneLaunchRequest::Ppsspp(
                PpssppLaunchRequest {
                    selected_content_path: path,
                    expected_platform_id: platform,
                    expected_game_key: game_key,
                    expected_psp_disc_id: disc_id,
                    profile_id: profile.profile_id.clone(),
                    expected_executable: binding.executable,
                },
                context.roots.clone(),
            ))
        }
        "rpcs3" => {
            let context = rpcs3?;
            let title_id = context.verified_ps3_title_id.clone()?;
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?;
            let binding =
                archivefs_core::patch_manager::resolve_rpcs3_native_launch_binding(profile).ok()?;
            Some(StandaloneLaunchRequest::Rpcs3(
                Rpcs3LaunchRequest {
                    selected_content_path: path,
                    expected_platform_id: platform,
                    expected_game_key: game_key,
                    expected_ps3_title_id: title_id,
                    profile_id: profile.profile_id.clone(),
                    expected_executable: binding.executable,
                },
                context.roots.clone(),
            ))
        }
        "xemu" => {
            let context = xemu?;
            let title_id = context.verified_xbox_title_id.clone()?;
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?;
            let binding =
                archivefs_core::patch_manager::resolve_xemu_native_launch_binding(profile).ok()?;
            Some(StandaloneLaunchRequest::Xemu(
                XemuLaunchRequest {
                    selected_content_path: path,
                    expected_platform_id: platform,
                    expected_game_key: game_key,
                    expected_xbox_title_id: title_id,
                    profile_id: profile.profile_id.clone(),
                    expected_executable: binding.executable,
                },
                context.roots.clone(),
            ))
        }
        "xenia" => {
            let context = xenia?;
            if context.verified_xex_title_id.is_none() && context.verified_xex_media_id.is_none() {
                return None;
            }
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?;
            let binding =
                archivefs_core::patch_manager::resolve_xenia_launch_binding(profile).ok()?;
            Some(StandaloneLaunchRequest::Xenia(
                XeniaLaunchRequest {
                    selected_content_path: path,
                    expected_platform_id: platform,
                    expected_game_key: game_key,
                    expected_xex_title_id: context.verified_xex_title_id.clone(),
                    expected_xex_media_id: context.verified_xex_media_id.clone(),
                    profile_id: profile.profile_id.clone(),
                    expected_executable: binding.executable,
                },
                context.roots.clone(),
            ))
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn typed_launch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xemu: Option<&XemuLaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
) -> Option<TypedLaunchRequest> {
    if let Some(request) = retroarch_launch_request(plan, candidate) {
        return Some(TypedLaunchRequest::RetroArch(request));
    }
    if let Some(context) = dolphin
        && let Some(request) = dolphin_launch_request(plan, candidate, context)
    {
        return Some(TypedLaunchRequest::Dolphin(request));
    }
    if let Some(context) = pcsx2
        && let Some(request) = pcsx2_launch_request(plan, candidate, context)
    {
        return Some(TypedLaunchRequest::Pcsx2(
            request,
            context.firmware_evidence.clone(),
        ));
    }
    standalone_launch_request(plan, candidate, duckstation, ppsspp, rpcs3, xemu, xenia)
        .map(TypedLaunchRequest::Standalone)
}

fn candidate_emulator_name(candidate: &LaunchCandidate) -> Option<&'static str> {
    match &candidate.target {
        LaunchTarget::Standalone { adapter_id, .. } => match *adapter_id {
            "duckstation" => Some("DuckStation"),
            "pcsx2" => Some("PCSX2"),
            "ppsspp" => Some("PPSSPP"),
            "rpcs3" => Some("RPCS3"),
            "xemu" => Some("xemu"),
            "xenia" => Some("Xenia"),
            "dolphin" => Some("Dolphin"),
            _ => None,
        },
        LaunchTarget::RetroArchCore { .. } => Some("RetroArch"),
    }
}

fn gamer_blocker_for_plan(
    plan: &LaunchPlan,
    retroarch_scanned: bool,
    standalone_scans_complete: bool,
) -> GamerBlocker {
    let platform_has_standalone = plan
        .platform_id
        .as_deref()
        .and_then(archivefs_core::launch::launch_compatibility_for_platform)
        .is_some_and(|compatibility| !compatibility.standalone_adapters.is_empty());

    if let Some(candidate) = plan.candidates.iter().find(|candidate| {
        candidate.blockers.iter().any(|blocker| {
            matches!(
                blocker.kind,
                LaunchBlockerKind::ContentNotResolved
                    | LaunchBlockerKind::DolphinContentFormatUnsupported
                    | LaunchBlockerKind::Pcsx2ContentFormatUnsupported
                    | LaunchBlockerKind::DuckStationContentFormatUnsupported
                    | LaunchBlockerKind::PpssppContentFormatUnsupported
                    | LaunchBlockerKind::Rpcs3ContentFormatUnsupported
                    | LaunchBlockerKind::XemuContentFormatUnsupported
                    | LaunchBlockerKind::XeniaContentFormatUnsupported
            )
        })
    }) {
        let detail = candidate
            .blockers
            .iter()
            .find(|blocker| {
                matches!(
                    blocker.kind,
                    LaunchBlockerKind::ContentNotResolved
                        | LaunchBlockerKind::DolphinContentFormatUnsupported
                        | LaunchBlockerKind::Pcsx2ContentFormatUnsupported
                        | LaunchBlockerKind::DuckStationContentFormatUnsupported
                        | LaunchBlockerKind::PpssppContentFormatUnsupported
                        | LaunchBlockerKind::Rpcs3ContentFormatUnsupported
                        | LaunchBlockerKind::XemuContentFormatUnsupported
                        | LaunchBlockerKind::XeniaContentFormatUnsupported
                )
            })
            .map(|blocker| blocker.detail.clone())
            .unwrap_or_else(|| "game content needs preparation".into());
        return GamerBlocker::new(GamerBlockerKind::ContentNeedsPreparation, detail);
    }

    if let Some(candidate) = plan.candidates.iter().find(|candidate| {
        candidate.preference == CandidatePreference::Undetermined
            && (candidate
                .blockers
                .iter()
                .any(|blocker| matches!(blocker.kind, LaunchBlockerKind::AmbiguousCore))
                || candidate
                    .warnings
                    .iter()
                    .any(|warning| warning.kind == LaunchWarningKind::MultipleEligibleProfiles))
    }) {
        let detail = candidate
            .blockers
            .first()
            .map(|blocker| blocker.detail.clone())
            .or_else(|| {
                candidate
                    .warnings
                    .first()
                    .map(|warning| warning.detail.clone())
            })
            .unwrap_or_else(|| "more than one safe launch choice is available".into());
        return GamerBlocker::new(GamerBlockerKind::MultipleChoices, detail);
    }

    if let Some(candidate) = plan.candidates.iter().find(|candidate| {
        candidate
            .blockers
            .iter()
            .any(|blocker| blocker.kind == LaunchBlockerKind::NoInstallationCandidate)
    }) {
        if !standalone_scans_complete || (!retroarch_scanned && !platform_has_standalone) {
            return GamerBlocker::new(
                GamerBlockerKind::EmulatorNotChecked,
                "Emulator readiness has not been checked yet.",
            );
        }
        if let Some(platform) = plan.platform_id.as_deref()
            && let Some(compatibility) =
                archivefs_core::launch::launch_compatibility_for_platform(platform)
            && compatibility.standalone_adapters.len() == 1
        {
            let emulator = match compatibility.standalone_adapters[0] {
                "pcsx2" => "PCSX2",
                "duckstation" => "DuckStation",
                "ppsspp" => "PPSSPP",
                "rpcs3" => "RPCS3",
                "xemu" => "xemu",
                "xenia" => "Xenia",
                "dolphin" => "Dolphin",
                adapter => adapter,
            };
            return GamerBlocker::for_emulator(
                GamerBlockerKind::EmulatorNotInstalled,
                emulator,
                candidate
                    .blockers
                    .first()
                    .map(|blocker| blocker.detail.clone())
                    .unwrap_or_else(|| "no installed profile was found".into()),
            );
        }
    }

    if let Some(candidate) = plan
        .candidates
        .iter()
        .find(|candidate| !candidate.blockers.is_empty())
    {
        let blocker = candidate.blockers.first().expect("non-empty blockers");
        let emulator = candidate_emulator_name(candidate);
        if matches!(
            blocker.kind,
            LaunchBlockerKind::ProfileIneligible
                | LaunchBlockerKind::RequiredFirmwareMissing
                | LaunchBlockerKind::CoreMissing
                | LaunchBlockerKind::RetroArchProfileMissing
                | LaunchBlockerKind::AmbiguousRetroArchProfile
                | LaunchBlockerKind::RetroArchCoreMismatch
                | LaunchBlockerKind::RetroArchExecutableMissing
                | LaunchBlockerKind::AmbiguousRetroArchExecutable
                | LaunchBlockerKind::RetroArchPathNotExact
                | LaunchBlockerKind::DolphinBindingUnavailable
                | LaunchBlockerKind::Pcsx2BindingUnavailable
                | LaunchBlockerKind::DuckStationBindingUnavailable
                | LaunchBlockerKind::PpssppBindingUnavailable
                | LaunchBlockerKind::Rpcs3BindingUnavailable
                | LaunchBlockerKind::XemuBindingUnavailable
                | LaunchBlockerKind::XeniaBindingUnavailable
        ) {
            return match emulator {
                Some(emulator) => GamerBlocker::for_emulator(
                    GamerBlockerKind::EmulatorSetupIncomplete,
                    emulator,
                    blocker.detail.clone(),
                ),
                None => GamerBlocker::new(GamerBlockerKind::NoSafeEmulator, blocker.detail.clone()),
            };
        }
        return GamerBlocker::new(GamerBlockerKind::LaunchPlanInvalid, blocker.detail.clone());
    }

    if !standalone_scans_complete || (!retroarch_scanned && !platform_has_standalone) {
        return GamerBlocker::new(
            GamerBlockerKind::EmulatorNotChecked,
            "RetroArch has not been checked yet, and no standalone launch is ready.",
        );
    }
    GamerBlocker::new(
        GamerBlockerKind::NoSafeEmulator,
        "No safe emulator launch candidate is available.",
    )
}

// One immediate-mode render call: the draw target, the plan/candidate being
// drawn, the read-only per-emulator contexts, and the mutable per-emulator
// launch states it toggles. A parameter struct would only rename the same
// arguments.
#[allow(clippy::too_many_arguments)]
fn show_candidate(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xemu: Option<&XemuLaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
    retroarch_launch_state: &mut RetroArchLaunchState,
    dolphin_launch_state: &mut DolphinLaunchState,
    pcsx2_launch_state: &mut Pcsx2LaunchState,
    standalone_launch_state: &mut StandaloneLaunchState,
) {
    widgets::card(ui, |ui| {
        let (name, profile) = target_labels(&candidate.target);
        let (readiness_label, readiness_tone) = readiness_label_and_tone(candidate.readiness);

        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(ui, readiness_label, readiness_tone);
            ui.label(egui::RichText::new(&name).strong().size(15.0));
        });
        ui.label(
            egui::RichText::new(&profile)
                .small()
                .color(theme::muted(ui)),
        );
        ui.label(
            egui::RichText::new(format!(
                "Preference: {}",
                preference_label(candidate.preference)
            ))
            .small()
            .color(theme::muted(ui)),
        );

        let (firmware_label, firmware_tone) = firmware_label_and_tone(candidate.firmware);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Firmware/BIOS:").small());
            widgets::status_badge(ui, firmware_label, firmware_tone);
        });

        for blocker in &candidate.blockers {
            show_blocker(ui, blocker);
        }
        for warning in &candidate.warnings {
            show_warning(ui, warning);
        }

        widgets::technical_details(ui, (&name, &profile), |ui| {
            detail_label(
                ui,
                "Content resolved",
                &candidate.content.has_runnable_path().to_string(),
            );
            detail_label(
                ui,
                "Requires mount",
                &candidate.content.requires_mount.to_string(),
            );
            detail_label(ui, "Content provenance", &candidate.content.provenance);
        });

        if let Some(request) = retroarch_launch_request(plan, candidate) {
            ui.add_space(6.0);
            show_launch_action(ui, retroarch_launch_state, request);
        }
        if let Some(context) = dolphin
            && let Some(request) = dolphin_launch_request(plan, candidate, context)
        {
            ui.add_space(6.0);
            show_dolphin_launch_action(ui, dolphin_launch_state, request);
        }
        if let Some(context) = pcsx2
            && let Some(request) = pcsx2_launch_request(plan, candidate, context)
        {
            ui.add_space(6.0);
            show_pcsx2_launch_action(
                ui,
                pcsx2_launch_state,
                request,
                context.firmware_evidence.clone(),
            );
        }
        if let Some(request) =
            standalone_launch_request(plan, candidate, duckstation, ppsspp, rpcs3, xemu, xenia)
        {
            ui.add_space(6.0);
            show_standalone_launch_action(ui, standalone_launch_state, request);
        }
    });
}

fn show_standalone_launch_action(
    ui: &mut egui::Ui,
    state: &mut StandaloneLaunchState,
    request: StandaloneLaunchRequest,
) {
    let (path, adapter) = request.key();
    let same = state
        .tracked
        .as_ref()
        .is_some_and(|(p, a, _)| *p == path && *a == adapter);
    if same {
        let label = match state.tracked.as_ref().map(|(_, _, stage)| stage) {
            Some(StandaloneLaunchStage::Starting(_)) => "Launching…",
            Some(StandaloneLaunchStage::Running(_)) => "Running",
            Some(StandaloneLaunchStage::Exited(_)) => "Process exited",
            Some(StandaloneLaunchStage::Failed(_)) => "Launch failed",
            None => "Launch",
        };
        ui.label(label);
        if let Some((_, _, StandaloneLaunchStage::Failed(detail))) = state.tracked.as_ref() {
            ui.label(egui::RichText::new(detail).small().color(theme::muted(ui)));
        }
        return;
    }
    if ui.button(format!("Launch {adapter}")).clicked() {
        state.start(request);
    }
}

/// Renders the user-facing "Play" action for one eligible candidate while
/// retaining the adapter name for technical clarity,
/// entirely from an owned snapshot of `launch_state` so the click handler
/// below never needs to mutate `launch_state` while a borrow of it is still
/// live from a `match`.
fn show_launch_action(
    ui: &mut egui::Ui,
    launch_state: &mut RetroArchLaunchState,
    request: RetroArchLaunchRequest,
) {
    let display = launch_state.display_for(&request);
    show_retroarch_launch_feedback(ui, &display);

    match display {
        RetroArchLaunchDisplay::Idle => {
            if ui.button("Play — Launch RetroArch").clicked() {
                launch_state.start(request);
            }
        }
        RetroArchLaunchDisplay::Starting => {
            ui.add_enabled(false, egui::Button::new("Starting RetroArch…"));
        }
        RetroArchLaunchDisplay::Running { .. } => {
            ui.add_enabled(false, egui::Button::new("Play — Launch RetroArch"));
        }
        RetroArchLaunchDisplay::Exited { .. } => {
            if ui.button("Play — Launch RetroArch").clicked() {
                launch_state.start(request);
            }
        }
        RetroArchLaunchDisplay::Failed { .. } => {
            if ui.button("Play — Launch RetroArch").clicked() {
                launch_state.start(request);
            }
        }
    }
}

/// Shared launch outcome/status renderer. It deliberately owns no button and
/// cannot start a process; each view keeps its own control styling while both
/// expose the exact same executor result and actionable failure detail.
pub(crate) fn show_retroarch_launch_feedback(ui: &mut egui::Ui, display: &RetroArchLaunchDisplay) {
    match display {
        RetroArchLaunchDisplay::Idle | RetroArchLaunchDisplay::Starting => {}
        RetroArchLaunchDisplay::Running { pid } => {
            widgets::status_badge(ui, "RetroArch running", widgets::StatusTone::Success);
            ui.label(egui::RichText::new(format!("PID {pid}")).small());
        }
        RetroArchLaunchDisplay::Exited {
            success,
            status_detail,
            stderr_tail,
        } => {
            if *success {
                widgets::banner(
                    ui,
                    "RetroArch exited",
                    "RetroArch closed normally.",
                    widgets::StatusTone::Success,
                );
            } else {
                widgets::banner(
                    ui,
                    "RetroArch exited",
                    &format!("RetroArch exited unexpectedly ({status_detail})."),
                    widgets::StatusTone::Warning,
                );
                if let Some(stderr_tail) = stderr_tail {
                    widgets::technical_details(ui, "retroarch-exit-stderr", |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(stderr_tail).monospace()).wrap(),
                        );
                    });
                }
            }
        }
        RetroArchLaunchDisplay::Failed { message, detail } => {
            widgets::banner(ui, "Launch failed", message, widgets::StatusTone::Blocked);
            widgets::technical_details(ui, "retroarch-launch-error", |ui| {
                ui.add(egui::Label::new(egui::RichText::new(detail).monospace()).wrap());
            });
        }
    }
}

/// Renders the "Launch Dolphin" action for one eligible candidate - the
/// same structure as [`show_launch_action`], for [`DolphinLaunchState`].
fn show_dolphin_launch_action(
    ui: &mut egui::Ui,
    launch_state: &mut DolphinLaunchState,
    request: DolphinLaunchRequest,
) {
    enum Display {
        Idle,
        Starting,
        Running {
            pid: u32,
        },
        Exited {
            success: bool,
            status_detail: String,
            stderr_tail: Option<String>,
        },
        Failed {
            message: &'static str,
            detail: String,
        },
    }

    let this_key = DolphinLaunchKey::from_request(&request);
    let display = match launch_state.tracked.as_mut() {
        Some((key, stage)) if *key == this_key => match stage {
            DolphinLaunchStage::Starting { .. } => Display::Starting,
            DolphinLaunchStage::Running { process } => Display::Running { pid: process.pid },
            DolphinLaunchStage::Exited { process } => {
                let report: &DolphinLaunchExitReport = process
                    .poll()
                    .expect("Exited stage always has a cached exit report");
                let success = matches!(&report.status, Ok(status) if status.success());
                let status_detail = match &report.status {
                    Ok(status) => format!("{status}"),
                    Err(error) => format!("wait() failed: {error}"),
                };
                let stderr_tail = (!success && !report.stderr.is_empty())
                    .then(|| String::from_utf8_lossy(&report.stderr).into_owned());
                Display::Exited {
                    success,
                    status_detail,
                    stderr_tail,
                }
            }
            DolphinLaunchStage::Failed { error } => {
                let (message, detail) = dolphin_launch_error_message(error);
                Display::Failed { message, detail }
            }
        },
        _ => Display::Idle,
    };

    match display {
        Display::Idle => {
            if ui.button("Launch Dolphin").clicked() {
                launch_state.start(request);
            }
        }
        Display::Starting => {
            ui.add_enabled(false, egui::Button::new("Starting Dolphin…"));
        }
        Display::Running { pid } => {
            widgets::status_badge(ui, "Dolphin running", widgets::StatusTone::Success);
            ui.label(egui::RichText::new(format!("PID {pid}")).small());
            ui.add_enabled(false, egui::Button::new("Launch Dolphin"));
        }
        Display::Exited {
            success,
            status_detail,
            stderr_tail,
        } => {
            if success {
                widgets::banner(
                    ui,
                    "Dolphin exited",
                    "Dolphin closed normally.",
                    widgets::StatusTone::Success,
                );
            } else {
                widgets::banner(
                    ui,
                    "Dolphin exited",
                    &format!("Dolphin exited unexpectedly ({status_detail})."),
                    widgets::StatusTone::Warning,
                );
                if let Some(stderr_tail) = stderr_tail {
                    widgets::technical_details(ui, "dolphin-exit-stderr", |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(stderr_tail).monospace()).wrap(),
                        );
                    });
                }
            }
            if ui.button("Launch Dolphin").clicked() {
                launch_state.start(request);
            }
        }
        Display::Failed { message, detail } => {
            widgets::banner(ui, "Launch failed", message, widgets::StatusTone::Blocked);
            widgets::technical_details(ui, "dolphin-launch-error", |ui| {
                ui.add(egui::Label::new(egui::RichText::new(detail).monospace()).wrap());
            });
            if ui.button("Launch Dolphin").clicked() {
                launch_state.start(request);
            }
        }
    }
}

/// Renders the "Launch PCSX2" action for one eligible candidate - the same
/// structure as [`show_dolphin_launch_action`], for [`Pcsx2LaunchState`].
/// `firmware_evidence` is the already-loaded PS2 BIOS evidence snapshot,
/// only ever cloned here to move into the click handler's background
/// thread - see [`Pcsx2LaunchState::start`].
fn show_pcsx2_launch_action(
    ui: &mut egui::Ui,
    launch_state: &mut Pcsx2LaunchState,
    request: Pcsx2LaunchRequest,
    firmware_evidence: Vec<FirmwareIdentityRecord>,
) {
    enum Display {
        Idle,
        Starting,
        Running {
            pid: u32,
        },
        Exited {
            success: bool,
            status_detail: String,
            stderr_tail: Option<String>,
        },
        Failed {
            message: &'static str,
            detail: String,
        },
    }

    let this_key = Pcsx2LaunchKey::from_request(&request);
    let display = match launch_state.tracked.as_mut() {
        Some((key, stage)) if *key == this_key => match stage {
            Pcsx2LaunchStage::Starting { .. } => Display::Starting,
            Pcsx2LaunchStage::Running { process } => Display::Running { pid: process.pid },
            Pcsx2LaunchStage::Exited { process } => {
                let report: &Pcsx2LaunchExitReport = process
                    .poll()
                    .expect("Exited stage always has a cached exit report");
                let success = matches!(&report.status, Ok(status) if status.success());
                let status_detail = match &report.status {
                    Ok(status) => format!("{status}"),
                    Err(error) => format!("wait() failed: {error}"),
                };
                let stderr_tail = (!success && !report.stderr.is_empty())
                    .then(|| String::from_utf8_lossy(&report.stderr).into_owned());
                Display::Exited {
                    success,
                    status_detail,
                    stderr_tail,
                }
            }
            Pcsx2LaunchStage::Failed { error } => {
                let (message, detail) = pcsx2_launch_error_message(error);
                Display::Failed { message, detail }
            }
        },
        _ => Display::Idle,
    };

    match display {
        Display::Idle => {
            if ui.button("Launch PCSX2").clicked() {
                launch_state.start(request, firmware_evidence);
            }
        }
        Display::Starting => {
            ui.add_enabled(false, egui::Button::new("Starting PCSX2…"));
        }
        Display::Running { pid } => {
            widgets::status_badge(ui, "PCSX2 running", widgets::StatusTone::Success);
            ui.label(egui::RichText::new(format!("PID {pid}")).small());
            ui.add_enabled(false, egui::Button::new("Launch PCSX2"));
        }
        Display::Exited {
            success,
            status_detail,
            stderr_tail,
        } => {
            if success {
                widgets::banner(
                    ui,
                    "PCSX2 exited",
                    "PCSX2 closed normally.",
                    widgets::StatusTone::Success,
                );
            } else {
                widgets::banner(
                    ui,
                    "PCSX2 exited",
                    &format!("PCSX2 exited unexpectedly ({status_detail})."),
                    widgets::StatusTone::Warning,
                );
                if let Some(stderr_tail) = stderr_tail {
                    widgets::technical_details(ui, "pcsx2-exit-stderr", |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(stderr_tail).monospace()).wrap(),
                        );
                    });
                }
            }
            if ui.button("Launch PCSX2").clicked() {
                launch_state.start(request, firmware_evidence);
            }
        }
        Display::Failed { message, detail } => {
            widgets::banner(ui, "Launch failed", message, widgets::StatusTone::Blocked);
            widgets::technical_details(ui, "pcsx2-launch-error", |ui| {
                ui.add(egui::Label::new(egui::RichText::new(detail).monospace()).wrap());
            });
            if ui.button("Launch PCSX2").clicked() {
                launch_state.start(request, firmware_evidence);
            }
        }
    }
}

fn show_blocker(ui: &mut egui::Ui, blocker: &LaunchBlocker) {
    ui.horizontal_wrapped(|ui| {
        widgets::status_badge(ui, "Blocked", widgets::StatusTone::Blocked);
        ui.label(egui::RichText::new(&blocker.detail).small());
    });
}

fn show_warning(ui: &mut egui::Ui, warning: &LaunchWarning) {
    ui.horizontal_wrapped(|ui| {
        widgets::status_badge(ui, "Warning", widgets::StatusTone::Warning);
        ui.label(egui::RichText::new(&warning.detail).small());
    });
}

fn detail_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_sized(
            [140.0, 0.0],
            egui::Label::new(egui::RichText::new(label).strong()),
        );
        ui.add(egui::Label::new(value).wrap());
    });
}

#[cfg(test)]
mod tests;
