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
//! # Launch RetroArch (Phase 1)
//!
//! Exactly one explicit action exists here: a "Launch RetroArch" button,
//! shown only for a candidate within the narrow slice
//! [`archivefs_core::launch::execution`] already supports (see
//! [`retroarch_launch_request`] for the exact eligibility rule, which is
//! also what builds the [`RetroArchLaunchRequest`] sent to core - the same
//! function decides both, so the button can never show for a request core
//! would refuse). The click itself is the user's authorization; no second
//! confirmation dialog is shown. Preflight and spawn happen on a
//! background thread via [`preflight_and_launch_retroarch`] and are polled
//! non-blockingly through [`RetroArchLaunchState::poll`] - this module
//! never builds a command line or trusts the plan's cached readiness as
//! execution authority; core re-validates everything fresh.
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
//! - It never launches a Flatpak/AppImage/standalone-emulator candidate,
//!   archive/mounted content, or a `ReadyWithWarnings`/`Blocked` candidate
//!   - see [`retroarch_launch_request`].
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
    DolphinLaunchSpawnError, FirmwareReadiness, LaunchBlocker, LaunchCandidate,
    LaunchContainerKind, LaunchExecutionError, LaunchExitReport, LaunchPlan,
    LaunchPreflightErrorKind, LaunchReadiness, LaunchSpawnError, LaunchTarget, LaunchWarning,
    LaunchedDolphinProcess, LaunchedPcsx2Process, LaunchedRetroArchProcess,
    PCSX2_SUPPORTED_PLATFORM_ID, Pcsx2LaunchExecutionError, Pcsx2LaunchExitReport,
    Pcsx2LaunchPreflightErrorKind, Pcsx2LaunchRequest, Pcsx2LaunchSpawnError,
    RetroArchLaunchRequest, preflight_and_launch_dolphin, preflight_and_launch_pcsx2,
    preflight_and_launch_retroarch,
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
    RetroArchNotScanned,
    /// `CanonicalIdentityStatus::Unknown` - identity could not be resolved
    /// at all.
    IdentityUnknown,
    /// `CanonicalIdentityStatus::Conflicting` - identity evidence conflicts.
    IdentityConflicting,
    /// Identity was resolved and a real [`LaunchPlan`] was built.
    Plan {
        plan: LaunchPlan,
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
    },
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

    fn start(&mut self, request: RetroArchLaunchRequest) {
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
) {
    widgets::section_header(
        ui,
        "Launch readiness",
        Some("Ways this game can be played."),
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
        } => show_plan(
            ui,
            plan,
            dolphin.as_ref(),
            pcsx2.as_ref(),
            retroarch_launch_state,
            dolphin_launch_state,
            pcsx2_launch_state,
        ),
    }
}

fn show_plan(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    retroarch_launch_state: &mut RetroArchLaunchState,
    dolphin_launch_state: &mut DolphinLaunchState,
    pcsx2_launch_state: &mut Pcsx2LaunchState,
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
            retroarch_launch_state,
            dolphin_launch_state,
            pcsx2_launch_state,
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
    retroarch_launch_state: &mut RetroArchLaunchState,
    dolphin_launch_state: &mut DolphinLaunchState,
    pcsx2_launch_state: &mut Pcsx2LaunchState,
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
    });
}

/// Renders the "Launch RetroArch" action for one eligible candidate,
/// entirely from an owned snapshot of `launch_state` so the click handler
/// below never needs to mutate `launch_state` while a borrow of it is still
/// live from a `match`.
fn show_launch_action(
    ui: &mut egui::Ui,
    launch_state: &mut RetroArchLaunchState,
    request: RetroArchLaunchRequest,
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

    let this_key = RetroArchLaunchKey::from_request(&request);
    // Matched via `as_mut()` (not a shared borrow) because reading the
    // cached exit report of an already-`Exited` process goes through
    // `LaunchedRetroArchProcess::poll`, which - though idempotent once a
    // report is cached - is only ever exposed as `&mut self`. This match's
    // borrow of `launch_state.tracked` ends with it since `display` only
    // ever holds owned data, so the later `launch_state.start(..)` calls
    // below borrow it fresh.
    let display = match launch_state.tracked.as_mut() {
        Some((key, stage)) if *key == this_key => match stage {
            RetroArchLaunchStage::Starting { .. } => Display::Starting,
            RetroArchLaunchStage::Running { process } => Display::Running { pid: process.pid },
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
                Display::Exited {
                    success,
                    status_detail,
                    stderr_tail,
                }
            }
            RetroArchLaunchStage::Failed { error } => {
                let (message, detail) = launch_error_message(error);
                Display::Failed { message, detail }
            }
        },
        _ => Display::Idle,
    };

    match display {
        Display::Idle => {
            if ui.button("Launch RetroArch").clicked() {
                launch_state.start(request);
            }
        }
        Display::Starting => {
            ui.add_enabled(false, egui::Button::new("Starting RetroArch…"));
        }
        Display::Running { pid } => {
            widgets::status_badge(ui, "RetroArch running", widgets::StatusTone::Success);
            ui.label(egui::RichText::new(format!("PID {pid}")).small());
            ui.add_enabled(false, egui::Button::new("Launch RetroArch"));
        }
        Display::Exited {
            success,
            status_detail,
            stderr_tail,
        } => {
            if success {
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
            if ui.button("Launch RetroArch").clicked() {
                launch_state.start(request);
            }
        }
        Display::Failed { message, detail } => {
            widgets::banner(ui, "Launch failed", message, widgets::StatusTone::Blocked);
            widgets::technical_details(ui, "retroarch-launch-error", |ui| {
                ui.add(egui::Label::new(egui::RichText::new(detail).monospace()).wrap());
            });
            if ui.button("Launch RetroArch").clicked() {
                launch_state.start(request);
            }
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
