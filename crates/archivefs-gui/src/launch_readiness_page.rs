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
//! - It never calls any ES-DE export/write function. For RetroArch and PCSX2,
//!   the read-only recipe uses the same typed command builders as preflight;
//!   final launch still performs fresh revalidation and rebuilds the command.
//! - It never resolves identity, mounts an archive, or guesses an inner
//!   archive member - see the module doc comment on
//!   [`archivefs_core::launch::evidence_bridge`] for where those honest
//!   fail-closed rules actually live; this module only renders whatever
//!   that bridge already decided.
//! - It never launches a Flatpak candidate, archive/mounted
//!   content, or a `Blocked` candidate. shadPS4 alone accepts its sealed
//!   `ReadyWithWarnings` plan with explicit unverified-compatibility warnings. Standalone
//!   candidates are limited to adapters with an existing typed GUI executor.
//! - It never exposes a Stop/Kill action and never automatically relaunches
//!   a process that has exited.

mod dolphin_profile_guidance;

#[path = "launch_readiness_page/cemu.rs"]
pub(crate) mod cemu;
mod classic_pokes;
pub(crate) mod controller_readiness;
pub(crate) mod dosbox;
#[path = "launch_readiness_page/flycast.rs"]
pub(crate) mod flycast;
mod melonds_cheats;
mod retroarch_cheat_sets;
mod retroarch_cheats;
mod scummvm_profile;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord;
use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::emulator_environment::retroarch::{
    AppImageIdentificationConfidence, DiscoveryEnvironment, ExecutableState, ProfileKind,
    ProfileRef, RetroArchEnvironmentReport,
};
use archivefs_core::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use archivefs_core::launch::{
    AmigaWHDLoadLaunchError, DuckStationLaunchExecutionError, DuckStationLaunchRequest,
    LaunchedAmigaWHDLoadProcess, LaunchedDuckStationProcess, LaunchedPpssppProcess,
    LaunchedRpcs3Process, LaunchedXemuProcess, LaunchedXeniaProcess, PpssppLaunchExecutionError,
    PpssppLaunchRequest, Rpcs3LaunchExecutionError, Rpcs3LaunchRequest, WHDLoadLaunchInput,
    XemuLaunchExecutionError, XemuLaunchRequest, XeniaLaunchExecutionError, XeniaLaunchRequest,
    build_amiberry_whdload_command_plan, build_dolphin_command_plan,
    build_duckstation_command_plan, build_fsuae_whdload_command_plan, build_pcsx2_command_plan,
    build_ppsspp_command_plan, build_retroarch_command_plan, build_rpcs3_command_plan,
    build_xenia_command_plan, preflight_and_launch_amiga_whdload, preflight_and_launch_openmsx,
    preflight_and_launch_ppsspp, preflight_and_launch_rpcs3, preflight_and_launch_xemu,
    preflight_and_launch_xenia,
};
use archivefs_core::launch::{
    CandidatePreference, DOLPHIN_SUPPORTED_PLATFORM_ID, DolphinLaunchExecutionError,
    DolphinLaunchExitReport, DolphinLaunchPreflightErrorKind, DolphinLaunchRequest,
    DolphinLaunchSpawnError, FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchCandidate,
    LaunchCommandSpec, LaunchContainerKind, LaunchExecutionError, LaunchExitReport, LaunchPlan,
    LaunchPreflightErrorKind, LaunchReadiness, LaunchSpawnError, LaunchTarget, LaunchWarning,
    LaunchWarningKind, LaunchedDolphinProcess, LaunchedPcsx2Process, LaunchedRetroArchProcess,
    MameCommandPlan, MameLaunchRequest, PCSX2_SUPPORTED_PLATFORM_ID, Pcsx2LaunchExecutionError,
    Pcsx2LaunchExitReport, Pcsx2LaunchPreflightErrorKind, Pcsx2LaunchRequest,
    Pcsx2LaunchSpawnError, RetroArchLaunchRequest, preflight_and_launch_dolphin,
    preflight_and_launch_pcsx2, preflight_mame_launch,
};
use archivefs_core::patch_manager::{
    DolphinLocalDiscoveryRoots, DolphinLocalProfileDiscovery, Pcsx2ProfileDiscovery,
    Pcsx2ProfileDiscoveryRoots, resolve_dolphin_native_launch_binding,
    resolve_pcsx2_native_launch_binding,
};
use archivefs_core::ready_to_play::{
    EmulatorEvidenceState, IdentityEvidenceState, MediaEvidenceState, ModEvidenceState,
    ReadinessEvidenceState, ReadyToPlayEvidence, ReadyToPlayResult, project_ready_to_play,
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
    AtariSt(crate::emulator_setup::hatari::Snapshot),
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
    /// A CUE/BIN disc whose files could not be resolved. The text is the
    /// selected-evidence worker's own diagnostic (which track is missing,
    /// unsafe or ambiguous), never a guess.
    DiscFilesUnresolved(String),
    /// The last scan could not finish reading this game's file, so the saved
    /// identity and platform may be stale. Not a finding about the file; a
    /// rescan can restore readiness.
    InspectionIncomplete,
    /// `CanonicalIdentityStatus::Conflicting` - identity evidence conflicts.
    IdentityConflicting,
    /// Identity was resolved and a real [`LaunchPlan`] was built.
    Plan {
        plan: LaunchPlan,
        retroarch: Option<RetroArchLaunchContext>,
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
        melonds: Option<MelonDsLaunchContext>,
        cemu: Option<cemu::Context>,
        flycast: Option<flycast::Context>,
        amiga_whdload: Option<AmigaWHDLoadLaunchContext>,
        #[cfg(target_os = "linux")]
        shadps4: Option<crate::emulator_setup::shadps4::Snapshot>,
    },
}

/// Projects the already-built launch-readiness input into the shared
/// Ready-to-Play view.  This is deliberately a pure adapter: it performs no
/// discovery and never calls the launch planner.
pub(crate) fn ready_to_play_result(
    input: &LaunchReadinessInput,
    item_identity: impl Into<String>,
) -> ReadyToPlayResult {
    let item_identity = item_identity.into();
    let evidence = match input {
        LaunchReadinessInput::AtariSt(snapshot) => {
            let plan = snapshot.plan();
            let mut evidence = ReadyToPlayEvidence::from_launch_plan(item_identity, &plan);
            evidence.identity = IdentityEvidenceState::Unknown;
            return project_ready_to_play(&evidence);
        }
        LaunchReadinessInput::Plan { plan, .. } => {
            return project_ready_to_play(&ReadyToPlayEvidence::from_launch_plan(
                item_identity,
                plan,
            ));
        }
        LaunchReadinessInput::EvidenceNotLoaded => ReadyToPlayEvidence {
            item_identity,
            launch_plan: None,
            launch_plan_state: ReadinessEvidenceState::NotGathered,
            identity: IdentityEvidenceState::Unknown,
            emulator: EmulatorEvidenceState::Unknown,
            firmware: ReadinessEvidenceState::NotGathered,
            media: MediaEvidenceState::Unknown,
            mod_state: ModEvidenceState::Unknown,
            arcade_dependency_blocked: false,
            save_state_version_sensitive: false,
        },
        LaunchReadinessInput::RetroArchNotScanned => ReadyToPlayEvidence {
            item_identity,
            launch_plan: None,
            launch_plan_state: ReadinessEvidenceState::NotGathered,
            identity: IdentityEvidenceState::Unknown,
            emulator: EmulatorEvidenceState::Unknown,
            firmware: ReadinessEvidenceState::Gathered,
            media: MediaEvidenceState::NotApplicable,
            mod_state: ModEvidenceState::None,
            arcade_dependency_blocked: false,
            save_state_version_sensitive: false,
        },
        LaunchReadinessInput::IdentityUnknown
        | LaunchReadinessInput::DiscFilesUnresolved(_)
        | LaunchReadinessInput::InspectionIncomplete => ReadyToPlayEvidence {
            item_identity,
            launch_plan: None,
            launch_plan_state: ReadinessEvidenceState::Gathered,
            identity: IdentityEvidenceState::Unknown,
            emulator: EmulatorEvidenceState::Unknown,
            firmware: ReadinessEvidenceState::Gathered,
            media: MediaEvidenceState::NotApplicable,
            mod_state: ModEvidenceState::None,
            arcade_dependency_blocked: false,
            save_state_version_sensitive: false,
        },
        LaunchReadinessInput::IdentityConflicting => ReadyToPlayEvidence {
            item_identity,
            launch_plan: None,
            launch_plan_state: ReadinessEvidenceState::Gathered,
            identity: IdentityEvidenceState::Conflicting,
            emulator: EmulatorEvidenceState::Unknown,
            firmware: ReadinessEvidenceState::Gathered,
            media: MediaEvidenceState::NotApplicable,
            mod_state: ModEvidenceState::None,
            arcade_dependency_blocked: false,
            save_state_version_sensitive: false,
        },
    };
    project_ready_to_play(&evidence)
}

/// A navigation request from the read-only launch-readiness presentation.
///
/// The panel does not run a firmware check or a Doctor scan itself. It can
/// only direct a person to the existing diagnostics workflow when already
/// computed firmware evidence needs attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchReadinessPageAction {
    OpenDoctor,
    Navigate(crate::navigation::NavClick),
    ReviewDetails,
}

/// Exact AppImage paths selected from the same RetroArch discovery report as
/// the launch plan. This binds an AppImage request to one reviewed executable;
/// it is not a generic AppImage launcher.
pub(crate) struct RetroArchLaunchContext {
    appimage_executables: Vec<(ProfileRef, PathBuf)>,
    environment: RetroArchEnvironmentReport,
    /// Verified media set(s) of the focused game, for playlist handoff.
    pub(crate) multidisc: Option<crate::multidisc_launch_view::MultiDiscLaunchContext>,
}

pub(crate) fn retroarch_launch_context(
    environment: &RetroArchEnvironmentReport,
) -> RetroArchLaunchContext {
    let mut appimage_executables = Vec::new();
    for profile in &environment.profiles {
        if profile.profile_kind != ProfileKind::AppImage {
            continue;
        }
        let profile_ref = ProfileRef {
            profile_kind: profile.profile_kind,
            scope: profile.scope,
        };
        for candidate in &profile.app_images {
            if candidate.confidence == AppImageIdentificationConfidence::Exact
                && candidate.executable == Some(ExecutableState::Executable)
                && !candidate.path.lossy
            {
                appimage_executables.push((profile_ref, PathBuf::from(&candidate.path.display)));
            }
        }
    }
    RetroArchLaunchContext {
        appimage_executables,
        environment: environment.clone(),
        multidisc: None,
    }
}

impl RetroArchLaunchContext {
    pub(crate) fn with_multidisc(
        mut self,
        multidisc: Option<crate::multidisc_launch_view::MultiDiscLaunchContext>,
    ) -> Self {
        self.multidisc = multidisc;
        self
    }

    /// The installed core's own `.info` `supported_extensions`, if found.
    fn core_extensions(&self, profile: ProfileRef, core_stem: &str) -> Option<&[String]> {
        use archivefs_core::emulator_environment::retroarch::CoreInfoFinding;
        self.environment
            .profiles
            .iter()
            .filter(|p| p.profile_kind == profile.profile_kind && p.scope == profile.scope)
            .flat_map(|p| p.cores.iter())
            .find(|core| core.core_stem == core_stem)
            .and_then(|core| match &core.info {
                CoreInfoFinding::Found {
                    supported_extensions,
                    ..
                } => Some(supported_extensions.as_slice()),
                _ => None,
            })
    }

    fn exact_appimage_executable(&self, profile: ProfileRef) -> Option<PathBuf> {
        let matches: Vec<_> = self
            .appimage_executables
            .iter()
            .filter(|(candidate_profile, _)| *candidate_profile == profile)
            .map(|(_, executable)| executable.clone())
            .collect();
        match matches.as_slice() {
            [executable] => Some(executable.clone()),
            [] | [_, _, ..] => None,
        }
    }
}

/// One exact adapter request selected by the shared launch plan. Gamer View
/// carries this typed value to the existing executor; it never constructs a
/// shell command or re-selects an emulator at click time.
#[derive(Clone)]
pub(crate) enum TypedLaunchRequest {
    RetroArch(
        RetroArchLaunchRequest,
        Option<crate::multidisc_launch_view::PlaylistHandoff>,
    ),
    Dolphin(DolphinLaunchRequest),
    Pcsx2(Pcsx2LaunchRequest, Vec<FirmwareIdentityRecord>),
    Standalone(StandaloneLaunchRequest),
    AmigaWHDLoad(WHDLoadLaunchInput),
}

impl TypedLaunchRequest {
    /// Caveats that must be shown wherever this request is offered as Play:
    /// compatibility or prerequisites the adapter cannot verify. Only the
    /// shadPS4 plan launches while ReadyWithWarnings today.
    pub(crate) fn unverified_warnings(&self) -> Vec<&'static str> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Standalone(StandaloneLaunchRequest::ShadPs4(request)) => {
                request.plan.warnings().to_vec()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn adapter_name(&self) -> &'static str {
        match self {
            Self::RetroArch(..) => "RetroArch",
            Self::Dolphin(_) => "Dolphin",
            Self::Pcsx2(_, _) => "PCSX2",
            Self::Standalone(request) => request.adapter_name(),
            Self::AmigaWHDLoad(request) => match request.profile.emulator {
                archivefs_core::patch_manager::AmigaEmulatorKind::Amiberry => "Amiberry",
                archivefs_core::patch_manager::AmigaEmulatorKind::FsUae => "FS-UAE",
            },
        }
    }

    pub(crate) fn start(
        self,
        retroarch: &mut RetroArchLaunchState,
        dolphin: &mut DolphinLaunchState,
        pcsx2: &mut Pcsx2LaunchState,
        standalone: &mut StandaloneLaunchState,
        amiga_whdload: &mut AmigaWHDLoadLaunchState,
    ) {
        match self {
            Self::RetroArch(request, playlist) => retroarch.start(request, playlist),
            Self::Dolphin(request) => dolphin.start(request),
            Self::Pcsx2(request, evidence) => pcsx2.start(request, evidence),
            Self::Standalone(request) => standalone.start(request),
            Self::AmigaWHDLoad(request) => amiga_whdload.start(request),
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
    InspectionIncomplete,
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
            (GamerBlockerKind::InspectionIncomplete, _) => "Rescan to finish checking".into(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GamerResolverNoticeKind {
    /// The preferred DuckStation was bypassed; a healthy alternative is used.
    PreferredFallback,
    /// The resolver refused: the pinned (or only) choice is unavailable and
    /// nothing else is substituted.
    Blocked,
    /// The selection is what was asked for, but the resolver raised a warning.
    Warning,
}

/// What the shared DuckStation resolver decided, in terms Gamer must show
/// beside Play. `lines` is the resolver's own text (selected installation and
/// profile, why, that the preference was bypassed, and its warnings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GamerResolverNotice {
    pub(crate) kind: GamerResolverNoticeKind,
    /// Compact one-row-each lines shown beside Play: the bypass and the chosen
    /// installation by its last path components, or the refusal.
    pub(crate) summary: Vec<String>,
    /// The resolver's full text (complete paths and reason), reached through
    /// the notice's "Full paths and reason" disclosure.
    pub(crate) lines: Vec<String>,
}

/// The last two path components, so a deep path stays recognisable in one row.
fn compact_path(path: &std::path::Path) -> String {
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let tail = &parts[parts.len().saturating_sub(2)..];
    if tail.is_empty() {
        path.display().to_string()
    } else {
        tail.join("/")
    }
}

impl GamerResolverNotice {
    pub(crate) fn heading(&self) -> &'static str {
        match self.kind {
            GamerResolverNoticeKind::PreferredFallback => {
                "Using a different DuckStation than you preferred"
            }
            GamerResolverNoticeKind::Blocked => {
                "DuckStation is blocked: your choice is unavailable"
            }
            GamerResolverNoticeKind::Warning => "About this DuckStation choice",
        }
    }
}

fn plan_targets_duckstation(plan: &LaunchPlan) -> bool {
    plan.candidates.iter().any(|candidate| {
        matches!(
            &candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "duckstation",
                ..
            }
        )
    })
}

/// The resolver's fallback or refusal for the game on screen, or `None` when
/// the selection was exactly what the user chose (or AUTO found), when the
/// plan has no DuckStation candidate, or when a different emulator is what
/// Play would actually start.
pub(crate) fn gamer_resolver_notice(
    input: &LaunchReadinessInput,
    play_action: &GamerPlayAction,
) -> Option<GamerResolverNotice> {
    use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority as Authority;
    let LaunchReadinessInput::Plan {
        plan, duckstation, ..
    } = input
    else {
        return None;
    };
    let context = duckstation.as_ref()?;
    if !plan_targets_duckstation(plan) {
        return None;
    }
    if let GamerPlayAction::Launch(request) = play_action
        && request.adapter_name() != "DuckStation"
    {
        return None;
    }
    // Gamer's own wording; the facts are the resolver's, untranslated: the
    // selected installation and profile, why, that the preference was
    // bypassed, and the resolver's warnings.
    let notice = match &context.authority {
        Authority::Refused { reason } => GamerResolverNotice {
            kind: GamerResolverNoticeKind::Blocked,
            summary: vec![
                reason.clone(),
                "EmuWiz will not use any other DuckStation installation.".into(),
            ],
            lines: vec![
                reason.clone(),
                "EmuWiz will not use any other DuckStation installation.".into(),
            ],
        },
        Authority::Authorised {
            executable,
            profile_root,
            fallback,
            why,
            notes,
            ..
        } => {
            let mut lines = Vec::new();
            let mut summary = Vec::new();
            if *fallback {
                summary.push(format!(
                    "Your preferred DuckStation is unavailable, so EmuWiz is using {}.",
                    compact_path(executable)
                ));
                summary.push(format!(
                    "{} Your saved preference was not changed.",
                    why.join(" ")
                ));
                lines.push(format!(
                    "Your preferred DuckStation is unavailable, so EmuWiz is using {} with settings in {}.",
                    executable.display(),
                    profile_root.display()
                ));
                lines.extend(why.iter().cloned());
                lines.push("Your saved preference was not changed.".into());
            }
            let notes: Vec<String> = notes
                .iter()
                .filter(|note| note.contains("preferred") || note.contains("could not be read"))
                .cloned()
                .collect();
            lines.extend(notes.iter().cloned());
            summary.extend(notes);
            if lines.is_empty() {
                return None;
            }
            GamerResolverNotice {
                summary,
                kind: if *fallback {
                    GamerResolverNoticeKind::PreferredFallback
                } else {
                    GamerResolverNoticeKind::Warning
                },
                lines,
            }
        }
    };
    Some(notice)
}

/// Projects the same shared launch plan used by Advanced View into Gamer
/// View's single primary action. This is presentation only: a Ready result
/// carries the exact typed request that the selected adapter preflights again
/// before spawning anything.
pub(crate) fn gamer_play_action(input: &LaunchReadinessInput) -> GamerPlayAction {
    let LaunchReadinessInput::Plan {
        plan,
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
        amiga_whdload,
        #[cfg(target_os = "linux")]
        shadps4,
        ..
    } = input
    else {
        return GamerPlayAction::BlockedTyped(match input {
            LaunchReadinessInput::AtariSt(snapshot) => {
                GamerBlocker::new(GamerBlockerKind::NoSafeEmulator, snapshot.reason())
            }
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
            LaunchReadinessInput::DiscFilesUnresolved(detail) => {
                GamerBlocker::new(GamerBlockerKind::ContentNeedsPreparation, detail.clone())
            }
            LaunchReadinessInput::InspectionIncomplete => GamerBlocker::new(
                GamerBlockerKind::InspectionIncomplete,
                archivefs_core::launch::INSPECTION_INCOMPLETE_REASON,
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
    let preferred = |candidate: &&LaunchCandidate| {
        (!has_remembered && !has_sole_eligible)
            || (has_remembered && candidate.preference == CandidatePreference::Remembered)
            || (!has_remembered
                && has_sole_eligible
                && candidate.preference == CandidatePreference::SoleEligible)
    };
    // A multi-disc game whose playlist cannot be prepared safely is blocked
    // with its reason; Gamer never falls through to another emulator that
    // would silently start one disc.
    if let Some(reason) = plan
        .candidates
        .iter()
        .filter(preferred)
        .find_map(|candidate| {
            match multidisc_decision(candidate, retroarch.as_ref(), duckstation.as_ref()) {
                archivefs_core::launch::MultiDiscLaunch::Refused { reason } => Some(reason),
                _ => None,
            }
        })
    {
        return GamerPlayAction::BlockedTyped(GamerBlocker::new(
            GamerBlockerKind::NoSafeEmulator,
            format!("This multi-disc game cannot be launched safely: {reason}."),
        ));
    }
    let requests: Vec<TypedLaunchRequest> = plan
        .candidates
        .iter()
        .filter(preferred)
        .filter_map(|candidate| {
            if let Some(request) = flycast.as_ref().and_then(|c| c.request(plan, candidate)) {
                return Some(TypedLaunchRequest::Standalone(
                    StandaloneLaunchRequest::Flycast(request),
                ));
            }
            if let Some(request) = cemu
                .as_ref()
                .and_then(|context| context.request(plan, candidate))
            {
                return Some(TypedLaunchRequest::Standalone(
                    StandaloneLaunchRequest::Cemu(request),
                ));
            }
            #[cfg(target_os = "linux")]
            if let Some(request) = shadps4
                .as_ref()
                .and_then(|snapshot| snapshot.request(candidate))
            {
                return Some(TypedLaunchRequest::Standalone(
                    StandaloneLaunchRequest::ShadPs4(request),
                ));
            }
            typed_launch_request(
                plan,
                candidate,
                retroarch.as_ref(),
                dolphin.as_ref(),
                pcsx2.as_ref(),
                duckstation.as_ref(),
                ppsspp.as_ref(),
                rpcs3.as_ref(),
                xemu.as_ref(),
                xenia.as_ref(),
                melonds.as_ref(),
                amiga_whdload.as_ref(),
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
    let mut blocker = gamer_blocker_for_plan(plan, *retroarch_scanned, *standalone_scans_complete);
    // The generic plan blocker only says "ineligible". When the shared
    // resolver refused DuckStation, Gamer shows the resolver's own reason and
    // keeps the Emulator Setup handoff, whichever generic kind the plan chose.
    if let Some(DuckStationLaunchContext {
        authority:
            crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority::Refused { reason },
        ..
    }) = duckstation.as_ref()
        && plan_targets_duckstation(plan)
        && matches!(
            blocker.kind,
            GamerBlockerKind::EmulatorNotInstalled
                | GamerBlockerKind::EmulatorSetupIncomplete
                | GamerBlockerKind::NoSafeEmulator
                | GamerBlockerKind::LaunchPlanInvalid
        )
    {
        blocker.kind = GamerBlockerKind::EmulatorSetupIncomplete;
        blocker.emulator = Some("DuckStation".into());
        blocker.detail = reason.clone();
    }
    GamerPlayAction::BlockedTyped(blocker)
}

pub(crate) struct DuckStationLaunchContext {
    pub(crate) discovery: archivefs_core::patch_manager::DuckStationProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::DuckStationProfileDiscoveryRoots,
    pub(crate) firmware_evidence: Vec<FirmwareIdentityRecord>,
    pub(crate) verified_ps1_serial: Option<String>,
    /// The shared resolver's decision that `discovery` was narrowed to.
    pub(crate) authority: crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority,
    /// Verified media set(s) of the focused game, for playlist handoff.
    pub(crate) multidisc: Option<crate::multidisc_launch_view::MultiDiscLaunchContext>,
}

impl DuckStationLaunchContext {
    /// Fallback / refusal text Launch must show beside DuckStation.
    pub(crate) fn notices(&self) -> Vec<String> {
        self.authority.launch_notices()
    }
}
pub(crate) struct PpssppLaunchContext {
    pub(crate) discovery: archivefs_core::patch_manager::PpssppProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::PpssppProfileDiscoveryRoots,
    pub(crate) verified_psp_disc_id: Option<String>,
}
pub(crate) struct MelonDsLaunchContext {
    pub(crate) discovery: archivefs_core::patch_manager::MelonDsProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::MelonDsProfileDiscoveryRoots,
    /// The independently verified Nintendo DS game key, if any.
    pub(crate) verified_game_key: Option<String>,
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

pub(crate) struct AmigaWHDLoadLaunchContext {
    pub(crate) requests: Vec<(String, String, WHDLoadLaunchInput)>,
    /// FS-UAE requests for a verified single-disk ADF, keyed like `requests`.
    pub(crate) adf: Vec<(
        String,
        String,
        archivefs_core::launch::FsUaeAdfLaunchRequest,
    )>,
}

enum AmigaWHDLoadProcess {
    Running(LaunchedAmigaWHDLoadProcess),
}

#[derive(Default)]
pub(crate) struct AmigaWHDLoadLaunchState {
    tracked: Option<(PathBuf, String, AmigaWHDLoadStage)>,
}

enum AmigaWHDLoadStage {
    Starting(Receiver<Result<AmigaWHDLoadProcess, String>>),
    Running(AmigaWHDLoadProcess),
    Exited(AmigaWHDLoadProcess),
    Failed(String),
}

impl AmigaWHDLoadLaunchState {
    pub(crate) fn poll(&mut self) -> bool {
        let Some((path, adapter, stage)) = self.tracked.take() else {
            return false;
        };
        let (stage, changed) = match stage {
            AmigaWHDLoadStage::Starting(receiver) => match receiver.try_recv() {
                Ok(Ok(process)) => (AmigaWHDLoadStage::Running(process), true),
                Ok(Err(error)) => (AmigaWHDLoadStage::Failed(error), true),
                Err(TryRecvError::Disconnected) => (
                    AmigaWHDLoadStage::Failed("launch worker stopped unexpectedly".into()),
                    true,
                ),
                Err(TryRecvError::Empty) => (AmigaWHDLoadStage::Starting(receiver), false),
            },
            AmigaWHDLoadStage::Running(mut process) => {
                let exited = match &mut process {
                    AmigaWHDLoadProcess::Running(process) => process.poll().is_some(),
                };
                (
                    if exited {
                        AmigaWHDLoadStage::Exited(process)
                    } else {
                        AmigaWHDLoadStage::Running(process)
                    },
                    exited,
                )
            }
            other @ (AmigaWHDLoadStage::Exited(_) | AmigaWHDLoadStage::Failed(_)) => (other, false),
        };
        self.tracked = Some((path, adapter, stage));
        changed
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.tracked,
            Some((
                _,
                _,
                AmigaWHDLoadStage::Starting(_) | AmigaWHDLoadStage::Running(_)
            ))
        )
    }

    pub(crate) fn failure_detail(&self) -> Option<String> {
        match &self.tracked {
            Some((_, _, AmigaWHDLoadStage::Failed(error))) => Some(error.clone()),
            _ => None,
        }
    }

    fn start(&mut self, request: WHDLoadLaunchInput) {
        let path = request
            .target
            .as_ref()
            .map(|target| target.package_path.clone())
            .unwrap_or_default();
        let adapter = match request.profile.emulator {
            archivefs_core::patch_manager::AmigaEmulatorKind::Amiberry => "amiberry",
            archivefs_core::patch_manager::AmigaEmulatorKind::FsUae => "fsuae",
        }
        .to_string();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let plan = match request.profile.emulator {
                archivefs_core::patch_manager::AmigaEmulatorKind::Amiberry => {
                    build_amiberry_whdload_command_plan(&request)
                }
                archivefs_core::patch_manager::AmigaEmulatorKind::FsUae => {
                    build_fsuae_whdload_command_plan(&request)
                }
            };
            let result = preflight_and_launch_amiga_whdload(&plan)
                .map(AmigaWHDLoadProcess::Running)
                .map_err(|error: AmigaWHDLoadLaunchError| format!("{error:?}"));
            let _ = sender.send(result);
        });
        self.tracked = Some((path, adapter, AmigaWHDLoadStage::Starting(receiver)));
    }
}

pub(crate) enum StandaloneProcess {
    #[cfg(target_os = "linux")]
    ShadPs4(archivefs_core::launch::process_spawn::WatchedProcess),
    Mame(archivefs_core::launch::process_spawn::WatchedProcess),
    /// MAME started through the canonical per-launch cheat runtime; the session
    /// owns the temporary cheat workspace and removes it after MAME exits.
    MameCheats(
        Box<
            archivefs_core::launch::cheat_runtime::CheatRuntimeSession<
                archivefs_core::launch::process_spawn::WatchedProcess,
            >,
        >,
    ),
    DuckStation(LaunchedDuckStationProcess),
    Ppsspp(LaunchedPpssppProcess),
    FsUae(archivefs_core::launch::process_spawn::WatchedProcess),
    MelonDs(archivefs_core::launch::process_spawn::WatchedProcess),
    /// melonDS started through the per-launch cheat runtime; the session owns
    /// the temporary cheat workspace and removes it after melonDS exits.
    MelonDsCheats(
        Box<
            archivefs_core::launch::cheat_runtime::CheatRuntimeSession<
                archivefs_core::launch::process_spawn::WatchedProcess,
            >,
        >,
    ),
    Rpcs3(LaunchedRpcs3Process),
    Xemu(LaunchedXemuProcess),
    Xenia(LaunchedXeniaProcess),
    Cemu(archivefs_core::launch::LaunchedCemuProcess),
    Flycast(archivefs_core::launch::LaunchedFlycastProcess),
    ScummVm(archivefs_core::launch::LaunchedScummVmProcess),
    OpenMsx(archivefs_core::launch::process_spawn::WatchedProcess),
    /// DOSBox running inside the bubblewrap sandbox.
    DosBox(archivefs_core::launch::LaunchedDosBoxProcess),
    /// ScummVM started on a private copy of a saved trainer profile the user
    /// chose for this launch; the copy is removed once it has exited.
    ScummVmTrainer(Box<archivefs_core::launch::LaunchedTrainerScummVm>),
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
    /// Frozen when a CUE preview is first offered, not silently renewed on
    /// redraw or at Play. Explicit recheck is needed after a member changes.
    cue_review: Option<(
        PathBuf,
        Vec<archivefs_core::identity_source::hashing::FileFingerprint>,
    )>,
    tracked: Option<(PathBuf, String, StandaloneLaunchStage)>,
    /// The user's optional melonDS cheat choice for the game on screen.
    melonds_cheats: Option<melonds_cheats::Choice>,
    melonds_message: Option<String>,
    /// Where temporary cheat workspaces go; `None` is the runtime's default.
    cheat_roots: Option<archivefs_core::launch::cheat_runtime::CheatRuntimeRoots>,
    scummvm_profile: scummvm_profile::Choice,
    /// The DOSBox check and the user's approval for the game on screen.
    dosbox: dosbox::Panel,
}

impl StandaloneLaunchState {
    pub(crate) fn clear_cue_review(&mut self) {
        self.cue_review = None;
    }

    /// The game the Launch page shows, for the DOSBox check.
    pub(crate) fn focus_dosbox_game(&mut self, path: Option<&Path>) {
        self.dosbox.focus(path);
    }

    fn bind_cue_review(&mut self, request: &mut StandaloneLaunchRequest) -> Result<(), String> {
        let StandaloneLaunchRequest::DuckStation(request, ..) = request else {
            return Ok(());
        };
        let path = &request.selected_content_path;
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"))
        {
            return Ok(());
        }
        if self
            .cue_review
            .as_ref()
            .is_none_or(|(reviewed, _)| reviewed != path)
        {
            let members =
                archivefs_core::launch::duckstation_execution::capture_duckstation_cue_release(
                    path,
                )
                .map_err(|error| {
                    format!("The CUE/BIN files cannot be reviewed: {}", error.detail)
                })?;
            self.cue_review = Some((path.clone(), members));
        }
        request.expected_cue_members = self.cue_review.as_ref().map(|(_, members)| members.clone());
        Ok(())
    }

    /// The reviewed files that are no longer what the preview recorded, named
    /// from what is observed now: missing or no longer a regular file, or
    /// present with a different fingerprint. Empty when nothing was reviewed.
    fn cue_review_changes(&self) -> Vec<String> {
        let Some((_, members)) = self.cue_review.as_ref() else {
            return Vec::new();
        };
        members
            .iter()
            .filter_map(|expected| {
                let name = expected.path.file_name().map_or_else(
                    || expected.path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                match std::fs::symlink_metadata(&expected.path) {
                    Err(_) => Some(format!("{name} (missing)")),
                    Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                        Some(format!("{name} (no longer a regular file)"))
                    }
                    Ok(_) => (archivefs_core::identity_source::hashing::FileFingerprint::observe(
                        &expected.path,
                    )
                    .as_ref()
                        != Some(expected))
                    .then(|| format!("{name} (changed)")),
                }
            })
            .collect()
    }

    /// The Playing Library supplies fresh, catalogue-bound verification in
    /// the same worker that invokes the existing native MAME executor.
    pub(crate) fn start_mame_revalidated(
        &mut self,
        content: PathBuf,
        prepare: impl FnOnce() -> Result<MameLaunchRequest, String> + Send + 'static,
    ) {
        if self.is_active() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result =
                prepare().and_then(|request| StandaloneLaunchRequest::Mame(request).execute());
            let _ = sender.send(result);
        });
        self.tracked = Some((
            content,
            "mame".into(),
            StandaloneLaunchStage::Starting(receiver),
        ));
    }

    /// As [`Self::start_mame_revalidated`], but starts MAME through the core's
    /// per-launch cheat runtime with the user's explicit selection. The core
    /// re-reads and re-validates the native cheat file; any refusal stops the
    /// launch before MAME starts.
    pub(crate) fn start_mame_revalidated_with_cheats(
        &mut self,
        content: PathBuf,
        cheat: archivefs_core::launch::cheat_runtime_mame::MameCheatSelection,
        roots: archivefs_core::launch::cheat_runtime::CheatRuntimeRoots,
        prepare: impl FnOnce() -> Result<MameLaunchRequest, String> + Send + 'static,
    ) {
        if self.is_active() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result =
                prepare().and_then(|request| launch_mame_with_cheats(&request, &cheat, &roots));
            let _ = sender.send(result);
        });
        self.tracked = Some((
            content,
            "mame".into(),
            StandaloneLaunchStage::Starting(receiver),
        ));
    }

    pub(crate) fn show_mame_feedback(&mut self, ui: &mut egui::Ui) {
        match self.tracked.as_mut().map(|(_, _, stage)| stage) {
            Some(StandaloneLaunchStage::Starting(_)) => {
                ui.label("Revalidating collection and launching…");
            }
            Some(StandaloneLaunchStage::Running(StandaloneProcess::MameCheats(_))) => {
                ui.label("MAME process running with this launch's cheat file · boot/playability not verified");
                ui.label(archivefs_core::launch::cheat_runtime_mame::MameCheatActivation::CheatSystemEnabledMaterialAvailableActivationNotProven.label());
            }
            Some(StandaloneLaunchStage::Running(_)) => {
                ui.label("MAME process running · boot/playability not verified");
            }
            Some(StandaloneLaunchStage::Exited(StandaloneProcess::MameCheats(session))) => {
                let receipt = session.receipt();
                let exit = match receipt.process.as_ref().and_then(|p| p.exit.as_ref()) {
                    Some(exit) if exit.success => "exited normally".to_string(),
                    Some(exit) => format!("failed (exit code {:?})", exit.code),
                    None => "exited (status unknown)".to_string(),
                };
                ui.label(format!(
                    "MAME {exit}; temporary cheat files: {:?}. EmuWiz did not see the cheats work.",
                    receipt.cleanup
                ));
                for error in &receipt.errors {
                    ui.label(error.as_str());
                }
            }
            Some(StandaloneLaunchStage::Failed(error)) => {
                ui.label(format!("Launch refused or failed: {error}"));
            }
            Some(StandaloneLaunchStage::Exited(StandaloneProcess::Mame(process))) => {
                if let Some(report) = process.poll() {
                    match &report.status {
                        Ok(status) if status.success() => {
                            ui.label("MAME process exited normally; this does not prove the machine booted.");
                        }
                        status => {
                            ui.label(format!("MAME process failed: {status:?}"));
                            ui.label(String::from_utf8_lossy(&report.stderr).as_ref());
                        }
                    }
                }
            }
            _ => {}
        }
    }

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
                    #[cfg(target_os = "linux")]
                    StandaloneProcess::ShadPs4(p) => p.poll().is_some(),
                    StandaloneProcess::Mame(p) => p.poll().is_some(),
                    StandaloneProcess::MameCheats(session) => {
                        session.poll();
                        session.is_finished()
                    }
                    StandaloneProcess::DuckStation(p) => p.poll().is_some(),
                    StandaloneProcess::Ppsspp(p) => p.poll().is_some(),
                    StandaloneProcess::MelonDs(p) => p.poll().is_some(),
                    StandaloneProcess::MelonDsCheats(session) => {
                        session.poll();
                        session.is_finished()
                    }
                    StandaloneProcess::Rpcs3(p) => p.poll().is_some(),
                    StandaloneProcess::Xemu(p) => p.poll().is_some(),
                    StandaloneProcess::Xenia(p) => p.poll().is_some(),
                    StandaloneProcess::FsUae(p) => p.poll().is_some(),
                    StandaloneProcess::Cemu(p) => p.poll().is_some(),
                    StandaloneProcess::Flycast(p) => p.poll().is_some(),
                    StandaloneProcess::ScummVm(p) => p.poll().is_some(),
                    StandaloneProcess::OpenMsx(p) => p.poll().is_some(),
                    StandaloneProcess::DosBox(p) => p.poll().is_some(),
                    StandaloneProcess::ScummVmTrainer(p) => p.poll().is_some(),
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

    pub(crate) fn failure_detail(&self) -> Option<String> {
        match &self.tracked {
            Some((_, _, StandaloneLaunchStage::Failed(error))) => Some(error.clone()),
            _ => None,
        }
    }

    fn start(&mut self, request: StandaloneLaunchRequest) {
        let (path, adapter) = request.key();
        if let Some(receiver) = scummvm_profile::start_with_profile(self, &request) {
            self.tracked = Some((path, adapter, StandaloneLaunchStage::Starting(receiver)));
            return;
        }
        let (sender, receiver) = mpsc::channel();
        if let StandaloneLaunchRequest::MelonDs(melonds, roots) = &request
            && let Some(choice) = self
                .melonds_cheats
                .clone()
                .filter(|choice| choice.rom == melonds.selected_content_path)
            && melonds_cheats::unsupported_reason(melonds).is_none()
        {
            let (melonds, roots) = (melonds.clone(), roots.clone());
            let runtime_roots = self.cheat_roots.clone().unwrap_or_default();
            thread::spawn(move || {
                let result = melonds_cheats::launch(&melonds, &roots, &choice, &runtime_roots);
                let _ = sender.send(result);
            });
            self.tracked = Some((path, adapter, StandaloneLaunchStage::Starting(receiver)));
            return;
        }
        thread::spawn(move || {
            let result = request.execute().map_err(|error| format!("{error:?}"));
            let _ = sender.send(result);
        });
        self.tracked = Some((path, adapter, StandaloneLaunchStage::Starting(receiver)));
    }
}

/// DuckStation refusal in words: the preflight's own explanation, not the
/// Rust debug rendering of its error type.
fn duckstation_refusal_text(error: &DuckStationLaunchExecutionError) -> String {
    match error {
        DuckStationLaunchExecutionError::Preflight(preflight) => format!(
            "DuckStation was not started: {}.",
            preflight.detail.trim_end_matches('.')
        ),
        DuckStationLaunchExecutionError::Spawn(spawn) => {
            format!("DuckStation could not be started: {spawn:?}")
        }
    }
}

fn openmsx_refusal_text(error: &archivefs_core::launch::OpenMsxLaunchPreflightError) -> String {
    use archivefs_core::launch::OpenMsxLaunchPreflightErrorKind as K;
    match error.kind {
        K::ContentNotFound | K::ContentIsSymlink | K::ContentNotRegularFile => {
            "The cartridge is no longer a regular file at its checked location. Restore the file and retry readiness.".into()
        }
        K::ContentChangedBeforeSpawn => {
            "The cartridge changed after it was checked. Recheck identity and readiness before playing.".into()
        }
        K::ContentFormatUnsupported => {
            "openMSX launch supports direct .mx1 cartridges for MSX and .mx2 cartridges for MSX2. Tape, disk, generic ROM and archive inputs stay blocked.".into()
        }
        K::IdentityUnresolved | K::IdentityMismatch => {
            "The current file no longer verifies as the same MSX game. Recheck its identity before playing.".into()
        }
        K::MachineMismatch => {
            "The checked C-BIOS machine no longer matches the verified MSX system. Recheck launch readiness.".into()
        }
        K::ProfileNotFound | K::ProfileIneligible | K::BindingUnavailable | K::BindingDrift => {
            "The checked openMSX installation changed or is unavailable. Check its installation and retry readiness.".into()
        }
        K::ContentPathNotAbsolute => "The cartridge path is invalid. Recheck the library location.".into(),
    }
}

#[cfg(test)]
mod duckstation_refusal_text_tests {
    use super::*;
    use archivefs_core::launch::duckstation_execution::{
        DuckStationLaunchPreflightError, DuckStationLaunchPreflightErrorKind,
    };

    #[test]
    fn a_preflight_refusal_reads_as_a_sentence_without_debug_syntax() {
        let text = duckstation_refusal_text(&DuckStationLaunchExecutionError::Preflight(
            DuckStationLaunchPreflightError {
                kind: DuckStationLaunchPreflightErrorKind::IdentityUnresolved,
                detail: "fresh identity re-inspection produced Unknown".into(),
            },
        ));
        assert_eq!(
            text,
            "DuckStation was not started: fresh identity re-inspection produced Unknown."
        );
    }
}

/// Native melonDS launch: the core's fresh preflight (content, profile, binding,
/// configuration and firmware) and its own spawn. The identity handed to the
/// preflight is the one authorised at readiness time (the verified game key
/// the request was built from), so it detects file swaps through
/// `content_identity`, `executable_identity` and `config_identity`, not later
/// changes to the key's evidence without the file changing.
fn launch_melonds(
    request: &archivefs_core::launch::MelonDsLaunchRequest,
    roots: &archivefs_core::patch_manager::MelonDsProfileDiscoveryRoots,
) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
    let identity = archivefs_core::launch::CanonicalIdentityStatus::Resolved(
        archivefs_core::launch::ResolvedIdentity {
            platform_id: request.expected_platform_id.clone(),
            game_key: request.expected_game_key.clone(),
        },
    );
    let command = archivefs_core::launch::preflight_melonds_launch(
        request,
        roots,
        &identity,
        Some(&request.expected_game_key),
    )
    .map_err(|e| format!("melonDS was not started: {:?}: {}", e.kind, e.detail))?;
    archivefs_core::launch::spawn_melonds(&command)
        .map_err(|e| format!("melonDS was not started: {e}"))
}

/// Starts MAME with the user's chosen cheats. Anything the core refuses stops
/// the launch before MAME starts.
fn launch_mame_with_cheats(
    request: &MameLaunchRequest,
    cheat: &archivefs_core::launch::cheat_runtime_mame::MameCheatSelection,
    roots: &archivefs_core::launch::cheat_runtime::CheatRuntimeRoots,
) -> Result<StandaloneProcess, String> {
    use archivefs_core::launch::cheat_runtime_mame::{
        MameCheatLaunch, preflight_and_launch_mame_with_cheats,
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let launch_id = format!("mame-{}-{nanos}", std::process::id());
    match preflight_and_launch_mame_with_cheats(request, Some(cheat), &launch_id, roots) {
        Ok(MameCheatLaunch::WithCheats { session, .. }) => {
            Ok(StandaloneProcess::MameCheats(session))
        }
        Ok(MameCheatLaunch::Plain(process)) => Ok(StandaloneProcess::Mame(process)),
        Err(error) => Err(format!("MAME was not started: {error:?}")),
    }
}

#[derive(Clone)]
pub(crate) enum StandaloneLaunchRequest {
    Cemu(cemu::Request),
    Flycast(flycast::Request),
    #[cfg(target_os = "linux")]
    ShadPs4(crate::emulator_setup::shadps4::Request),
    Mame(MameLaunchRequest),
    DuckStation(
        DuckStationLaunchRequest,
        archivefs_core::patch_manager::DuckStationProfileDiscoveryRoots,
        Vec<FirmwareIdentityRecord>,
        Option<crate::multidisc_launch_view::PlaylistHandoff>,
    ),
    Ppsspp(
        PpssppLaunchRequest,
        archivefs_core::patch_manager::PpssppProfileDiscoveryRoots,
    ),
    MelonDs(
        archivefs_core::launch::MelonDsLaunchRequest,
        archivefs_core::patch_manager::MelonDsProfileDiscoveryRoots,
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
    /// Plain native ScummVM launch of an extracted game folder; core re-runs
    /// detection and checks the executable at execute time.
    ScummVm(archivefs_core::launch::ScummVmLaunchRequest),
    /// One verified single-disk ADF; core re-derives its DAT identity and
    /// re-runs the FS-UAE preflight at click time.
    FsUaeAdf(Box<archivefs_core::launch::FsUaeAdfLaunchRequest>),
    OpenMsx(
        archivefs_core::launch::OpenMsxLaunchRequest,
        archivefs_core::patch_manager::OpenMsxProfileDiscoveryRoots,
    ),
    /// A DOS game folder and start command the user approved; core
    /// re-reviews it and only runs DOSBox inside the sandbox.
    DosBox(Box<archivefs_core::launch::DosBoxLaunchRequest>),
}

impl StandaloneLaunchRequest {
    fn adapter_name(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::ShadPs4(_) => "shadPS4",
            Self::Mame(_) => "MAME",
            Self::DuckStation(..) => "DuckStation",
            Self::Ppsspp(_, _) => "PPSSPP",
            Self::MelonDs(_, _) => "melonDS",
            Self::Rpcs3(_, _) => "RPCS3",
            Self::Xemu(_, _) => "xemu",
            Self::Xenia(_, _) => "Xenia",
            Self::FsUaeAdf(_) => "FS-UAE",
            Self::Cemu(_) => "Cemu",
            Self::Flycast(_) => "Flycast",
            Self::ScummVm(_) => "ScummVM",
            Self::OpenMsx(..) => "openMSX",
            Self::DosBox(_) => "DOSBox",
        }
    }

    fn key(&self) -> (PathBuf, String) {
        match self {
            #[cfg(target_os = "linux")]
            Self::ShadPs4(r) => (r.plan.game().executable().into(), "shadps4".into()),
            Self::Mame(r) => (r.selected_content.clone(), "mame".into()),
            Self::DuckStation(r, ..) => (r.selected_content_path.clone(), "duckstation".into()),
            Self::Ppsspp(r, _) => (r.selected_content_path.clone(), "ppsspp".into()),
            Self::MelonDs(r, _) => (r.selected_content_path.clone(), "melonds".into()),
            Self::Rpcs3(r, _) => (r.selected_content_path.clone(), "rpcs3".into()),
            Self::Xemu(r, _) => (r.selected_content_path.clone(), "xemu".into()),
            Self::Xenia(r, _) => (r.selected_content_path.clone(), "xenia".into()),
            Self::FsUaeAdf(r) => (r.fsuae.selected_content_path.clone(), "fsuae".into()),
            Self::Cemu(r) => r.key(),
            Self::Flycast(r) => r.key(),
            Self::ScummVm(r) => (r.selected_game_folder.clone(), "scummvm".into()),
            Self::OpenMsx(r, _) => (r.selected_content_path.clone(), "openmsx".into()),
            Self::DosBox(r) => (r.game_directory.clone(), dosbox::ADAPTER_KEY.into()),
        }
    }
    pub(crate) fn execute(self) -> Result<StandaloneProcess, String> {
        match self {
            Self::Cemu(r) => r.execute().map(StandaloneProcess::Cemu),
            Self::Flycast(r) => r.execute().map(StandaloneProcess::Flycast),
            #[cfg(target_os = "linux")]
            Self::ShadPs4(r) => r
                .execute()
                .map(StandaloneProcess::ShadPs4)
                .map_err(|e| format!("{:?}: {}", e.kind, e.detail)),
            Self::Mame(r) => archivefs_core::launch::preflight_and_launch_mame(&r)
                .map(StandaloneProcess::Mame)
                .map_err(|error| format!("{error:?}")),
            Self::DuckStation(r, roots, firmware, playlist) => {
                archivefs_core::launch::preflight_and_launch_duckstation_with_playlist(
                    &r,
                    &roots,
                    &firmware,
                    playlist
                        .as_ref()
                        .map(|(sets, list)| (sets.as_slice(), list)),
                )
                .map(StandaloneProcess::DuckStation)
                .map_err(|e: DuckStationLaunchExecutionError| duckstation_refusal_text(&e))
            }
            Self::MelonDs(r, roots) => launch_melonds(&r, &roots).map(StandaloneProcess::MelonDs),
            Self::Ppsspp(r, roots) => preflight_and_launch_ppsspp(&r, &roots)
                .map(StandaloneProcess::Ppsspp)
                .map_err(|e: PpssppLaunchExecutionError| format!("{e:?}")),
            Self::Rpcs3(r, roots) => preflight_and_launch_rpcs3(&r, &roots)
                .map(StandaloneProcess::Rpcs3)
                .map_err(|e: Rpcs3LaunchExecutionError| format!("{e:?}")),
            Self::Xemu(r, roots) => preflight_and_launch_xemu(&r, &roots)
                .map(StandaloneProcess::Xemu)
                .map_err(|e: XemuLaunchExecutionError| format!("{e:?}")),
            Self::ScummVm(r) => archivefs_core::launch::preflight_and_launch_scummvm(&r)
                .map(StandaloneProcess::ScummVm)
                .map_err(|error| scummvm_launch_error_message(&error)),
            Self::DosBox(r) => archivefs_core::launch::preflight_and_launch_dosbox(&r)
                .map(StandaloneProcess::DosBox)
                .map_err(|error| dosbox::dosbox_refusal_text(&error)),
            Self::FsUaeAdf(r) => archivefs_core::launch::preflight_and_launch_fsuae_adf(&r)
                .map(StandaloneProcess::FsUae)
                .map_err(|error| error.reason()),
            Self::OpenMsx(r, roots) => {
                let report = crate::selected_evidence_page::gather_selected_evidence_fast(
                    &r.selected_content_path,
                    Some(&r.expected_platform_id),
                )
                .map_err(|error| format!("openMSX identity could not be rechecked: {error}"))?;
                let (identity, _) = archivefs_core::launch::canonical_identity_from_game_report(
                    &report.game_identity_report,
                );
                preflight_and_launch_openmsx(&r, &roots, &identity)
                    .map(StandaloneProcess::OpenMsx)
                    .map_err(|error| match error {
                        archivefs_core::launch::OpenMsxLaunchExecutionError::Preflight(e) => {
                            format!("{} ({:?}: {})", openmsx_refusal_text(&e), e.kind, e.detail)
                        }
                        archivefs_core::launch::OpenMsxLaunchExecutionError::Spawn(e) => {
                            format!("openMSX could not be started: {e}")
                        }
                    })
            }
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
    expected_appimage_executable: Option<PathBuf>,
}

impl RetroArchLaunchKey {
    fn from_request(request: &RetroArchLaunchRequest) -> Self {
        Self {
            content_path: request.selected_content_path.clone(),
            profile: request.profile,
            core_stem: request.core_stem.clone(),
            expected_appimage_executable: request.expected_appimage_executable.clone(),
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
    /// A launch with cheats: the worker probes the core, checks overrides and
    /// prepares the temporary profile before RetroArch starts.
    StartingCheats {
        receiver: Receiver<
            Result<archivefs_core::launch::cheat_runtime_retroarch::RetroArchCheatLaunch, String>,
        >,
    },
    RunningCheats {
        session: Box<
            archivefs_core::launch::cheat_runtime::CheatRuntimeSession<LaunchedRetroArchProcess>,
        >,
    },
    ExitedCheats {
        session: Box<
            archivefs_core::launch::cheat_runtime::CheatRuntimeSession<LaunchedRetroArchProcess>,
        >,
    },
    FailedCheats {
        message: String,
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
    classic_pokes: classic_pokes::Chooser,
    tracked: Option<(RetroArchLaunchKey, RetroArchLaunchStage)>,
    /// The user's optional cheat file and choices for the game on screen.
    cheats: Option<retroarch_cheats::Choice>,
    cheats_message: Option<String>,
    /// The read-only lookup of cheat sets already on this computer.
    cheat_sets: retroarch_cheat_sets::Lookup,
    /// The cheat cache the lookup reads; `None` is EmuWiz's default cache.
    cheat_cache_root: Option<PathBuf>,
    /// The player asked to continue in Cheats & Mods.
    open_cheats_mods: bool,
    /// How the core-name probe is run; `None` is the production launcher.
    probe: Option<
        std::sync::Arc<dyn archivefs_core::launch::libretro_probe::ProbeLauncher + Send + Sync>,
    >,
    /// The environment the cheat launch discovers RetroArch in; `None` is the
    /// real process environment.
    environment: Option<DiscoveryEnvironment>,
    /// Test seam: a cheat file "picked" without the native file dialog. It is
    /// consumed at the same point, by the same `Choice::load` call, as a
    /// dialog result.
    #[cfg(test)]
    pub(crate) pending_cheat_file: Option<PathBuf>,
}

#[cfg(test)]
impl RetroArchLaunchState {
    /// Run the core-name probe through `probe` instead of re-executing the
    /// current binary (a test binary cannot serve the production probe).
    /// Read cheat sets from `root` instead of EmuWiz's default cheat cache.
    pub(crate) fn use_cheat_cache(&mut self, root: PathBuf) {
        self.cheat_cache_root = Some(root);
    }

    pub(crate) fn use_probe(
        &mut self,
        probe: std::sync::Arc<
            dyn archivefs_core::launch::libretro_probe::ProbeLauncher + Send + Sync,
        >,
    ) {
        self.probe = Some(probe);
    }
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
            RetroArchLaunchStage::StartingCheats { receiver } => match receiver.try_recv() {
                Ok(Ok(archivefs_core::launch::cheat_runtime_retroarch::RetroArchCheatLaunch::Plain(
                    process,
                ))) => {
                    self.tracked = Some((key, RetroArchLaunchStage::Running { process }));
                    true
                }
                Ok(Ok(
                    archivefs_core::launch::cheat_runtime_retroarch::RetroArchCheatLaunch::WithCheats {
                        session,
                        ..
                    },
                )) => {
                    self.tracked = Some((key, RetroArchLaunchStage::RunningCheats { session }));
                    true
                }
                Ok(Err(message)) => {
                    self.tracked = Some((key, RetroArchLaunchStage::FailedCheats { message }));
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.tracked = Some((key, RetroArchLaunchStage::StartingCheats { receiver }));
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.tracked = Some((
                        key,
                        RetroArchLaunchStage::FailedCheats {
                            message: "RetroArch was not started: the launch worker stopped without reporting a result.".into(),
                        },
                    ));
                    true
                }
            },
            RetroArchLaunchStage::RunningCheats { mut session } => {
                session.poll();
                if session.is_finished() {
                    self.tracked = Some((key, RetroArchLaunchStage::ExitedCheats { session }));
                    true
                } else {
                    self.tracked = Some((key, RetroArchLaunchStage::RunningCheats { session }));
                    false
                }
            }
            other @ (RetroArchLaunchStage::Exited { .. }
            | RetroArchLaunchStage::Failed { .. }
            | RetroArchLaunchStage::ExitedCheats { .. }
            | RetroArchLaunchStage::FailedCheats { .. }) => {
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
                RetroArchLaunchStage::Starting { .. }
                    | RetroArchLaunchStage::Running { .. }
                    | RetroArchLaunchStage::StartingCheats { .. }
                    | RetroArchLaunchStage::RunningCheats { .. }
            ))
        )
    }

    pub(crate) fn failure_detail(&self) -> Option<String> {
        match &self.tracked {
            Some((_, RetroArchLaunchStage::Failed { error })) => Some(format!("{error:?}")),
            Some((_, RetroArchLaunchStage::FailedCheats { message })) => Some(message.clone()),
            _ => None,
        }
    }

    pub(crate) fn start(
        &mut self,
        request: RetroArchLaunchRequest,
        playlist: Option<crate::multidisc_launch_view::PlaylistHandoff>,
    ) {
        let key = RetroArchLaunchKey::from_request(&request);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let filesystem = HostReadOnlyFilesystem;
            let environment = DiscoveryEnvironment::from_process_environment();
            let result = archivefs_core::launch::preflight_and_launch_retroarch_with_playlist(
                &request,
                &filesystem,
                &environment,
                playlist
                    .as_ref()
                    .map(|(sets, list)| (sets.as_slice(), list)),
            );
            let _ = sender.send(result);
        });
        self.tracked = Some((key, RetroArchLaunchStage::Starting { receiver }));
    }

    /// Launches with the user's cheat choices (Advanced view only). Without a
    /// choice for this exact request it is an ordinary launch. Every refusal
    /// happens in the worker before RetroArch starts.
    pub(crate) fn start_with_cheats(&mut self, request: RetroArchLaunchRequest) {
        let Some(choice) = self
            .cheats
            .clone()
            .filter(|choice| choice.matches(&request))
        else {
            self.start(request, None);
            return;
        };
        let key = RetroArchLaunchKey::from_request(&request);
        let probe = self
            .probe
            .clone()
            .unwrap_or_else(retroarch_cheats::production_probe);
        let environment = self.environment.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let environment =
                environment.unwrap_or_else(DiscoveryEnvironment::from_process_environment);
            let result = retroarch_cheats::launch(&request, &choice, probe.as_ref(), &environment);
            let _ = sender.send(result);
        });
        self.tracked = Some((key, RetroArchLaunchStage::StartingCheats { receiver }));
    }

    fn start_with_pokes(
        &mut self,
        request: RetroArchLaunchRequest,
        selected: classic_pokes::Selection,
    ) {
        let key = RetroArchLaunchKey::from_request(&request);
        let probe = self
            .probe
            .clone()
            .unwrap_or_else(retroarch_cheats::production_probe);
        let environment = self.environment.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let environment =
                environment.unwrap_or_else(DiscoveryEnvironment::from_process_environment);
            let _ = sender.send(classic_pokes::launch(
                &request,
                &selected,
                probe.as_ref(),
                &environment,
            ));
        });
        self.tracked = Some((key, RetroArchLaunchStage::StartingCheats { receiver }));
    }

    /// Cheat-specific lines for the launch tracked for `request`, if any.
    pub(crate) fn cheat_lines(&mut self, request: &RetroArchLaunchRequest) -> Vec<String> {
        let this_key = RetroArchLaunchKey::from_request(request);
        match self.tracked.as_mut() {
            Some((key, RetroArchLaunchStage::RunningCheats { session, .. }))
                if *key == this_key =>
            {
                vec![
                    retroarch_cheats::requested_label(session),
                    retroarch_cheats::activation_label(),
                ]
            }
            Some((key, RetroArchLaunchStage::ExitedCheats { session })) if *key == this_key => {
                retroarch_cheats::session_summary(session)
            }
            _ => Vec::new(),
        }
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
                RetroArchLaunchStage::StartingCheats { .. } => RetroArchLaunchDisplay::Starting,
                RetroArchLaunchStage::RunningCheats { session } => {
                    RetroArchLaunchDisplay::Running {
                        pid: session.process().map_or(0, |p| p.pid),
                    }
                }
                RetroArchLaunchStage::ExitedCheats { session } => {
                    // `poll` on an exited process returns its cached report, so
                    // calling it every frame is cheap and idempotent.
                    let exit = session
                        .receipt()
                        .process
                        .as_ref()
                        .and_then(|p| p.exit.clone());
                    let success = exit.as_ref().is_some_and(|e| e.success);
                    let status_detail = match &exit {
                        Some(exit) => format!("exit code {:?}", exit.code),
                        None => "status unknown".to_string(),
                    };
                    let stderr_tail = (!success)
                        .then(|| {
                            session
                                .process_mut()
                                .and_then(|p| p.poll())
                                .filter(|report| !report.stderr.is_empty())
                                .map(|report| String::from_utf8_lossy(&report.stderr).into_owned())
                        })
                        .flatten();
                    RetroArchLaunchDisplay::Exited {
                        success,
                        status_detail,
                        stderr_tail,
                    }
                }
                RetroArchLaunchStage::FailedCheats { message } => RetroArchLaunchDisplay::Failed {
                    message: "RetroArch was not started.",
                    detail: message.clone(),
                },
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

    pub(crate) fn failure_detail(&self) -> Option<String> {
        match &self.tracked {
            Some((_, DolphinLaunchStage::Failed { error })) => Some(format!("{error:?}")),
            _ => None,
        }
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

    pub(crate) fn failure_detail(&self) -> Option<String> {
        match &self.tracked {
            Some((_, Pcsx2LaunchStage::Failed { error })) => Some(format!("{error:?}")),
            _ => None,
        }
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
        expected_installation: binding.installation,
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
/// [`ProfileKind::Native`] or an exact, verified RetroArch
/// [`ProfileKind::AppImage`], strictly [`LaunchReadiness::Ready`] (never
/// `ReadyWithWarnings`), direct loose/plain content
/// ([`LaunchContainerKind::PlainFile`], `requires_mount == false`), no
/// blockers, no warnings, and a resolved plan-level platform/game key.
/// The single multi-disc decision for a candidate, used by Preview, Launch and
/// Game Details alike. Only DuckStation and RetroArch can receive a playlist.
pub(crate) fn multidisc_decision(
    candidate: &LaunchCandidate,
    retroarch: Option<&RetroArchLaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
) -> archivefs_core::launch::MultiDiscLaunch {
    multidisc_decision_with(candidate, retroarch, duckstation, None)
}

/// [`multidisc_decision`] with an explicit media-set context for emulators
/// other than RetroArch and DuckStation (which carry their own). Those other
/// emulators never receive a playlist; the decision only explains that.
pub(crate) fn multidisc_decision_with(
    candidate: &LaunchCandidate,
    retroarch: Option<&RetroArchLaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    fallback: Option<&crate::multidisc_launch_view::MultiDiscLaunchContext>,
) -> archivefs_core::launch::MultiDiscLaunch {
    use archivefs_core::launch::MultiDiscLaunch;
    match &candidate.target {
        LaunchTarget::RetroArchCore {
            profile, core_stem, ..
        } => {
            let Some(context) = retroarch else {
                return MultiDiscLaunch::NotMultiDisc;
            };
            let Some(multidisc) = &context.multidisc else {
                return MultiDiscLaunch::NotMultiDisc;
            };
            multidisc.decide(
                candidate,
                context.core_extensions(*profile, core_stem),
                false,
            )
        }
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } if *adapter_id == "duckstation" => {
            let Some(context) = duckstation else {
                return MultiDiscLaunch::NotMultiDisc;
            };
            let Some(multidisc) = &context.multidisc else {
                return MultiDiscLaunch::NotMultiDisc;
            };
            let sandboxed = context
                .discovery
                .profiles
                .iter()
                .find(|profile| &profile.profile_id == profile_id)
                .and_then(|profile| {
                    archivefs_core::patch_manager::resolve_duckstation_native_launch_binding(
                        profile,
                        &context.roots,
                    )
                    .ok()
                })
                .is_none_or(|binding| {
                    binding.installation.kind()
                        == archivefs_core::launch::installation::InstallationKind::Flatpak
                });
            multidisc.decide(candidate, None, sandboxed)
        }
        _ => {
            let multidisc = fallback
                .or_else(|| retroarch.and_then(|context| context.multidisc.as_ref()))
                .or_else(|| duckstation.and_then(|context| context.multidisc.as_ref()));
            match multidisc {
                Some(multidisc) => multidisc.decide(candidate, None, false),
                None => MultiDiscLaunch::NotMultiDisc,
            }
        }
    }
}

fn retroarch_playlist(
    candidate: &LaunchCandidate,
    retroarch: Option<&RetroArchLaunchContext>,
) -> Option<crate::multidisc_launch_view::PlaylistHandoff> {
    let decision = multidisc_decision(candidate, retroarch, None);
    retroarch?.multidisc.as_ref()?.handoff(&decision)
}

/// The request Play uses for `candidate`, only when RetroArch is handed that
/// one content file (no multi-disc playlist). Features that must name files
/// after what RetroArch loads, such as per-game bezel overrides, use this.
pub(crate) fn single_content_retroarch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    retroarch: Option<&RetroArchLaunchContext>,
) -> Option<RetroArchLaunchRequest> {
    if retroarch_playlist(candidate, retroarch).is_some() {
        return None;
    }
    retroarch_launch_request(plan, candidate, retroarch)
}

/// What a plain launch of `request` would hand RetroArch: see
/// [`retroarch_cheats::bezel_identity`]. Starts a probe child; call it from a
/// worker thread.
pub(crate) fn retroarch_bezel_identity(
    request: &RetroArchLaunchRequest,
) -> Result<RetroArchBezelLaunch, String> {
    retroarch_cheats::bezel_identity(request)
}

/// The RetroArch install a plain launch uses, as far as a per-game bezel
/// needs it: the launched content and core, plus that profile's own settings
/// file and overlay folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RetroArchBezelLaunch {
    pub(crate) identity: archivefs_core::bezel_apply::RetroArchContentIdentity,
    pub(crate) config: PathBuf,
    pub(crate) overlays: PathBuf,
}

fn retroarch_launch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    retroarch: Option<&RetroArchLaunchContext>,
) -> Option<RetroArchLaunchRequest> {
    // A supported multi-disc game whose playlist cannot be prepared safely is
    // not offered as a single-disc Play.
    if matches!(
        multidisc_decision(candidate, retroarch, None),
        archivefs_core::launch::MultiDiscLaunch::Refused { .. }
    ) {
        return None;
    }
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
    let expected_appimage_executable = match profile.profile_kind {
        ProfileKind::Native => None,
        ProfileKind::AppImage => retroarch?.exact_appimage_executable(*profile),
        ProfileKind::Flatpak => return None,
    };
    Some(RetroArchLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: platform_id.clone(),
        expected_game_key: game_key.clone(),
        profile: *profile,
        core_stem: core_stem.clone(),
        expected_appimage_executable,
    })
}

/// Translates a core launch error into a short player-facing message. Never
/// says "no emulator installed" for an identity/content revalidation
/// failure - the detailed technical text stays available behind
/// [`widgets::technical_details`].
/// Plain wording for a refused or failed ScummVM start. Nothing was launched
/// unless the spawn itself succeeded.
fn scummvm_launch_error_message(
    error: &archivefs_core::launch::ScummVmLaunchExecutionError,
) -> String {
    use archivefs_core::launch::ScummVmLaunchExecutionError as E;
    match error {
        E::Preflight(e) => format!(
            "ScummVM was not started: {:?}: {}. Check ScummVM readiness and the game folder, then try again.",
            e.kind, e.detail
        ),
        E::Spawn(e) => format!("ScummVM could not be started: {e:?}"),
    }
}

fn launch_error_message(error: &LaunchExecutionError) -> (&'static str, String) {
    match error {
        LaunchExecutionError::Preflight(preflight)
            if preflight.kind == LaunchPreflightErrorKind::ExecutableAmbiguous =>
        {
            (
                "More than one RetroArch program was found, so EmuWiz did not start either. \
                 Keep only the RetroArch you want on your PATH (for example remove or rename an \
                 extra launcher script), then check readiness again. Technical details list the \
                 programs found.",
                format!("{:?}: {}", preflight.kind, preflight.detail),
            )
        }
        LaunchExecutionError::Preflight(preflight) => {
            let message = match preflight.kind {
                LaunchPreflightErrorKind::ExecutableAmbiguous => {
                    "More than one RetroArch program was found, so EmuWiz did not start either."
                }
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
                    "The selected RetroArch profile is not supported for launch."
                }
                LaunchPreflightErrorKind::AppImageMissing => {
                    "The selected RetroArch AppImage is no longer available."
                }
                LaunchPreflightErrorKind::AppImageVerificationStale => {
                    "The selected RetroArch AppImage can no longer be verified."
                }
                LaunchPreflightErrorKind::AppImageUnsafe => {
                    "The selected RetroArch AppImage is not a safe regular file."
                }
                LaunchPreflightErrorKind::AppImageNotExecutable => {
                    "The selected RetroArch AppImage is not executable. Restore its execute permission and try again."
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
                LaunchPreflightErrorKind::MultiDiscPlaylistRefused => {
                    "A disc in this multi-disc game changed, or its disc playlist could not be \
                     prepared - re-check readiness and try again."
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
    amiga_whdload_launch_state: &mut AmigaWHDLoadLaunchState,
) -> Option<LaunchReadinessPageAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Play / Launch readiness",
        Some(
            "Ways this game can be played. Check emulator readiness, then launch only a command backed by verified identity and resolved content.",
        ),
    );

    match input {
        LaunchReadinessInput::AtariSt(snapshot) => snapshot.show(ui),
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
        LaunchReadinessInput::DiscFilesUnresolved(detail) => {
            widgets::banner(
                ui,
                "Disc files need attention",
                detail,
                widgets::StatusTone::Warning,
            );
        }
        LaunchReadinessInput::InspectionIncomplete => {
            widgets::banner(
                ui,
                "Rescan to finish checking",
                archivefs_core::launch::INSPECTION_INCOMPLETE_REASON,
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
            retroarch,
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
            amiga_whdload,
            #[cfg(target_os = "linux")]
            shadps4,
            ..
        } => {
            if let Some(context) = flycast {
                for candidate in &plan.candidates {
                    context.show(ui, plan, candidate, standalone_launch_state);
                }
            }
            if let Some(context) = cemu {
                for candidate in &plan.candidates {
                    if matches!(
                        candidate.target,
                        LaunchTarget::Standalone {
                            adapter_id: "cemu",
                            ..
                        }
                    ) {
                        context.show(ui, plan, candidate, standalone_launch_state);
                    }
                }
            }
            #[cfg(target_os = "linux")]
            if let Some(snapshot) = shadps4 {
                for candidate in &plan.candidates {
                    if matches!(
                        candidate.target,
                        LaunchTarget::Standalone {
                            adapter_id: "shadps4",
                            ..
                        }
                    ) {
                        if let Some(request) =
                            crate::emulator_setup::shadps4::show(ui, snapshot, candidate)
                        {
                            show_standalone_launch_action(
                                ui,
                                standalone_launch_state,
                                StandaloneLaunchRequest::ShadPs4(request),
                            );
                        }
                    }
                }
            }
            let dos = plan.platform_id.as_deref() == Some(dosbox::DOS_PLATFORM_ID);
            if dos {
                dosbox::show(ui, &preview_identity(plan), standalone_launch_state);
            }
            if !(dos && plan.candidates.is_empty())
                && show_plan(
                    ui,
                    plan,
                    retroarch.as_ref(),
                    dolphin.as_ref(),
                    pcsx2.as_ref(),
                    duckstation.as_ref(),
                    ppsspp.as_ref(),
                    rpcs3.as_ref(),
                    xemu.as_ref(),
                    xenia.as_ref(),
                    melonds.as_ref(),
                    retroarch_launch_state,
                    dolphin_launch_state,
                    pcsx2_launch_state,
                    standalone_launch_state,
                    amiga_whdload.as_ref(),
                    amiga_whdload_launch_state,
                )
            {
                action = Some(LaunchReadinessPageAction::OpenDoctor);
            }
        }
    }
    if std::mem::take(&mut retroarch_launch_state.open_cheats_mods) {
        action = Some(LaunchReadinessPageAction::Navigate(
            crate::navigation::NavClick::View(crate::navigation::MainView::CheatsMods),
        ));
    }

    action
}

// This is the UI boundary for the complete launch plan and its per-adapter
// state; keeping these inputs explicit makes ownership and dispatch visible.
#[allow(clippy::too_many_arguments)]
fn show_plan(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    retroarch: Option<&RetroArchLaunchContext>,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xemu: Option<&XemuLaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
    melonds: Option<&MelonDsLaunchContext>,
    retroarch_launch_state: &mut RetroArchLaunchState,
    dolphin_launch_state: &mut DolphinLaunchState,
    pcsx2_launch_state: &mut Pcsx2LaunchState,
    standalone_launch_state: &mut StandaloneLaunchState,
    amiga_whdload: Option<&AmigaWHDLoadLaunchContext>,
    amiga_whdload_launch_state: &mut AmigaWHDLoadLaunchState,
) -> bool {
    let mut open_doctor = false;
    if let Some(context) = duckstation
        && plan.candidates.iter().any(|candidate| {
            matches!(
                &candidate.target,
                LaunchTarget::Standalone {
                    adapter_id: "duckstation",
                    ..
                }
            )
        })
    {
        for notice in context.notices() {
            ui.colored_label(ui.visuals().warn_fg_color, notice);
        }
    }
    if plan.candidates.is_empty() {
        widgets::empty_state(
            ui,
            "No launch options found",
            if matches!(plan.platform_id.as_deref(), Some("MSX" | "MSX2")) {
                "No openMSX installation or compatible RetroArch core was found. Install openMSX or configure it on PATH, then check readiness again. No configuration was changed."
            } else {
                "No installed RetroArch core is a candidate for this game's platform yet."
            },
            None,
        );
        return false;
    }
    if let Some(topology) = &plan.media_topology {
        widgets::card(ui, |ui| {
            ui.strong("Multi-disc launch composition");
            let total = topology
                .media_sequence
                .iter()
                .filter_map(|step| step.ordinal.as_ref().map(|ordinal| ordinal.number))
                .max()
                .unwrap_or(topology.media_sequence.len() as u16);
            if topology.missing_media.is_empty() {
                ui.label(format!("Complete set · {total} disc(s) in verified order"));
            } else {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "Incomplete set · {} required disc(s) missing",
                        topology.missing_media.len()
                    ),
                );
            }
            ui.label(&topology.explanation);
            for (index, step) in topology.media_sequence.iter().enumerate() {
                let ordinal = step
                    .ordinal
                    .as_ref()
                    .map(|ordinal| format!("Disc {}", ordinal.number))
                    .unwrap_or_else(|| format!("Medium {}", index + 1));
                let side = step
                    .side
                    .as_ref()
                    .map(|side| format!(" · Side {}", side.number))
                    .unwrap_or_default();
                ui.label(format!("{ordinal}{side}"));
                if let Some(source) = &step.preferred_representation {
                    ui.small(format!("Source evidence: {}", source.path.display()));
                }
            }
            for missing in &topology.missing_media {
                ui.colored_label(ui.visuals().warn_fg_color, &missing.detail);
            }
        });
    }
    for candidate in &plan.candidates {
        if matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "cemu",
                ..
            }
        ) {
            continue;
        }
        #[cfg(target_os = "linux")]
        if matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "shadps4",
                ..
            }
        ) {
            continue;
        }
        ui.add_space(6.0);
        open_doctor |= show_candidate(
            ui,
            plan,
            candidate,
            retroarch,
            dolphin,
            pcsx2,
            duckstation,
            ppsspp,
            rpcs3,
            xemu,
            xenia,
            melonds,
            retroarch_launch_state,
            dolphin_launch_state,
            pcsx2_launch_state,
            standalone_launch_state,
            amiga_whdload,
            amiga_whdload_launch_state,
        );
    }
    open_doctor
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

/// Draws the human-facing recipe for the exact candidate already present in
/// the shared launch plan. For RetroArch and PCSX2 this projects the same
/// typed command builders used by preflight; adapter preflight still owns
/// executable discovery, identity revalidation, and final launch authority.
#[allow(clippy::too_many_arguments)]
fn show_launch_recipe(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    retroarch: Option<&RetroArchLaunchContext>,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
    melonds: Option<&MelonDsLaunchContext>,
) {
    let (emulator, profile) = target_labels(&candidate.target);
    let system = plan.platform_id.as_deref().unwrap_or("Unknown system");
    let input = candidate
        .content
        .resolved_path
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "No runnable game file has been resolved".into());
    let command = launch_command_preview(
        plan,
        candidate,
        retroarch,
        dolphin,
        pcsx2,
        duckstation,
        ppsspp,
        rpcs3,
        xenia,
        melonds,
    );
    let readiness = readiness_label_and_tone(candidate.readiness);

    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Launch recipe").strong());
        detail_label(ui, "System", system);
        detail_label(ui, "Emulator", &emulator);
        detail_label(ui, "Profile", &profile);
        detail_label(ui, "Game file", &input);
        if let Some(note) = crate::multidisc_launch_view::launch_note(&multidisc_decision(
            candidate,
            retroarch,
            duckstation,
        )) {
            detail_label(ui, "Discs", &note);
        }
        if let Some(command) = &command {
            detail_label(ui, "Executable", &command.executable.display().to_string());
            detail_label(
                ui,
                "Working directory",
                command
                    .working_directory
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .as_deref()
                    .unwrap_or("Inherited"),
            );
            ui.label(egui::RichText::new("Arguments").strong());
            for (index, argument) in command.arguments.iter().enumerate() {
                detail_label(ui, &format!("[{index}]"), &argument.to_string_lossy());
            }
        } else {
            detail_label(ui, "Command", "Final command resolved at launch");
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Readiness").strong());
            widgets::status_badge(ui, readiness.0, readiness.1);
        });
        if candidate.firmware != FirmwareReadiness::NotRequired {
            let firmware = firmware_summary(candidate.firmware, &candidate.blockers);
            detail_label(ui, "Firmware", firmware.status);
        }
        if let Some(blocker) = candidate.blockers.first() {
            detail_label(ui, "Why", &blocker.detail);
        }
        ui.label(
            egui::RichText::new(
                "Preview only. EmuWiz rechecks this command immediately before launch.",
            )
            .small()
            .color(theme::muted(ui)),
        );
    });
}

fn preview_identity(plan: &LaunchPlan) -> CanonicalIdentityStatus {
    match (&plan.platform_id, &plan.game_key) {
        (Some(platform_id), Some(game_key)) => {
            CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: platform_id.clone(),
                game_key: game_key.clone(),
            })
        }
        _ => CanonicalIdentityStatus::Unknown,
    }
}

#[allow(clippy::too_many_arguments)]
fn launch_command_preview(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    retroarch: Option<&RetroArchLaunchContext>,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
    melonds: Option<&MelonDsLaunchContext>,
) -> Option<LaunchCommandSpec> {
    let identity = preview_identity(plan);
    match &candidate.target {
        LaunchTarget::RetroArchCore { .. } => {
            build_retroarch_command_plan(&identity, candidate, &retroarch?.environment)
                .command
                .map(|command| {
                    let mut spec = command.command_spec();
                    crate::multidisc_launch_view::preview_arguments(
                        &mut spec.arguments,
                        candidate.content.resolved_path.as_deref(),
                        &multidisc_decision(candidate, retroarch, None),
                    );
                    spec
                })
        }
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } => match *adapter_id {
            "dolphin" => {
                let context = dolphin?;
                let profile = context
                    .discovery
                    .profiles
                    .iter()
                    .find(|profile| &profile.profile_id == profile_id)?;
                let binding = resolve_dolphin_native_launch_binding(profile, &context.roots);
                build_dolphin_command_plan(&identity, candidate, &binding)
                    .command
                    .map(|command| command.command_spec())
            }
            "pcsx2" => {
                let context = pcsx2?;
                let serial = context.verified_ps2_serial.as_deref();
                let profile = context
                    .discovery
                    .profiles
                    .iter()
                    .find(|profile| &profile.profile_id == profile_id)?;
                let binding = resolve_pcsx2_native_launch_binding(profile, &context.roots);
                build_pcsx2_command_plan(&identity, serial, candidate, &binding)
                    .command
                    .map(|command| command.command_spec())
            }
            "duckstation" => {
                let context = duckstation?;
                let profile = context
                    .discovery
                    .profiles
                    .iter()
                    .find(|profile| &profile.profile_id == profile_id)?;
                let binding =
                    archivefs_core::patch_manager::resolve_duckstation_native_launch_binding(
                        profile,
                        &context.roots,
                    );
                build_duckstation_command_plan(
                    &identity,
                    context.verified_ps1_serial.as_deref(),
                    candidate,
                    &binding,
                )
                .command
                .map(|command| {
                    let mut spec = command.command_spec();
                    crate::multidisc_launch_view::preview_arguments(
                        &mut spec.arguments,
                        candidate.content.resolved_path.as_deref(),
                        &multidisc_decision(candidate, None, Some(context)),
                    );
                    spec
                })
            }
            "melonds" => {
                let context = melonds?;
                let profile = context
                    .discovery
                    .profiles
                    .iter()
                    .find(|profile| &profile.profile_id == profile_id)?;
                let binding =
                    archivefs_core::patch_manager::resolve_melonds_native_launch_binding(profile);
                archivefs_core::launch::build_melonds_command_plan(
                    &identity,
                    context.verified_game_key.as_deref(),
                    candidate,
                    &binding,
                )
                .command
                .map(|command| LaunchCommandSpec {
                    executable: command.executable,
                    arguments: command.arguments,
                    working_directory: command.working_directory,
                })
            }
            "ppsspp" => {
                let context = ppsspp?;
                let disc_id = context.verified_psp_disc_id.as_deref();
                let profile = context
                    .discovery
                    .profiles
                    .iter()
                    .find(|profile| &profile.profile_id == profile_id)?;
                let binding =
                    archivefs_core::patch_manager::resolve_ppsspp_native_launch_binding(profile);
                build_ppsspp_command_plan(&identity, disc_id, candidate, &binding)
                    .command
                    .map(|command| command.command_spec())
            }
            "rpcs3" => {
                let context = rpcs3?;
                let title_id = context.verified_ps3_title_id.as_deref();
                let profile = context
                    .discovery
                    .profiles
                    .iter()
                    .find(|profile| &profile.profile_id == profile_id)?;
                let binding =
                    archivefs_core::patch_manager::resolve_rpcs3_native_launch_binding(profile);
                build_rpcs3_command_plan(&identity, title_id, candidate, &binding)
                    .command
                    .map(|command| command.command_spec())
            }
            "xenia" => {
                let context = xenia?;
                let profile = context
                    .discovery
                    .profiles
                    .iter()
                    .find(|profile| &profile.profile_id == profile_id)?;
                let binding = archivefs_core::patch_manager::resolve_xenia_launch_binding(profile);
                build_xenia_command_plan(
                    &identity,
                    context.verified_xex_title_id.as_deref(),
                    context.verified_xex_media_id.as_deref(),
                    candidate,
                    &binding,
                )
                .command
                .map(|command| command.command_spec())
            }
            _ => None,
        },
    }
}

/// Plain-language firmware presentation, projected solely from the existing
/// shared readiness value and the existing launch blockers. It deliberately
/// does not derive a requirement from an emulator name, a filename, or a
/// filesystem probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FirmwareSummary {
    status: &'static str,
    description: &'static str,
    tone: widgets::StatusTone,
    show_doctor_action: bool,
}

fn firmware_summary(firmware: FirmwareReadiness, blockers: &[LaunchBlocker]) -> FirmwareSummary {
    let launch_blocked_by_missing_firmware = blockers
        .iter()
        .any(|blocker| blocker.kind == LaunchBlockerKind::RequiredFirmwareMissing);

    match firmware {
        FirmwareReadiness::Verified => FirmwareSummary {
            status: "Firmware ready",
            description: "EmuWiz recognised the required firmware by hash.",
            tone: widgets::StatusTone::Success,
            show_doctor_action: false,
        },
        FirmwareReadiness::PresentUnverified => FirmwareSummary {
            status: "Firmware unverified",
            description: "Firmware was found, but EmuWiz could not verify its contents.",
            tone: widgets::StatusTone::Warning,
            show_doctor_action: true,
        },
        FirmwareReadiness::Missing if launch_blocked_by_missing_firmware => FirmwareSummary {
            status: "Firmware missing",
            description: "This emulator needs firmware before it can launch.",
            tone: widgets::StatusTone::Blocked,
            show_doctor_action: true,
        },
        FirmwareReadiness::Missing => FirmwareSummary {
            status: "Firmware missing",
            description: "EmuWiz could not find firmware this emulator expects.",
            tone: widgets::StatusTone::Blocked,
            show_doctor_action: true,
        },
        FirmwareReadiness::Unknown => FirmwareSummary {
            status: "Firmware state unknown",
            description: "EmuWiz could not determine whether the required firmware is ready.",
            tone: widgets::StatusTone::Pending,
            show_doctor_action: true,
        },
        FirmwareReadiness::NotRequired => FirmwareSummary {
            status: "Firmware not required",
            description: "This launch option does not need separate firmware.",
            tone: widgets::StatusTone::Info,
            show_doctor_action: false,
        },
    }
}

/// Draws a concise, read-only firmware summary. Exact expected firmware
/// records remain in the adapter-specific diagnostic seams: a launch
/// candidate only carries the shared readiness projection, so this UI never
/// invents filenames or hashes it was not given.
fn show_firmware_summary(ui: &mut egui::Ui, candidate: &LaunchCandidate) -> bool {
    let summary = firmware_summary(candidate.firmware, &candidate.blockers);
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Firmware").strong());
        widgets::status_badge(ui, summary.status, summary.tone);
    });
    ui.label(
        egui::RichText::new(summary.description)
            .small()
            .color(theme::muted(ui)),
    );

    if summary.show_doctor_action {
        ui.label(
            egui::RichText::new(
                "EmuWiz verifies firmware; it does not supply BIOS or system ROM files.",
            )
            .small()
            .color(theme::muted(ui)),
        );
        return widgets::action_button(
            ui,
            "Open BIOS & Firmware",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked();
    }

    false
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

/// A validated CUE/BIN release is DuckStation content only; every other
/// standalone adapter keeps requiring one plain file.
fn standalone_container_allowed(adapter_id: &str, container: Option<LaunchContainerKind>) -> bool {
    match container {
        Some(LaunchContainerKind::PlainFile) => true,
        Some(LaunchContainerKind::CueBin) => adapter_id == "duckstation",
        _ => false,
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
    melonds: Option<&MelonDsLaunchContext>,
) -> Option<StandaloneLaunchRequest> {
    if candidate.readiness != LaunchReadiness::Ready
        || !candidate.blockers.is_empty()
        || !candidate.warnings.is_empty()
        || candidate.content.requires_mount
    {
        return None;
    }
    let path = candidate.content.resolved_path.clone()?;
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        profile_path,
    } = &candidate.target
    else {
        return None;
    };
    if !standalone_container_allowed(adapter_id, candidate.content.container) {
        return None;
    }
    let platform = plan.platform_id.clone()?;
    let game_key = plan.game_key.clone()?;
    match *adapter_id {
        "scummvm" => {
            // The plan's resolved key is the detector's engine:game ID; core
            // re-runs detection and rejects any disagreement. Only a real
            // (non-link) game directory and a named executable are offered.
            let folder_ok = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
            (platform == archivefs_core::launch::SCUMMVM_SUPPORTED_PLATFORM_ID && folder_ok)
                .then(|| {
                    profile_path.clone().map(|expected_executable| {
                        StandaloneLaunchRequest::ScummVm(
                            archivefs_core::launch::ScummVmLaunchRequest {
                                selected_game_folder: path,
                                expected_game_key: game_key,
                                expected_executable,
                                trainer: None,
                            },
                        )
                    })
                })
                .flatten()
        }
        "openmsx" => {
            let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: platform.clone(),
                game_key: game_key.clone(),
            });
            let planned = archivefs_core::launch::openmsx_command::build_openmsx_command_plan(
                &identity, candidate,
            );
            let command = planned.command?;
            let executable = command.executable.clone();
            let content_metadata = std::fs::symlink_metadata(&path).ok()?;
            let executable_metadata = std::fs::symlink_metadata(&command.executable).ok()?;
            Some(StandaloneLaunchRequest::OpenMsx(
                archivefs_core::launch::OpenMsxLaunchRequest {
                    selected_content_path: path,
                    expected_platform_id: platform,
                    expected_game_key: game_key,
                    profile_id: profile_id.clone(),
                    expected_executable: command.executable,
                    expected_machine: command.machine.into(),
                    content_identity:
                        archivefs_core::launch::process_spawn::CapturedFileIdentity::capture(
                            &content_metadata,
                        ),
                    executable_identity:
                        archivefs_core::launch::process_spawn::CapturedFileIdentity::capture(
                            &executable_metadata,
                        ),
                },
                archivefs_core::patch_manager::OpenMsxProfileDiscoveryRoots {
                    explicit_executables: vec![executable],
                    path_env: None,
                },
            ))
        }
        "mame" => mame_launch_request(plan, candidate)
            .ok()
            .map(StandaloneLaunchRequest::Mame),
        "duckstation" => {
            let context = duckstation?;
            let decision = multidisc_decision(candidate, None, Some(context));
            if matches!(
                decision,
                archivefs_core::launch::MultiDiscLaunch::Refused { .. }
            ) {
                return None;
            }
            let playlist = context
                .multidisc
                .as_ref()
                .and_then(|multidisc| multidisc.handoff(&decision));
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
                    expected_cue_members: None, // bound to the displayed preview by the launch state
                    selected_content_path: path,
                    expected_platform_id: platform,
                    expected_game_key: game_key,
                    expected_ps1_serial: serial,
                    profile_id: profile.profile_id.clone(),
                    expected_executable: binding.executable,
                    expected_user_directory_mode: binding.user_directory_mode,
                    expected_installation: binding.installation,
                },
                context.roots.clone(),
                context.firmware_evidence.clone(),
                playlist,
            ))
        }
        "melonds" => {
            let context = melonds?;
            // The independently verified key is the identity; a plan key that
            // disagrees with it is never launched.
            let game_key = context
                .verified_game_key
                .clone()
                .filter(|key| key == &game_key)?;
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?;
            let binding =
                archivefs_core::patch_manager::resolve_melonds_native_launch_binding(profile)
                    .ok()?;
            // Only a real file is ever offered: a link or special file could
            // never launch, so no Launch button is shown for it.
            let identity_of = |path: &std::path::Path| {
                std::fs::symlink_metadata(path)
                    .ok()
                    .filter(|meta| meta.file_type().is_file())
                    .map(|meta| {
                        archivefs_core::launch::process_spawn::CapturedFileIdentity::capture(&meta)
                    })
            };
            // The configuration is guarded like the ROM and executable. If it
            // is missing, a link or not a regular file there is no request;
            // the core requires this identity and always compares it.
            let config_identity = identity_of(profile.config_path.as_deref()?)?;
            Some(StandaloneLaunchRequest::MelonDs(
                archivefs_core::launch::MelonDsLaunchRequest {
                    content_identity: identity_of(&path)?,
                    executable_identity: identity_of(&binding.executable)?,
                    config_identity,
                    selected_content_path: path,
                    expected_platform_id: platform,
                    expected_game_key: game_key,
                    profile_id: profile.profile_id.clone(),
                    expected_executable: binding.executable,
                    expected_installation: binding.installation,
                },
                context.roots.clone(),
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
                    expected_installation: binding.installation,
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

fn mame_launch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Result<MameLaunchRequest, LaunchBlocker> {
    let path = candidate.content.resolved_path.clone().ok_or_else(|| {
        mame_blocker(
            LaunchBlockerKind::MameSetVerdictUnavailable,
            "the trusted MAME logical-set path is unavailable",
        )
    })?;
    let LaunchTarget::Standalone {
        adapter_id,
        profile_path,
        ..
    } = &candidate.target
    else {
        return Err(mame_blocker(
            LaunchBlockerKind::MameEmulatorUnavailable,
            "the MAME launch target is unavailable",
        ));
    };
    if *adapter_id != "mame" {
        return Err(mame_blocker(
            LaunchBlockerKind::MameEmulatorUnavailable,
            "the selected launch target is not MAME",
        ));
    }
    let executable = profile_path.clone().ok_or_else(|| {
        mame_blocker(
            LaunchBlockerKind::MameEmulatorUnavailable,
            "no exact MAME executable is selected",
        )
    })?;
    let platform_id = plan.platform_id.clone().ok_or_else(|| {
        mame_blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "the Arcade platform identity is unresolved",
        )
    })?;
    let game_key = plan.game_key.clone().ok_or_else(|| {
        mame_blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "the MAME logical-set identity is unresolved",
        )
    })?;
    let database_path = archivefs_core::default_database_path().map_err(|error| {
        mame_blocker(
            LaunchBlockerKind::MameSetVerdictUnavailable,
            format!("the MAME audit database path is unavailable: {error}"),
        )
    })?;
    let database = archivefs_core::Database::open_read_only(database_path).map_err(|error| {
        mame_blocker(
            LaunchBlockerKind::MameSetVerdictUnavailable,
            format!("the MAME audit database cannot be read: {error}"),
        )
    })?;
    let evidence = database
        .mame_arcade_join_for_archive_path(
            archivefs_core::dat::mame_arcade_join::MAME_0174_SHA256,
            &path,
        )
        .map_err(|error| {
            mame_blocker(
                LaunchBlockerKind::MameSetVerdictUnavailable,
                format!("the exact MAME audit evidence cannot be read: {error}"),
            )
        })?
        .ok_or_else(|| {
            mame_blocker(
                LaunchBlockerKind::MameSetVerdictUnavailable,
                "no current exact-path MAME audit evidence is available",
            )
        })?;
    let resolution =
        archivefs_core::dat::mame_arcade_join::launch_resolution_for_join(&evidence, &path)
            .ok_or_else(|| {
                mame_blocker(
                    LaunchBlockerKind::MameSetVerdictUnavailable,
                    "the persisted MAME audit does not authorize this logical set",
                )
            })?;
    let rom_search_path = path.parent().map(Path::to_path_buf).ok_or_else(|| {
        mame_blocker(
            LaunchBlockerKind::MameSearchPathUnconfigured,
            "the trusted MAME collection root cannot be determined",
        )
    })?;
    Ok(MameLaunchRequest {
        identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id,
            game_key,
        }),
        set_resolutions: vec![resolution],
        expected_executable: executable,
        selected_content: path,
        expected_content_identity: None,
        rom_search_path,
    })
}

fn mame_blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

/// Reconcile generic launch planning with the actual strict MAME preflight.
/// A MAME card can only remain Ready when the same typed request used by the
/// launch button passes core preflight now.
pub(crate) fn project_mame_strict_readiness(plan: &mut LaunchPlan, strict: &MameCommandPlan) {
    for candidate in &mut plan.candidates {
        let LaunchTarget::Standalone { adapter_id, .. } = &candidate.target else {
            continue;
        };
        if *adapter_id != "mame" {
            continue;
        }
        for blocker in &strict.blockers {
            if !candidate.blockers.contains(blocker) {
                candidate.blockers.push(blocker.clone());
            }
        }
        candidate.readiness = if !candidate.blockers.is_empty() {
            LaunchReadiness::Blocked
        } else if !candidate.warnings.is_empty() {
            LaunchReadiness::ReadyWithWarnings
        } else {
            LaunchReadiness::Ready
        };
    }
    plan.summary.ready = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::Ready)
        .count();
    plan.summary.ready_with_warnings = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::ReadyWithWarnings)
        .count();
    plan.summary.blocked = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::Blocked)
        .count();
}

pub(crate) fn apply_mame_strict_preflight(plan: &mut LaunchPlan) {
    for index in 0..plan.candidates.len() {
        let snapshot = plan.candidates[index].clone();
        let is_mame = matches!(
            &snapshot.target,
            LaunchTarget::Standalone {
                adapter_id: "mame",
                ..
            }
        );
        if !is_mame {
            continue;
        }
        let blockers = match mame_launch_request(plan, &snapshot) {
            Ok(request) => preflight_mame_launch(&request)
                .err()
                .map(|error| error.blockers)
                .unwrap_or_default(),
            Err(blocker) => vec![blocker],
        };
        if !blockers.is_empty() {
            block_mame_candidate(plan, index, blockers);
        }
    }
}

fn block_mame_candidate(plan: &mut LaunchPlan, index: usize, blockers: Vec<LaunchBlocker>) {
    let candidate = &mut plan.candidates[index];
    candidate.readiness = LaunchReadiness::Blocked;
    candidate.blockers = blockers;
    plan.summary.ready = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::Ready)
        .count();
    plan.summary.ready_with_warnings = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::ReadyWithWarnings)
        .count();
    plan.summary.blocked = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::Blocked)
        .count();
}

#[allow(clippy::too_many_arguments)]
fn typed_launch_request(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    retroarch: Option<&RetroArchLaunchContext>,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xemu: Option<&XemuLaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
    melonds: Option<&MelonDsLaunchContext>,
    amiga_whdload: Option<&AmigaWHDLoadLaunchContext>,
) -> Option<TypedLaunchRequest> {
    if let Some(request) = retroarch_launch_request(plan, candidate, retroarch) {
        let playlist = retroarch_playlist(candidate, retroarch);
        return Some(TypedLaunchRequest::RetroArch(request, playlist));
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
    if let Some(context) = amiga_whdload
        && let LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } = &candidate.target
        && let Some((candidate_adapter, candidate_profile, request)) =
            context
                .requests
                .iter()
                .find(|(candidate_adapter, candidate_profile, _)| {
                    candidate_adapter == adapter_id && candidate_profile == profile_id
                })
        && candidate.readiness == LaunchReadiness::Ready
        && candidate.blockers.is_empty()
        && candidate.warnings.is_empty()
    {
        let _ = (candidate_adapter, candidate_profile);
        return Some(TypedLaunchRequest::AmigaWHDLoad(request.clone()));
    }
    if let Some(context) = amiga_whdload
        && let LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } = &candidate.target
        && let Some((_, _, request)) = context
            .adf
            .iter()
            .find(|(adapter, profile, _)| adapter == adapter_id && profile == profile_id)
        && candidate.readiness == LaunchReadiness::Ready
        && candidate.blockers.is_empty()
        && candidate.warnings.is_empty()
    {
        return Some(TypedLaunchRequest::Standalone(
            StandaloneLaunchRequest::FsUaeAdf(Box::new(request.clone())),
        ));
    }
    standalone_launch_request(
        plan,
        candidate,
        duckstation,
        ppsspp,
        rpcs3,
        xemu,
        xenia,
        melonds,
    )
    .map(TypedLaunchRequest::Standalone)
}

fn candidate_emulator_name(candidate: &LaunchCandidate) -> Option<&'static str> {
    match &candidate.target {
        LaunchTarget::Standalone { adapter_id, .. } => match *adapter_id {
            "mesen" => Some("Mesen"),
            "duckstation" => Some("DuckStation"),
            "pcsx2" => Some("PCSX2"),
            "ppsspp" => Some("PPSSPP"),
            "melonds" => Some("melonDS"),
            "rpcs3" => Some("RPCS3"),
            "shadps4" => Some("shadPS4"),
            "xemu" => Some("xemu"),
            "cemu" => Some("Cemu"),
            "xenia" => Some("Xenia"),
            "amiberry" => Some("Amiberry"),
            "fsuae" => Some("FS-UAE"),
            "dolphin" => Some("Dolphin"),
            "scummvm" => Some("ScummVM"),
            "fuse" => Some("Fuse"),
            "tsugaru" => Some("Tsugaru"),
            "xroar" => Some("XRoar"),
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

    // A discovered but unsupported Mesen contract is distinct from absence
    // of an emulator; an unscanned RetroArch lane must not hide this reason.
    if let Some(blocker) = plan.candidates.iter().find_map(|candidate| {
        matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "mesen",
                ..
            }
        )
        .then(|| {
            candidate
                .blockers
                .iter()
                .find(|b| b.kind == LaunchBlockerKind::MesenBindingUnavailable)
        })
        .flatten()
    }) {
        return GamerBlocker::for_emulator(
            GamerBlockerKind::EmulatorSetupIncomplete,
            "Mesen",
            blocker.detail.clone(),
        );
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
                "melonds" => "melonDS",
                "rpcs3" => "RPCS3",
                "xemu" => "xemu",
                "xenia" => "Xenia",
                "dolphin" => "Dolphin",
                "snes9x" => "Snes9x",
                "openmsx" => "openMSX",
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
    retroarch: Option<&RetroArchLaunchContext>,
    dolphin: Option<&DolphinLaunchContext>,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
    rpcs3: Option<&Rpcs3LaunchContext>,
    xemu: Option<&XemuLaunchContext>,
    xenia: Option<&XeniaLaunchContext>,
    melonds: Option<&MelonDsLaunchContext>,
    retroarch_launch_state: &mut RetroArchLaunchState,
    dolphin_launch_state: &mut DolphinLaunchState,
    pcsx2_launch_state: &mut Pcsx2LaunchState,
    standalone_launch_state: &mut StandaloneLaunchState,
    amiga_whdload: Option<&AmigaWHDLoadLaunchContext>,
    amiga_whdload_launch_state: &mut AmigaWHDLoadLaunchState,
) -> bool {
    let mut open_doctor = false;
    widgets::card(ui, |ui| {
        let (name, profile) = target_labels(&candidate.target);
        let strict_mame_blocked = candidate.readiness == LaunchReadiness::Blocked
            && matches!(
                &candidate.target,
                LaunchTarget::Standalone {
                    adapter_id: "mame",
                    ..
                }
            );
        let (readiness_label, readiness_tone) = if strict_mame_blocked {
            ("Needs attention", widgets::StatusTone::Blocked)
        } else {
            readiness_label_and_tone(candidate.readiness)
        };

        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(ui, readiness_label, readiness_tone);
            ui.label(egui::RichText::new(&name).strong().size(15.0));
        });
        ui.label(
            egui::RichText::new(&profile)
                .small()
                .color(theme::muted(ui)),
        );
        if strict_mame_blocked {
            ui.label("MAME cannot launch this set yet.");
        }
        ui.label(
            egui::RichText::new(format!(
                "Preference: {}",
                preference_label(candidate.preference)
            ))
            .small()
            .color(theme::muted(ui)),
        );

        show_launch_recipe(
            ui,
            plan,
            candidate,
            retroarch,
            dolphin,
            pcsx2,
            duckstation,
            ppsspp,
            rpcs3,
            xenia,
            melonds,
        );

        if let Some(context) = dolphin {
            dolphin_profile_guidance::show(ui, plan, candidate, context);
        }
        controller_readiness::show(ui, plan, candidate, pcsx2);

        open_doctor = show_firmware_summary(ui, candidate);

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

        if matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "mesen",
                ..
            }
        ) {
            ui.add_enabled(false, egui::Button::new("Launch Mesen"))
                .on_hover_text(crate::emulator_setup::mesen::NATIVE_CONTRACT_BLOCKER);
        }
        if let Some(request) = retroarch_launch_request(plan, candidate, retroarch) {
            ui.add_space(6.0);
            let playlist = retroarch_playlist(candidate, retroarch);
            show_launch_action(ui, retroarch_launch_state, request, playlist);
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
        if let Some(request) = standalone_launch_request(
            plan,
            candidate,
            duckstation,
            ppsspp,
            rpcs3,
            xemu,
            xenia,
            melonds,
        ) {
            ui.add_space(6.0);
            if let StandaloneLaunchRequest::OpenMsx(ref request, _) = request {
                ui.small(format!(
                    "openMSX installation found · planned machine {} · direct {} cartridge (machine files are not inspected)",
                    request.expected_machine,
                    if request.expected_platform_id == "MSX" {
                        ".mx1"
                    } else {
                        ".mx2"
                    },
                ));
            }
            show_standalone_launch_action(ui, standalone_launch_state, request);
        }
        if let Some(context) = amiga_whdload
            && let Some(TypedLaunchRequest::AmigaWHDLoad(request)) = typed_launch_request(
                plan,
                candidate,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(context),
            )
        {
            ui.add_space(6.0);
            show_amiga_whdload_launch_action(ui, amiga_whdload_launch_state, request);
        }
        if let Some(context) = amiga_whdload
            && let Some(TypedLaunchRequest::Standalone(
                request @ StandaloneLaunchRequest::FsUaeAdf(_),
            )) = typed_launch_request(
                plan,
                candidate,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(context),
            )
        {
            ui.add_space(6.0);
            show_standalone_launch_action(ui, standalone_launch_state, request);
        }
    });
    open_doctor
}

/// What EmuWiz can honestly say once melonDS has exited: the process result,
/// never whether the game booted or played.
fn melonds_exit_line(report: &archivefs_core::launch::process_spawn::ProcessExitReport) -> String {
    exit_line("melonDS", report)
}

/// The same honest wording for any adapter: a process result only.
fn exit_line(
    name: &str,
    report: &archivefs_core::launch::process_spawn::ProcessExitReport,
) -> String {
    match &report.status {
        Ok(status) if status.success() => {
            format!("{name} exited normally; this does not prove the game ran.")
        }
        Ok(status) => {
            let stderr = String::from_utf8_lossy(&report.stderr);
            let stderr = stderr.trim();
            if stderr.is_empty() {
                format!("{name} failed: {status}")
            } else {
                let shown: String = stderr.chars().take(400).collect();
                format!("{name} failed: {status} · {shown}")
            }
        }
        Err(error) => format!("{name} could not be watched to completion: {error}"),
    }
}

fn show_standalone_launch_action(
    ui: &mut egui::Ui,
    state: &mut StandaloneLaunchState,
    mut request: StandaloneLaunchRequest,
) {
    if let Err(detail) = state.bind_cue_review(&mut request) {
        ui.label(detail);
        return;
    }
    let (path, adapter) = request.key();
    let same = state
        .tracked
        .as_ref()
        .is_some_and(|(p, a, _)| *p == path && *a == adapter);
    melonds_cheats::show_chooser(ui, state, &request);
    scummvm_profile::show_chooser(ui, state, &request);
    if same {
        let label = match state.tracked.as_ref().map(|(_, _, stage)| stage) {
            Some(StandaloneLaunchStage::Starting(_)) => "Launching…",
            Some(StandaloneLaunchStage::Running(_)) => "Running",
            Some(StandaloneLaunchStage::Exited(_)) => "Process exited",
            Some(StandaloneLaunchStage::Failed(_)) => "Launch failed",
            None => "Launch",
        };
        ui.label(label);
        match state.tracked.as_mut() {
            Some((
                _,
                _,
                StandaloneLaunchStage::Exited(StandaloneProcess::MelonDsCheats(session)),
            )) => {
                ui.small("Historical result from the last launch attempt; current readiness is shown above.");
                for line in melonds_cheats::session_summary(session) {
                    ui.label(line);
                }
            }
            Some((_, _, StandaloneLaunchStage::Running(StandaloneProcess::MelonDsCheats(_)))) => {
                ui.small(melonds_cheats::activation_label());
            }
            Some((
                _,
                _,
                StandaloneLaunchStage::Running(StandaloneProcess::ScummVmTrainer(process)),
            )) => {
                ui.small("ScummVM process started with the saved trainer profile you chose. Starting is all EmuWiz can confirm: it cannot see the game, and ScummVM does not report whether a trainer option took effect.");
                if !process.inherited.is_empty() {
                    ui.small(format!(
                        "Kept from your ScummVM settings: {}.",
                        process
                            .inherited
                            .iter()
                            .map(|(k, _)| k.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                for (key, reason) in &process.not_inherited {
                    ui.small(format!("Not carried over: {key} ({reason})."));
                }
            }
            Some((
                _,
                _,
                StandaloneLaunchStage::Running(StandaloneProcess::DuckStation(process)),
            )) => {
                ui.small(format!("DuckStation running · PID {}", process.pid));
                ui.small("Starting the process does not prove the game ran.");
            }
            Some((_, _, StandaloneLaunchStage::Running(StandaloneProcess::ScummVm(_)))) => {
                ui.small("ScummVM process started; EmuWiz cannot tell whether the game itself is running.");
            }
            Some((_, _, StandaloneLaunchStage::Running(StandaloneProcess::OpenMsx(process)))) => {
                ui.small(format!("openMSX running · PID {}", process.pid));
                ui.small("Starting the process does not prove the game ran.");
            }
            Some((_, _, StandaloneLaunchStage::Running(StandaloneProcess::DosBox(process)))) => {
                ui.small(format!(
                    "DOSBox running in the sandbox · PID {}",
                    process.pid
                ));
                ui.small("Starting the process does not prove the game ran.");
            }
            Some((_, _, StandaloneLaunchStage::Exited(process))) => {
                ui.small("Historical result from the last launch attempt; current readiness is shown above.");
                if let StandaloneProcess::DuckStation(process) = process
                    && let Some(report) = process.poll()
                {
                    ui.label(exit_line("DuckStation", report));
                }
                if let StandaloneProcess::MelonDs(process) = process
                    && let Some(report) = process.poll()
                {
                    ui.label(melonds_exit_line(report));
                }
                if let StandaloneProcess::ScummVm(process) = process
                    && let Some(report) = process.poll()
                {
                    ui.label(exit_line("ScummVM", report));
                }
                if let StandaloneProcess::OpenMsx(process) = process
                    && let Some(report) = process.poll()
                {
                    ui.label(exit_line("openMSX", report));
                }
                if let StandaloneProcess::DosBox(process) = process
                    && let Some(report) = process.poll()
                {
                    ui.label(exit_line("DOSBox", report));
                }
                if let StandaloneProcess::ScummVmTrainer(process) = process {
                    if let Some(report) = process.poll() {
                        ui.label(exit_line("ScummVM", report));
                    }
                    ui.label("It was started with the saved trainer profile you chose. EmuWiz did not observe whether any trainer option took effect.");
                    if let Some(Err(e)) = process.cleanup_result() {
                        ui.label(format!("The private profile copy could not be removed: {e}. Your own ScummVM configuration and saves were not changed."));
                    }
                }
            }
            Some((_, _, StandaloneLaunchStage::Failed(detail))) => {
                ui.small("Historical result from the last launch attempt; current readiness is shown above.");
                ui.label(
                    egui::RichText::new(detail.as_str())
                        .small()
                        .color(theme::muted(ui)),
                );
            }
            _ => {}
        }
        if adapter == "duckstation" && !state.is_active() {
            let changed = state.cue_review_changes();
            if !changed.is_empty() {
                ui.label(format!(
                    "The CUE/BIN files changed after the preview: {}. Review their current state before another launch.",
                    changed.join(", ")
                ));
                if ui.button("Recheck CUE/BIN files").clicked() {
                    state.clear_cue_review();
                    state.tracked = None;
                    return;
                }
            } else if ui.button("Launch DuckStation again (recheck)").clicked() {
                state.start(request);
                return;
            }
        }
        if adapter == "flycast"
            && !state.is_active()
            && ui.button("Launch Flycast again (recheck)").clicked()
        {
            state.start(request);
            return;
        }
        if adapter == "melonds"
            && !state.is_active()
            && ui.button("Launch melonDS again (recheck)").clicked()
        {
            state.start(request);
            return;
        }
        if adapter == "scummvm"
            && !state.is_active()
            && ui.button("Launch ScummVM again (recheck)").clicked()
        {
            state.start(request);
            return;
        }
        if adapter == dosbox::ADAPTER_KEY
            && !state.is_active()
            && ui.button("Play in DOSBox again (recheck)").clicked()
        {
            state.start(request);
            return;
        }
        #[cfg(target_os = "linux")]
        if adapter == "shadps4"
            && !state.is_active()
            && ui.button("Launch shadPS4 again (recheck)").clicked()
        {
            state.start(request);
        }
        return;
    }
    let label = match state.melonds_cheats.as_ref() {
        Some(choice) if adapter == "melonds" && choice.chosen_count() > 0 => {
            format!("Launch {adapter} with {} cheat(s)", choice.chosen_count())
        }
        _ if adapter == dosbox::ADAPTER_KEY => "Play in DOSBox".to_string(),
        // ScummVM's Play reads as the product name, like its recheck button.
        _ if adapter == "scummvm" => format!("Launch {}", request.adapter_name()),
        _ => format!("Launch {adapter}"),
    };
    if ui.button(label).clicked() {
        state.start(request);
    }
}

fn show_amiga_whdload_launch_action(
    ui: &mut egui::Ui,
    state: &mut AmigaWHDLoadLaunchState,
    request: WHDLoadLaunchInput,
) {
    crate::cheats_mods::whdload_trainer_flow::show_launch_status(
        ui,
        Some(request.profile.emulator),
    );
    let same_request = state.tracked.as_ref().is_some_and(|(path, adapter, _)| {
        let request_path = request
            .target
            .as_ref()
            .map(|target| target.package_path.as_path());
        let request_adapter = match request.profile.emulator {
            archivefs_core::patch_manager::AmigaEmulatorKind::Amiberry => "amiberry",
            archivefs_core::patch_manager::AmigaEmulatorKind::FsUae => "fsuae",
        };
        request_path == Some(path.as_path()) && request_adapter == adapter
    });
    if same_request {
        match state.tracked.as_ref().map(|(_, _, stage)| stage) {
            Some(AmigaWHDLoadStage::Starting(_)) => {
                ui.label("Launching WHDLoad…");
            }
            Some(AmigaWHDLoadStage::Running(_)) => {
                ui.label("WHDLoad running");
            }
            Some(AmigaWHDLoadStage::Exited(_)) => {
                ui.label("WHDLoad process exited");
                ui.small("Historical result from the last launch attempt; current readiness is shown above.");
            }
            Some(AmigaWHDLoadStage::Failed(detail)) => {
                ui.label("WHDLoad launch failed");
                ui.small("Historical result from the last launch attempt.");
                ui.label(egui::RichText::new(detail).small().color(theme::muted(ui)));
            }
            None => {}
        }
        return;
    }
    let label = match request.profile.emulator {
        archivefs_core::patch_manager::AmigaEmulatorKind::Amiberry => "Amiberry",
        archivefs_core::patch_manager::AmigaEmulatorKind::FsUae => "FS-UAE",
    };
    if ui.button(format!("Launch {label} WHDLoad")).clicked() {
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
    playlist: Option<crate::multidisc_launch_view::PlaylistHandoff>,
) {
    let busy = launch_state.is_active();
    if let Some(selected) = launch_state.classic_pokes.show(ui, &request, busy) {
        launch_state.start_with_pokes(request.clone(), selected);
    }
    let display = launch_state.display_for(&request);
    show_retroarch_launch_feedback(ui, &display);
    for line in launch_state.cheat_lines(&request) {
        ui.small(line);
    }
    retroarch_cheats::show_chooser(ui, launch_state, &request, playlist.is_some());
    let cheat_count = launch_state
        .cheats
        .as_ref()
        .filter(|choice| choice.matches(&request) && playlist.is_none())
        .map(retroarch_cheats::Choice::ticked_count)
        .filter(|count| *count > 0);
    let label = match cheat_count {
        Some(count) => format!("Play — Launch RetroArch with {count} cheat(s)"),
        None => "Play — Launch RetroArch".to_string(),
    };
    let launch =
        |state: &mut RetroArchLaunchState,
         request: RetroArchLaunchRequest,
         playlist: Option<crate::multidisc_launch_view::PlaylistHandoff>| {
            if cheat_count.is_some() && playlist.is_none() {
                state.start_with_cheats(request);
            } else {
                state.start(request, playlist);
            }
        };

    match display {
        RetroArchLaunchDisplay::Idle => {
            if ui.button(&label).clicked() {
                launch(launch_state, request, playlist);
            }
        }
        RetroArchLaunchDisplay::Starting => {
            ui.add_enabled(false, egui::Button::new("Starting RetroArch…"));
        }
        RetroArchLaunchDisplay::Running { .. } => {
            ui.add_enabled(false, egui::Button::new("Play — Launch RetroArch"));
        }
        RetroArchLaunchDisplay::Exited { .. } => {
            if ui.button(&label).clicked() {
                launch(launch_state, request, playlist);
            }
        }
        RetroArchLaunchDisplay::Failed { .. } => {
            if ui.button(&label).clicked() {
                launch(launch_state, request, playlist);
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
                ui.small("Historical result from the last launch attempt; current readiness is shown above.");
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
            ui.small(
                "Historical result from the last launch attempt; current readiness is shown above.",
            );
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
                ui.small("Historical result from the last launch attempt; current readiness is shown above.");
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
            ui.small(
                "Historical result from the last launch attempt; current readiness is shown above.",
            );
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
                ui.small("Historical result from the last launch attempt; current readiness is shown above.");
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
            ui.small(
                "Historical result from the last launch attempt; current readiness is shown above.",
            );
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
mod melonds_tests;
#[cfg(test)]
mod retroarch_cheat_sets_tests;
#[cfg(test)]
mod retroarch_cheats_tests;
#[cfg(test)]
pub(crate) use retroarch_cheats_tests::TestProbe;
#[cfg(test)]
mod scummvm_tests;
#[cfg(test)]
mod tests;

/// Game Details uses the exact same candidates and discovery as Launch. No
/// additional resolver or readiness mutation is involved.
pub(crate) fn show_dolphin_profile_guidance(ui: &mut egui::Ui, input: &LaunchReadinessInput) {
    if let LaunchReadinessInput::Plan {
        plan,
        dolphin: Some(context),
        ..
    } = input
    {
        for candidate in &plan.candidates {
            dolphin_profile_guidance::show(ui, plan, candidate, context);
        }
    }
}
