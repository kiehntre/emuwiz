//! Compact, read-only launch-readiness presentation for GUI-v2 Game Details.
//!
//! The projector consumes the existing launch input and never performs
//! discovery, identity resolution, firmware inspection, or launch planning.

use crate::launch_readiness_page::LaunchReadinessInput;
use archivefs_core::launch::{
    CandidatePreference, FirmwareReadiness, LaunchBlockerKind, LaunchCandidate, LaunchPlan,
    LaunchReadiness, LaunchTarget, LaunchWarningKind,
};
use eframe::egui;

mod diagnosis;
pub(crate) use diagnosis::{Attempt, AttemptTracker, Finding};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadinessPresentationState {
    Checking,
    Ready,
    ReadyWithWarnings,
    NeedsEmulator,
    NeedsFirmware,
    NeedsIdentityReview,
    SourceUnavailable,
    MediaUnreadable,
    Blocked,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadinessAction {
    Play,
    SetUpEmulator,
    CheckFirmware,
    ReviewIdentity,
    CheckGamesFolder,
    ReviewProblem,
    ChooseEmulator,
    RecheckReadiness,
    ReviewDiscSet,
    OpenActivity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IdentitySummary {
    StrongLocal,
    LaunchableWithWarning,
    NeedsReview,
    /// The saved catalogue holds an exact authoritative match, but launch
    /// still needs the file's own identity evidence.
    CatalogueVerified,
    Mismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceSummary {
    Available,
    Unavailable,
    Unreadable,
    RecheckNeeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FirmwareSummary {
    NotRequired,
    Ready,
    Missing,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadinessFreshness {
    Current,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EmulatorSummary {
    pub(crate) name: String,
    pub(crate) profile: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GameReadinessSummary {
    pub(crate) status: ReadinessPresentationState,
    pub(crate) headline: String,
    pub(crate) explanation: String,
    pub(crate) emulator: Option<EmulatorSummary>,
    pub(crate) firmware: FirmwareSummary,
    pub(crate) identity: IdentitySummary,
    pub(crate) source: SourceSummary,
    pub(crate) warnings: Vec<String>,
    pub(crate) primary_action: Option<ReadinessAction>,
    pub(crate) details: Vec<String>,
    pub(crate) freshness: ReadinessFreshness,
    /// Everything currently blocking launch, most actionable first.
    pub(crate) findings: Vec<Finding>,
    /// What happened the last time Play was pressed for this game.
    pub(crate) attempt: Option<Attempt>,
}

pub(crate) fn project(
    input: &LaunchReadinessInput,
    freshness: ReadinessFreshness,
) -> GameReadinessSummary {
    let mut summary = match input {
        LaunchReadinessInput::EvidenceNotLoaded | LaunchReadinessInput::RetroArchNotScanned => {
            checking_summary()
        }
        LaunchReadinessInput::AtariSt(snapshot) => {
            let mut summary = project_plan(&snapshot.plan(), true, true);
            summary.status = ReadinessPresentationState::Blocked;
            summary.headline = "Atari ST / Hatari readiness".into();
            summary.explanation = snapshot.reason();
            summary.identity = IdentitySummary::NeedsReview;
            summary.primary_action = None;
            summary.details.extend(snapshot.details());
            summary.details.push("This read-only media/profile projection never verifies a release or authorises Launch. Open Advanced details for each profile's TOS evidence and the existing backend command preview.".into());
            summary
        }
        LaunchReadinessInput::DosBoxReview => base(
            ReadinessPresentationState::Blocked,
            "Review the DOSBox start before playing",
            "Choose whether to approve the displayed game folder and start executable. User approval identifies a local launch target; it does not verify a game release against a DAT.",
            IdentitySummary::NeedsReview,
            SourceSummary::Available,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::Play),
        ),
        LaunchReadinessInput::IdentityUnknown => identity_summary(false),
        LaunchReadinessInput::DiscFilesUnresolved(detail) => base(
            ReadinessPresentationState::NeedsIdentityReview,
            "The disc's files need attention",
            &format!("{detail}. Restore or fix the file, then recheck."),
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::RecheckReadiness),
        ),
        LaunchReadinessInput::InspectionIncomplete => base(
            ReadinessPresentationState::NeedsIdentityReview,
            "Rescan to finish checking this game",
            archivefs_core::launch::INSPECTION_INCOMPLETE_REASON,
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::CheckGamesFolder),
        ),
        LaunchReadinessInput::IdentityConflicting => identity_summary(true),
        LaunchReadinessInput::Plan {
            plan,
            retroarch_scanned,
            standalone_scans_complete,
            ..
        } => project_plan(plan, *retroarch_scanned, *standalone_scans_complete),
    };
    summary.freshness = freshness;
    attach_duckstation_resolver_notices(&mut summary, input);
    attach_findings(&mut summary, input);
    if freshness == ReadinessFreshness::Stale {
        summary.status = ReadinessPresentationState::Stale;
        summary.headline = "Readiness needs to be checked again".into();
        summary.explanation =
            "The emulator, firmware, source, or game changed since this result.".into();
        summary.primary_action = Some(ReadinessAction::RecheckReadiness);
    }
    summary
}

/// The shared DuckStation resolver's fallback or refusal, shown with the
/// Launch readiness so it is never a silent substitution.
fn attach_duckstation_resolver_notices(
    summary: &mut GameReadinessSummary,
    input: &LaunchReadinessInput,
) {
    let LaunchReadinessInput::Plan {
        plan, duckstation, ..
    } = input
    else {
        return;
    };
    let Some(context) = duckstation else {
        return;
    };
    let has_duckstation = plan.candidates.iter().any(|candidate| {
        matches!(
            &candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "duckstation",
                ..
            }
        )
    });
    if !has_duckstation {
        return;
    }
    // The summary shows only its first warnings, so the resolver's notice goes
    // first; the general warnings stay, in order, behind it.
    let mut at = 0;
    for notice in context.notices() {
        if !summary.warnings.contains(&notice) {
            summary.warnings.insert(at, notice);
            at += 1;
        }
    }
}

/// Adds the full list of blockers to a not-ready summary and points the
/// primary action at the most actionable one.
fn attach_findings(summary: &mut GameReadinessSummary, input: &LaunchReadinessInput) {
    use diagnosis::{Cause, findings_for};
    let named_disc_refusal = summary
        .explanation
        .starts_with(archivefs_core::launch::evidence_bridge::CUE_BIN_REFUSED_PREFIX);
    let mut findings = match input {
        LaunchReadinessInput::IdentityUnknown | LaunchReadinessInput::IdentityConflicting => {
            diagnosis::single(Cause::IdentityNotConfirmed)
        }
        // Carries its own named reason and rescan action.
        LaunchReadinessInput::DosBoxReview | LaunchReadinessInput::DiscFilesUnresolved(_) => {
            Vec::new()
        }
        // Already carries its own reason and rescan action.
        LaunchReadinessInput::InspectionIncomplete => Vec::new(),
        LaunchReadinessInput::Plan { plan, .. } => {
            if plan.candidates.is_empty() {
                if summary.status == ReadinessPresentationState::NeedsEmulator {
                    diagnosis::single(Cause::EmulatorMissing)
                } else {
                    Vec::new()
                }
            } else {
                selected_candidate(plan)
                    .or_else(|| plan.candidates.iter().find(|c| !c.blockers.is_empty()))
                    .map(findings_for)
                    .unwrap_or_default()
            }
        }
        _ => Vec::new(),
    };
    // The named disc refusal already explains the missing/unsafe companion.
    // Do not replace its recheck with a misleading missing-drive finding;
    // unrelated firmware or emulator findings remain visible.
    if named_disc_refusal {
        findings.retain(|finding| finding.cause != Cause::GameFileUnavailable);
    }
    if findings.is_empty()
        || matches!(
            summary.status,
            ReadinessPresentationState::Ready | ReadinessPresentationState::ReadyWithWarnings
        )
    {
        return;
    }
    if !named_disc_refusal && let Some(fix) = findings[0].fix {
        summary.primary_action = Some(fix);
    }
    summary.findings = findings;
}

fn checking_summary() -> GameReadinessSummary {
    base(
        ReadinessPresentationState::Checking,
        "Checking whether this game is ready",
        "EmuWiz is checking the game, emulator, and any required system software.",
        IdentitySummary::NeedsReview,
        SourceSummary::RecheckNeeded,
        FirmwareSummary::Unknown,
        None,
        None,
    )
}

fn identity_summary(conflicting: bool) -> GameReadinessSummary {
    base(
        ReadinessPresentationState::NeedsIdentityReview,
        "This exact release is not confirmed",
        if conflicting {
            "Evidence for this game conflicts and needs review before launch."
        } else {
            "EmuWiz cannot confirm this exact release yet."
        },
        if conflicting {
            IdentitySummary::Mismatch
        } else {
            IdentitySummary::NeedsReview
        },
        SourceSummary::RecheckNeeded,
        FirmwareSummary::Unknown,
        None,
        Some(ReadinessAction::ReviewIdentity),
    )
}

/// Why a DOS game shows Play before its DOSBox checks have run.
pub(crate) const DOSBOX_REVIEW_EXPLANATION: &str = "DOS games run only in DOSBox inside a sandbox. \
     Play shows the game folder and start command for you to approve; nothing starts until you \
     approve them and every check passes.";

fn project_plan(
    plan: &LaunchPlan,
    retroarch_scanned: bool,
    standalone_scans_complete: bool,
) -> GameReadinessSummary {
    let mut details = Vec::new();
    details.push(format!("Launch candidates: {}", plan.candidates.len()));
    if let Some(platform) = &plan.platform_id {
        details.push(format!("Platform: {platform}"));
    }

    // DOS games are not planned through the shared candidate list: the
    // Launch page runs the DOSBox folder review and sandbox checks, and asks
    // for approval before anything starts. Play opens that page.
    if plan.candidates.is_empty()
        && plan.platform_id.as_deref()
            == Some(crate::launch_readiness_page::dosbox::DOS_PLATFORM_ID)
    {
        return with_details(
            base(
                ReadinessPresentationState::Blocked,
                "Review the DOSBox start before playing",
                DOSBOX_REVIEW_EXPLANATION,
                IdentitySummary::StrongLocal,
                SourceSummary::Available,
                FirmwareSummary::Unknown,
                None,
                Some(ReadinessAction::Play),
            ),
            details,
        );
    }

    if plan.candidates.is_empty() {
        return if !retroarch_scanned || !standalone_scans_complete {
            checking_summary_with_details(details)
        } else {
            base(
                ReadinessPresentationState::NeedsEmulator,
                "An emulator is needed",
                "EmuWiz found the game, but no safe launch setup is ready.",
                IdentitySummary::StrongLocal,
                SourceSummary::Available,
                FirmwareSummary::Unknown,
                None,
                Some(ReadinessAction::SetUpEmulator),
            )
        };
    }

    // A CUE/BIN disc that failed inspection already names the file at fault.
    if let Some(detail) = plan
        .candidates
        .iter()
        .flat_map(|candidate| &candidate.blockers)
        .find(|blocker| {
            blocker.kind == LaunchBlockerKind::ContentNotResolved
                && blocker
                    .detail
                    .starts_with(archivefs_core::launch::evidence_bridge::CUE_BIN_REFUSED_PREFIX)
        })
        .map(|blocker| blocker.detail.clone())
    {
        return with_details(
            base(
                ReadinessPresentationState::NeedsIdentityReview,
                "The disc's files need attention",
                &detail,
                IdentitySummary::StrongLocal,
                SourceSummary::RecheckNeeded,
                FirmwareSummary::Unknown,
                None,
                Some(ReadinessAction::RecheckReadiness),
            ),
            details,
        );
    }

    if plan.candidates.iter().any(|candidate| {
        candidate
            .blockers
            .iter()
            .any(|blocker| blocker.kind == LaunchBlockerKind::InspectionIncomplete)
    }) {
        return with_details(
            base(
                ReadinessPresentationState::NeedsIdentityReview,
                "Rescan to finish checking this game",
                archivefs_core::launch::INSPECTION_INCOMPLETE_REASON,
                IdentitySummary::NeedsReview,
                SourceSummary::RecheckNeeded,
                FirmwareSummary::Unknown,
                None,
                Some(ReadinessAction::CheckGamesFolder),
            ),
            details,
        );
    }

    if let Some(candidate) = plan.candidates.iter().find(|candidate| {
        candidate.blockers.iter().any(|blocker| {
            matches!(
                blocker.kind,
                LaunchBlockerKind::ContentNotResolved | LaunchBlockerKind::NoInstallationCandidate
            )
        })
    }) && candidate
        .blockers
        .iter()
        .any(|blocker| blocker.kind == LaunchBlockerKind::ContentNotResolved)
    {
        return with_details(
            base(
                ReadinessPresentationState::SourceUnavailable,
                "The game file is unavailable",
                "The library remembers this game, but its current folder cannot be reached.",
                IdentitySummary::StrongLocal,
                SourceSummary::Unavailable,
                firmware_for(candidate),
                emulator_for(candidate),
                Some(ReadinessAction::CheckGamesFolder),
            ),
            details,
        );
    }

    if let Some(candidate) = plan.candidates.iter().find(|candidate| {
        candidate.blockers.iter().any(|blocker| {
            matches!(
                blocker.kind,
                LaunchBlockerKind::MediaTopologyBlocked
                    | LaunchBlockerKind::MediaTopologyReviewRequired
                    | LaunchBlockerKind::ContentNotResolved
            )
        })
    }) {
        return with_details(
            base(
                ReadinessPresentationState::MediaUnreadable,
                "The game file could not be read",
                "EmuWiz could not complete the checks needed for this media.",
                IdentitySummary::StrongLocal,
                SourceSummary::Unreadable,
                firmware_for(candidate),
                emulator_for(candidate),
                Some(ReadinessAction::ReviewProblem),
            ),
            details,
        );
    }

    let selected = selected_candidate(plan);
    let eligible = plan
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.readiness != LaunchReadiness::Blocked && candidate.blockers.is_empty()
        })
        .count();
    if selected.is_none() && eligible > 1 {
        return with_details(
            base(
                ReadinessPresentationState::NeedsEmulator,
                "Choose how to play this game",
                "More than one safe emulator choice is available; choose one explicitly.",
                IdentitySummary::StrongLocal,
                SourceSummary::Available,
                FirmwareSummary::Unknown,
                None,
                Some(ReadinessAction::ChooseEmulator),
            ),
            details,
        );
    }

    let candidate = selected.or_else(|| plan.candidates.first());
    let Some(candidate) = candidate else {
        return checking_summary_with_details(details);
    };
    details.extend(candidate_details(candidate));

    if candidate.blockers.iter().any(|blocker| {
        matches!(
            blocker.kind,
            LaunchBlockerKind::RequiredFirmwareMissing
                | LaunchBlockerKind::Rpcs3FirmwareUnavailable
                | LaunchBlockerKind::Vita3kFirmwareUnavailable
                | LaunchBlockerKind::XRoarFirmwareMissing
                | LaunchBlockerKind::XRoarFirmwareUnavailable
                | LaunchBlockerKind::TsugaruFirmwareMissing
                | LaunchBlockerKind::TsugaruFirmwareUnavailable
        )
    }) || candidate.firmware == FirmwareReadiness::Missing
    {
        return with_details(
            base(
                ReadinessPresentationState::NeedsFirmware,
                "This system needs firmware",
                "The selected emulator needs system software before this game can start.",
                IdentitySummary::StrongLocal,
                source_for(candidate),
                FirmwareSummary::Missing,
                emulator_for(candidate),
                Some(ReadinessAction::CheckFirmware),
            ),
            details,
        );
    }

    if let Some(blocker) = candidate
        .blockers
        .iter()
        .find(|blocker| blocker.kind == LaunchBlockerKind::RetroArchArcadeSetNeedsArchive)
    {
        return with_details(
            base(
                ReadinessPresentationState::Blocked,
                "RetroArch cannot use this arcade layout",
                &blocker.detail,
                IdentitySummary::StrongLocal,
                source_for(candidate),
                firmware_for(candidate),
                emulator_for(candidate),
                Some(ReadinessAction::ReviewProblem),
            ),
            details,
        );
    }

    if let Some(blocker) = candidate.blockers.first() {
        let needs_emulator = matches!(
            blocker.kind,
            LaunchBlockerKind::NoInstallationCandidate
                | LaunchBlockerKind::ProfileIneligible
                | LaunchBlockerKind::CoreMissing
                | LaunchBlockerKind::RetroArchProfileMissing
                | LaunchBlockerKind::AmbiguousRetroArchProfile
                | LaunchBlockerKind::RetroArchCoreMismatch
                | LaunchBlockerKind::RetroArchExecutableMissing
                | LaunchBlockerKind::AmbiguousRetroArchExecutable
                | LaunchBlockerKind::RetroArchPathNotExact
        );
        return with_details(
            base(
                if needs_emulator {
                    ReadinessPresentationState::NeedsEmulator
                } else {
                    ReadinessPresentationState::Blocked
                },
                if needs_emulator {
                    "An emulator is needed"
                } else {
                    "This game is not ready yet"
                },
                if needs_emulator {
                    "EmuWiz found the game, but no safe launch setup is ready."
                } else {
                    "Review the reason below before trying again."
                },
                IdentitySummary::StrongLocal,
                source_for(candidate),
                firmware_for(candidate),
                emulator_for(candidate),
                Some(if needs_emulator {
                    ReadinessAction::SetUpEmulator
                } else {
                    ReadinessAction::ReviewProblem
                }),
            ),
            details,
        );
    }

    let mut warnings = candidate
        .warnings
        .iter()
        .map(|warning| warning.detail.clone())
        .collect::<Vec<_>>();
    if candidate.firmware == FirmwareReadiness::PresentUnverified {
        warnings.push("Firmware is present but not verified.".into());
    }
    let has_warning = candidate.readiness == LaunchReadiness::ReadyWithWarnings
        || !warnings.is_empty()
        || candidate.firmware == FirmwareReadiness::Unknown;
    let mut summary = base(
        if has_warning {
            ReadinessPresentationState::ReadyWithWarnings
        } else {
            ReadinessPresentationState::Ready
        },
        if has_warning {
            "Ready, with a warning"
        } else {
            "Ready to play"
        },
        if has_warning {
            "You can play, but review this note if you want more certainty."
        } else {
            "The selected emulator and game checks are ready."
        },
        if has_warning {
            IdentitySummary::LaunchableWithWarning
        } else {
            IdentitySummary::StrongLocal
        },
        source_for(candidate),
        firmware_for(candidate),
        emulator_for(candidate),
        Some(ReadinessAction::Play),
    );
    summary.warnings = warnings;
    summary.details = details;
    if !has_warning && let Some(emulator) = &summary.emulator {
        summary.explanation = format!(
            "EmuWiz can now prepare this game for {}. Checking readiness does not change your game files.",
            emulator.name
        );
    }
    summary
}

fn base(
    status: ReadinessPresentationState,
    headline: &str,
    explanation: &str,
    identity: IdentitySummary,
    source: SourceSummary,
    firmware: FirmwareSummary,
    emulator: Option<EmulatorSummary>,
    primary_action: Option<ReadinessAction>,
) -> GameReadinessSummary {
    GameReadinessSummary {
        status,
        headline: headline.into(),
        explanation: explanation.into(),
        emulator,
        firmware,
        identity,
        source,
        warnings: Vec::new(),
        primary_action,
        details: Vec::new(),
        freshness: ReadinessFreshness::Current,
        findings: Vec::new(),
        attempt: None,
    }
}

fn with_details(mut summary: GameReadinessSummary, details: Vec<String>) -> GameReadinessSummary {
    summary.details = details;
    summary
}

fn checking_summary_with_details(details: Vec<String>) -> GameReadinessSummary {
    with_details(checking_summary(), details)
}

pub(crate) fn selected_candidate(plan: &LaunchPlan) -> Option<&LaunchCandidate> {
    let preferred = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.preference != CandidatePreference::Undetermined)
        .collect::<Vec<_>>();
    match preferred.as_slice() {
        [candidate] => Some(candidate),
        _ => {
            let ready = plan
                .candidates
                .iter()
                .filter(|candidate| {
                    candidate.readiness != LaunchReadiness::Blocked && candidate.blockers.is_empty()
                })
                .collect::<Vec<_>>();
            (ready.len() == 1).then(|| ready[0])
        }
    }
}

fn candidate_details(candidate: &LaunchCandidate) -> Vec<String> {
    let mut details = Vec::new();
    if !candidate.content.provenance.is_empty() {
        details.push(format!("Content: {}", candidate.content.provenance));
    }
    details.push(format!("Preference: {:?}", candidate.preference));
    details.extend(
        candidate
            .blockers
            .iter()
            .map(|blocker| format!("Blocker: {:?} — {}", blocker.kind, blocker.detail)),
    );
    details.extend(
        candidate
            .warnings
            .iter()
            .map(|warning| format!("Warning: {:?} — {}", warning.kind, warning.detail)),
    );
    details
}

fn source_for(candidate: &LaunchCandidate) -> SourceSummary {
    if candidate.content.has_runnable_path() {
        SourceSummary::Available
    } else if candidate.content.requires_mount {
        SourceSummary::RecheckNeeded
    } else {
        SourceSummary::Unavailable
    }
}

fn firmware_for(candidate: &LaunchCandidate) -> FirmwareSummary {
    match candidate.firmware {
        FirmwareReadiness::NotRequired => FirmwareSummary::NotRequired,
        FirmwareReadiness::Verified => FirmwareSummary::Ready,
        FirmwareReadiness::PresentUnverified => FirmwareSummary::Unknown,
        FirmwareReadiness::Missing => FirmwareSummary::Missing,
        FirmwareReadiness::Unknown => FirmwareSummary::Unknown,
    }
}

fn emulator_for(candidate: &LaunchCandidate) -> Option<EmulatorSummary> {
    match &candidate.target {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } => Some(EmulatorSummary {
            name: human_emulator_name(adapter_id),
            profile: profile_id.clone(),
        }),
        LaunchTarget::RetroArchCore { core_stem, .. } => Some(EmulatorSummary {
            name: format!("RetroArch · {core_stem}"),
            profile: "selected RetroArch profile".into(),
        }),
    }
}

fn human_emulator_name(adapter_id: &str) -> String {
    match adapter_id {
        "duckstation" => "DuckStation",
        "pcsx2" => "PCSX2",
        "ppsspp" => "PPSSPP",
        "rpcs3" => "RPCS3",
        "flycast" => "Flycast",
        "dolphin" => "Dolphin",
        "mame" => "MAME",
        "amiberry" => "Amiberry",
        "fsuae" => "FS-UAE",
        other => other,
    }
    .into()
}

pub(crate) fn status_tone(status: ReadinessPresentationState) -> crate::ui::components::StatusTone {
    match status {
        ReadinessPresentationState::Ready => crate::ui::components::StatusTone::Success,
        ReadinessPresentationState::ReadyWithWarnings
        | ReadinessPresentationState::Checking
        | ReadinessPresentationState::Stale => crate::ui::components::StatusTone::Warning,
        ReadinessPresentationState::NeedsEmulator
        | ReadinessPresentationState::NeedsFirmware
        | ReadinessPresentationState::NeedsIdentityReview
        | ReadinessPresentationState::SourceUnavailable
        | ReadinessPresentationState::MediaUnreadable
        | ReadinessPresentationState::Blocked => crate::ui::components::StatusTone::Blocked,
    }
}

/// The tone in the Blue view: red only for real failures (the file is gone or
/// unreadable, or something blocks it); a missing setup step is "needs
/// attention" (amber), not a failure.
pub(crate) fn blue_status_tone(
    status: ReadinessPresentationState,
) -> crate::ui::components::StatusTone {
    match status {
        ReadinessPresentationState::NeedsEmulator
        | ReadinessPresentationState::NeedsFirmware
        | ReadinessPresentationState::NeedsIdentityReview => {
            crate::ui::components::StatusTone::Warning
        }
        other => status_tone(other),
    }
}

pub(crate) fn status_label(status: ReadinessPresentationState) -> &'static str {
    match status {
        ReadinessPresentationState::Checking => "Checking",
        ReadinessPresentationState::Ready => "Ready to play",
        ReadinessPresentationState::ReadyWithWarnings => "Ready with warning",
        ReadinessPresentationState::NeedsEmulator => "Emulator needed",
        ReadinessPresentationState::NeedsFirmware => "Firmware needed",
        ReadinessPresentationState::NeedsIdentityReview => "Identity needs review",
        ReadinessPresentationState::SourceUnavailable => "Game file unavailable",
        ReadinessPresentationState::MediaUnreadable => "Game file unreadable",
        ReadinessPresentationState::Blocked => "Needs attention",
        ReadinessPresentationState::Stale => "Check again",
    }
}

pub(crate) fn action_label(action: ReadinessAction) -> &'static str {
    match action {
        ReadinessAction::Play => "Play",
        ReadinessAction::SetUpEmulator => "Set up emulator",
        ReadinessAction::CheckFirmware => "Check BIOS / Firmware",
        ReadinessAction::ReviewIdentity => "Review identity",
        ReadinessAction::CheckGamesFolder => "Check games folder",
        ReadinessAction::ReviewProblem => "Review problem",
        ReadinessAction::ChooseEmulator => "Choose emulator",
        ReadinessAction::RecheckReadiness => "Recheck readiness",
        ReadinessAction::ReviewDiscSet => "Review multi-disc set",
        ReadinessAction::OpenActivity => "Open Activity",
    }
}

pub(crate) fn show(ui: &mut egui::Ui, summary: &GameReadinessSummary) -> Option<ReadinessAction> {
    let mut action = None;
    crate::ui::components::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let tone = if crate::ui::theme::blue_profile(ui) {
                blue_status_tone(summary.status)
            } else {
                status_tone(summary.status)
            };
            crate::ui::components::status_badge(ui, status_label(summary.status), tone);
            ui.strong(&summary.headline);
        });
        // The action sits directly under the state so it stays on screen in a
        // short window; the explanation and evidence chips follow it.
        let action_count = 1 + usize::from(
            summary.primary_action.is_some()
                && summary.primary_action != Some(ReadinessAction::Play),
        );
        let available = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let slot = if available >= 320.0 && action_count == 2 {
            (available - gap) / 2.0
        } else {
            available.min(260.0)
        };
        ui.horizontal_wrapped(|ui| {
            if let Some(primary) = summary.primary_action {
                let fill = crate::ui::theme::primary_action_fill(ui);
                if ui
                    .add_sized(
                        [slot, 64.0],
                        egui::Button::new(egui::RichText::new(action_label(primary)).strong())
                            .fill(fill)
                            .wrap(),
                    )
                    .clicked()
                {
                    action = Some(primary);
                }
            }
            // Play is always on screen so the page never changes shape: when
            // it cannot run, it is disabled and the headline above says why.
            if summary.primary_action != Some(ReadinessAction::Play) {
                let blue = crate::ui::theme::blue_profile(ui);
                let mut play = egui::Button::new(egui::RichText::new("Play").strong()).wrap();
                if blue {
                    play = play.fill(egui::Color32::TRANSPARENT);
                }
                let response = ui
                    .add_enabled_ui(false, |ui| ui.add_sized([slot, 64.0], play))
                    .inner
                    .on_disabled_hover_text(summary.headline.as_str());
                // Blue: disabled is dashed; the headline above is the reason.
                if blue {
                    crate::ui::components::paint_dashed_outline(
                        ui.painter(),
                        response.rect,
                        crate::ui::theme::blue::BORDER_STRONG,
                    );
                }
            }
        });
        // A finding's own reason replaces the generic sentence, not repeats it.
        if summary.findings.is_empty() {
            ui.label(&summary.explanation);
        }
        if let Some(attempt) = &summary.attempt {
            if let Some(fix) = show_attempt(ui, attempt) {
                action = Some(fix);
            }
        }
        if let Some(fix) = show_findings(ui, summary) {
            action = Some(fix);
        }
        if summary.firmware == FirmwareSummary::Missing && summary.findings.is_empty() {
            ui.label("BIOS / firmware is the system software this emulator needs. Use your own legally obtained files in BIOS Setup; a matching filename alone does not prove the right file.");
        }
        if summary.status == ReadinessPresentationState::Stale {
            ui.label("The previous result is out of date. Recheck before trying to play; EmuWiz will not reuse an old approval.");
        }
        if summary.status == ReadinessPresentationState::NeedsEmulator {
            ui.label("If you already installed an emulator, choose its executable or profile in Emulator Setup. You do not need to download another copy just because it was not detected.");
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(emulator) = &summary.emulator {
                ui.label(format!("Using: {}", emulator.name));
            }
            ui.label(format!("Identity: {}", identity_label(summary.identity)));
            if summary.firmware != FirmwareSummary::Unknown {
                ui.label(format!("Firmware: {}", firmware_label(summary.firmware)));
            }
            ui.label(format!("Game file: {}", source_label(summary.source)));
        });
        for warning in summary.warnings.iter().take(2) {
            ui.colored_label(crate::ui::theme::warning(ui), warning);
        }
        ui.collapsing("Readiness details", |ui| {
            if let Some(emulator) = &summary.emulator {
                ui.label(format!("Profile: {}", emulator.profile));
            }
            for detail in &summary.details {
                ui.monospace(detail);
            }
            for finding in &summary.findings {
                for line in &finding.technical {
                    ui.monospace(line);
                }
            }
            ui.label(format!(
                "Identity evidence: {}",
                identity_label(summary.identity)
            ));
            ui.label(format!("Source state: {}", source_label(summary.source)));
            ui.label(format!(
                "Firmware state: {}",
                firmware_label(summary.firmware)
            ));
        });
    });
    action
}

/// Shown while the Library's check has not reached a current answer.
pub(crate) const PLAY_RUNS_THE_CHECK: &str =
    "Play runs the full launch check first. Nothing starts unless it passes.";

/// The Library's selected-game readiness, before it is drawn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompactReadiness {
    pub(crate) badge: &'static str,
    pub(crate) tone: ReadinessPresentationState,
    /// A check is running right now.
    pub(crate) spinner: bool,
    pub(crate) reason: String,
    /// Why Play is disabled, for its hover text.
    pub(crate) headline: String,
    /// Play is offered. True when ready and while no current answer exists,
    /// because the Launch preflight still decides; false only for a current
    /// refusal.
    pub(crate) play_enabled: bool,
    /// The step that fixes the first problem, if any.
    pub(crate) fix: Option<ReadinessAction>,
    /// No current answer yet: Play stays primary and the fix is secondary.
    pub(crate) pending: bool,
    pub(crate) more_findings: usize,
}

/// What the Library shows for a projection. `checksum_pending` means the
/// whole-file checksum (left to Game Details and Launch) has not run, so an
/// identity answer is not final yet.
pub(crate) fn compact(summary: &GameReadinessSummary, checksum_pending: bool) -> CompactReadiness {
    use ReadinessPresentationState as State;
    let reason = match summary.findings.first() {
        Some(finding) => format!("{} {}", finding.title, finding.why),
        None => summary.explanation.clone(),
    };
    let fix = summary
        .primary_action
        .filter(|action| *action != ReadinessAction::Play);
    let more_findings = summary.findings.len().saturating_sub(1);
    match summary.status {
        State::Checking => CompactReadiness {
            badge: status_label(State::Checking),
            tone: State::Checking,
            spinner: true,
            reason: PLAY_RUNS_THE_CHECK.into(),
            headline: summary.headline.clone(),
            play_enabled: true,
            fix: None,
            pending: true,
            more_findings: 0,
        },
        State::Stale => CompactReadiness {
            badge: status_label(State::Stale),
            tone: State::Stale,
            spinner: false,
            reason: format!("{reason} {PLAY_RUNS_THE_CHECK}"),
            headline: summary.headline.clone(),
            play_enabled: true,
            fix,
            pending: true,
            more_findings,
        },
        State::NeedsIdentityReview if checksum_pending => CompactReadiness {
            badge: "Not fully checked",
            tone: State::Checking,
            spinner: false,
            headline: summary.headline.clone(),
            reason: format!(
                "The full identity check reads the whole file, so it runs when you press Play. \
                 {PLAY_RUNS_THE_CHECK}"
            ),
            play_enabled: true,
            fix,
            pending: true,
            more_findings: 0,
        },
        status => {
            let ready = summary.primary_action == Some(ReadinessAction::Play);
            CompactReadiness {
                badge: status_label(status),
                tone: status,
                spinner: false,
                reason,
                headline: summary.headline.clone(),
                play_enabled: ready,
                fix,
                pending: false,
                more_findings,
            }
        }
    }
}

/// What was clicked on the compact card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompactChoice {
    Action(ReadinessAction),
    /// The caller's own trailing button (the Library's Game details).
    Trailing,
}

/// Compact readiness for the Library's selected-game panel: the status, the
/// real reason, and one primary button. Play is primary when the game is
/// ready or not yet checked (it opens the Launch preflight, which decides);
/// for a current refusal the fix is primary and Play stays on screen,
/// disabled, with the reason. `trailing` is one more secondary button, put
/// on the same row when it fits. Returns what was clicked; never launches.
pub(crate) fn show_compact(
    ui: &mut egui::Ui,
    view: &CompactReadiness,
    width: f32,
    reason_rows: usize,
    trailing: Option<&str>,
) -> Option<CompactChoice> {
    let blue = crate::ui::theme::blue_profile(ui);
    let mut choice = None;
    ui.horizontal(|ui| {
        let tone = if blue {
            blue_status_tone(view.tone)
        } else {
            status_tone(view.tone)
        };
        crate::ui::components::status_badge(ui, view.badge, tone);
        if view.spinner {
            ui.spinner();
        }
    });
    let mut reason = egui::text::LayoutJob::single_section(
        view.reason.clone(),
        egui::TextFormat {
            font_id: egui::FontId::proportional(crate::ui::theme::METADATA_SIZE),
            color: crate::ui::theme::SECONDARY_TEXT,
            ..Default::default()
        },
    );
    reason.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: reason_rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.add(egui::Label::new(reason).wrap().selectable(false))
        .on_hover_text(view.reason.as_str());
    let gap = ui.spacing().item_spacing.x;
    let fill = crate::ui::theme::primary_action_fill(ui);
    let fix_is_primary = view.fix.is_some() && !view.pending;
    let count = 1 + usize::from(view.fix.is_some());
    let inline_trailing =
        trailing.is_some() && width >= (count + 1) as f32 * 144.0 + gap * count as f32;
    let slots = count + usize::from(inline_trailing);
    let stacked = width < count as f32 * 144.0 + gap * (count - 1) as f32;
    let slot = if stacked {
        width
    } else {
        (width - gap * (slots - 1) as f32) / slots as f32
    };
    // Equal height also leaves room for a two-line action label.
    let action_size = [slot, 64.0];
    let fix_button = |ui: &mut egui::Ui, primary: bool| {
        let fix = view.fix?;
        let mut button = egui::Button::new(egui::RichText::new(action_label(fix)).strong()).wrap();
        if primary {
            button = button.fill(fill);
        }
        ui.add_sized(action_size, button)
            .on_hover_text(action_label(fix))
            .clicked()
            .then_some(CompactChoice::Action(fix))
    };
    let trailing_button = |ui: &mut egui::Ui, label: &str, width: f32| {
        ui.add_sized([width, 64.0], egui::Button::new(label).wrap())
            .clicked()
            .then_some(CompactChoice::Trailing)
    };
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        if stacked {
            egui::Layout::top_down(egui::Align::Min)
        } else {
            egui::Layout::left_to_right(egui::Align::Min)
        },
        |ui| {
            if fix_is_primary && let Some(clicked) = fix_button(ui, true) {
                choice = Some(clicked);
            }
            let mut play = egui::Button::new(egui::RichText::new("Play").strong());
            if view.play_enabled && !fix_is_primary {
                play = play.fill(fill);
            } else if blue {
                play = play.fill(egui::Color32::TRANSPARENT);
            }
            let response = ui
                .add_enabled_ui(view.play_enabled, |ui| ui.add_sized(action_size, play))
                .inner;
            if view.play_enabled {
                if response.clicked() {
                    choice = Some(CompactChoice::Action(ReadinessAction::Play));
                }
            } else {
                let rect = response.rect;
                response.on_disabled_hover_text(view.headline.as_str());
                if blue {
                    crate::ui::components::paint_dashed_outline(
                        ui.painter(),
                        rect,
                        crate::ui::theme::blue::BORDER_STRONG,
                    );
                }
            }
            if !fix_is_primary && let Some(clicked) = fix_button(ui, false) {
                choice = Some(clicked);
            }
            if inline_trailing
                && let Some(label) = trailing
                && let Some(clicked) = trailing_button(ui, label, slot)
            {
                choice = Some(clicked);
            }
        },
    );
    if !inline_trailing
        && let Some(label) = trailing
        && let Some(clicked) = trailing_button(ui, label, width)
    {
        choice = Some(clicked);
    }
    if view.more_findings > 0 {
        ui.label(
            egui::RichText::new(format!(
                "{} more to fix. Game details lists them all.",
                view.more_findings
            ))
            .size(crate::ui::theme::METADATA_SIZE)
            .color(crate::ui::theme::SECONDARY_TEXT),
        );
    }
    choice
}

/// The Library's state before its check has started (the selection is still
/// settling): nothing is known yet, so it reads as Checking.
pub(crate) fn not_started_summary() -> GameReadinessSummary {
    checking_summary()
}

/// When more than one thing blocks the game, list them all now so nobody has
/// to fix one just to discover the next. The first one's button is already the
/// card's primary action, so it is not repeated.
fn show_findings(ui: &mut egui::Ui, summary: &GameReadinessSummary) -> Option<ReadinessAction> {
    if summary.findings.is_empty() {
        return None;
    }
    let mut action = None;
    if summary.findings.len() > 1 {
        ui.strong(format!("{} things need attention", summary.findings.len()));
    }
    for (index, finding) in summary.findings.iter().enumerate() {
        if summary.findings.len() > 1 {
            ui.label(egui::RichText::new(format!("{}. {}", index + 1, finding.title)).strong());
        } else {
            ui.label(egui::RichText::new(finding.title).strong());
        }
        ui.label(finding.why);
        if index > 0
            && let Some(fix) = finding.fix
            && ui.button(action_label(fix)).clicked()
        {
            action = Some(fix);
        }
    }
    ui.label(
        "Your game was not changed. Checking launch readiness does not change your game files.",
    );
    action
}

fn show_attempt(ui: &mut egui::Ui, attempt: &Attempt) -> Option<ReadinessAction> {
    ui.separator();
    ui.label(egui::RichText::new(attempt.title()).strong());
    ui.label(attempt.why());
    if let Some(hint) = attempt.hint() {
        ui.label(hint);
    }
    ui.label("EmuWiz did not change your game files.");
    let fix = attempt.fix();
    let clicked = ui.button(action_label(fix)).clicked();
    crate::ui::components::technical_details(ui, "launch_attempt_details", |ui| {
        ui.monospace(attempt.technical());
    });
    ui.separator();
    clicked.then_some(fix)
}

/// A game whose saved exact DAT match already verified it must not be told its
/// identity is "not confirmed". Launch is deliberately unchanged: it still
/// requires the file's own identity evidence (and re-reads the file when it
/// starts), so say exactly that instead of contradicting the verified status.
/// A Ready projection whose RetroArch launch is withheld for an installation
/// problem (an unusable saved choice, or a program that changed since the
/// last check) is not Ready: say why and route to Emulator Setup. Game-level
/// refusals are left to the projection.
pub(crate) fn note_retroarch_installation_refusal(
    summary: &mut GameReadinessSummary,
    refusal: &crate::launch_readiness_page::RetroArchRequestRefusal,
) {
    if !refusal.installation_problem()
        || !matches!(
            summary.status,
            ReadinessPresentationState::Ready | ReadinessPresentationState::ReadyWithWarnings
        )
    {
        return;
    }
    summary.status = ReadinessPresentationState::NeedsEmulator;
    summary.headline = "RetroArch can't be used as chosen".into();
    summary.explanation = refusal.message();
    summary.primary_action = Some(ReadinessAction::SetUpEmulator);
}

pub(crate) fn note_catalogue_verified(summary: &mut GameReadinessSummary) {
    if summary.status == ReadinessPresentationState::NeedsIdentityReview
        && summary.identity == IdentitySummary::NeedsReview
    {
        summary.identity = IdentitySummary::CatalogueVerified;
        summary.headline = "Verified. Launch also needs the file's own identity".into();
        summary.explanation = "This game matched trusted identification data exactly. Before it plays, EmuWiz also reads the file's own identity data, and that could not be confirmed yet. The verification itself is fine; Review identity shows what is known.".into();
    }
}

fn identity_label(identity: IdentitySummary) -> &'static str {
    match identity {
        IdentitySummary::StrongLocal => "Strong local identity",
        IdentitySummary::LaunchableWithWarning => "Launchable with warning",
        IdentitySummary::NeedsReview => "Needs review",
        IdentitySummary::CatalogueVerified => "Verified · file identity still needed to launch",
        IdentitySummary::Mismatch => "Mismatch / blocked",
    }
}

fn firmware_label(firmware: FirmwareSummary) -> &'static str {
    match firmware {
        FirmwareSummary::NotRequired => "Firmware not required",
        FirmwareSummary::Ready => "Firmware ready",
        FirmwareSummary::Missing => "Firmware missing",
        FirmwareSummary::Unknown => "Firmware state unknown",
    }
}

fn source_label(source: SourceSummary) -> &'static str {
    match source {
        SourceSummary::Available => "Available",
        SourceSummary::Unavailable => "Unavailable",
        SourceSummary::Unreadable => "Unreadable",
        SourceSummary::RecheckNeeded => "Recheck needed",
    }
}

#[cfg(test)]
mod diagnosis_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::{
        LaunchBlocker, LaunchContentRef, LaunchPlanSummary, LaunchWarning,
    };
    use std::path::PathBuf;

    fn candidate(
        readiness: LaunchReadiness,
        firmware: FirmwareReadiness,
        blockers: Vec<LaunchBlocker>,
        warnings: Vec<LaunchWarning>,
        preference: CandidatePreference,
        path: Option<&str>,
    ) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "duckstation",
                profile_id: "default".into(),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: None,
                container: None,
                resolved_path: path.map(PathBuf::from),
                requires_mount: false,
                provenance: "synthetic fixture".into(),
            },
            firmware,
            blockers,
            warnings,
            readiness,
            preference,
        }
    }

    fn plan(candidates: Vec<LaunchCandidate>) -> LaunchReadinessInput {
        LaunchReadinessInput::Plan {
            plan: LaunchPlan {
                platform_id: Some("PSX".into()),
                game_key: Some("SLUS-00000".into()),
                summary: LaunchPlanSummary {
                    candidates: candidates.len(),
                    ready: candidates
                        .iter()
                        .filter(|candidate| candidate.readiness == LaunchReadiness::Ready)
                        .count(),
                    ready_with_warnings: candidates
                        .iter()
                        .filter(|candidate| {
                            candidate.readiness == LaunchReadiness::ReadyWithWarnings
                        })
                        .count(),
                    blocked: candidates
                        .iter()
                        .filter(|candidate| candidate.readiness == LaunchReadiness::Blocked)
                        .count(),
                },
                candidates,
                media_topology: None,
            },
            retroarch: None,
            retroarch_scanned: true,
            standalone_scans_complete: true,
            dolphin: None,
            pcsx2: None,
            duckstation: None,
            ppsspp: None,
            rpcs3: None,
            xemu: None,
            xenia: None,
            melonds: None,
            cemu: None,
            flycast: None,
            xroar: None,
            vice: None,
            amiga_whdload: None,
            #[cfg(target_os = "linux")]
            shadps4: None,
        }
    }

    #[test]
    fn fully_ready_projects_to_play() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Ready,
                FirmwareReadiness::NotRequired,
                vec![],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        assert_eq!(summary.status, ReadinessPresentationState::Ready);
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
    }

    fn arcade_layout_blocker() -> LaunchBlocker {
        LaunchBlocker::new(
            LaunchBlockerKind::RetroArchArcadeSetNeedsArchive,
            "RetroArch needs this arcade set as a ZIP or other supported archive. Your game is stored as an extracted MAME set, so use MAME for this copy.",
        )
    }

    #[test]
    fn extracted_arcade_set_for_fbneo_is_blocked_without_a_missing_file_claim() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::NotRequired,
                vec![arcade_layout_blocker()],
                vec![],
                CandidatePreference::Undetermined,
                Some("/mnt/arcade/19xx"),
            )]),
            ReadinessFreshness::Current,
        );
        assert_eq!(summary.status, ReadinessPresentationState::Blocked);
        assert_eq!(summary.headline, "RetroArch cannot use this arcade layout");
        assert!(summary.explanation.contains("extracted MAME set"));
        assert!(summary.explanation.contains("use MAME"));
        let text = format!("{} {}", summary.headline, summary.explanation).to_lowercase();
        assert!(!text.contains("no longer available") && !text.contains("moved"));
        assert_ne!(
            summary.status,
            ReadinessPresentationState::SourceUnavailable
        );
    }

    #[test]
    fn ready_mame_stays_playable_beside_a_blocked_fbneo_candidate() {
        let mut fbneo = candidate(
            LaunchReadiness::Blocked,
            FirmwareReadiness::NotRequired,
            vec![arcade_layout_blocker()],
            vec![],
            CandidatePreference::Undetermined,
            Some("/mnt/arcade/19xx"),
        );
        fbneo.target = LaunchTarget::RetroArchCore {
            profile: archivefs_core::emulator_environment::retroarch::ProfileRef {
                profile_kind: archivefs_core::emulator_environment::retroarch::ProfileKind::Native,
                scope: archivefs_core::emulator_environment::retroarch::ProfileScope::User,
            },
            core_stem: "fbneo".into(),
            platform_id: "Arcade",
        };
        let mame = candidate(
            LaunchReadiness::Ready,
            FirmwareReadiness::NotRequired,
            vec![],
            vec![],
            CandidatePreference::Undetermined,
            Some("/mnt/arcade/19xx"),
        );
        let summary = project(&plan(vec![mame, fbneo]), ReadinessFreshness::Current);
        assert_eq!(summary.status, ReadinessPresentationState::Ready);
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
    }

    #[test]
    fn ready_with_warning_does_not_become_blocked() {
        let warning = LaunchWarning {
            kind: LaunchWarningKind::OptionalFirmwareMissing,
            detail: "title only".into(),
        };
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::ReadyWithWarnings,
                FirmwareReadiness::NotRequired,
                vec![],
                vec![warning],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        assert_eq!(
            summary.status,
            ReadinessPresentationState::ReadyWithWarnings
        );
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
        assert_eq!(summary.identity, IdentitySummary::LaunchableWithWarning);
    }

    #[test]
    fn missing_firmware_projects_to_firmware_action() {
        let blocker = LaunchBlocker {
            kind: LaunchBlockerKind::RequiredFirmwareMissing,
            detail: "missing BIOS".into(),
        };
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::Missing,
                vec![blocker],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        assert_eq!(summary.status, ReadinessPresentationState::NeedsFirmware);
        assert_eq!(summary.primary_action, Some(ReadinessAction::CheckFirmware));
    }

    #[test]
    fn source_and_media_failures_precede_emulator_details() {
        let blocker = LaunchBlocker {
            kind: LaunchBlockerKind::ContentNotResolved,
            detail: "missing source".into(),
        };
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::NotRequired,
                vec![blocker],
                vec![],
                CandidatePreference::SoleEligible,
                None,
            )]),
            ReadinessFreshness::Current,
        );
        assert_eq!(
            summary.status,
            ReadinessPresentationState::SourceUnavailable
        );
        assert_eq!(
            summary.primary_action,
            Some(ReadinessAction::CheckGamesFolder)
        );
    }

    #[test]
    fn unresolved_disc_files_offer_the_existing_in_place_recheck() {
        let summary = project(
            &LaunchReadinessInput::DiscFilesUnresolved(
                "This disc cannot be launched: missing Track 02 audio.bin".into(),
            ),
            ReadinessFreshness::Current,
        );
        assert!(summary.explanation.contains("Track 02 audio.bin"));
        assert_eq!(
            summary.primary_action,
            Some(ReadinessAction::RecheckReadiness)
        );
        assert_eq!(
            summary.status,
            ReadinessPresentationState::NeedsIdentityReview
        );
    }

    #[test]
    fn a_refused_cue_bin_disc_names_the_file_instead_of_a_generic_missing_source() {
        let blocker = LaunchBlocker {
            kind: LaunchBlockerKind::ContentNotResolved,
            detail: format!(
                "{}a file the CUE sheet needs is missing: /games/Track 02 audio.bin. Recheck its sheet and companion tracks",
                archivefs_core::launch::evidence_bridge::CUE_BIN_REFUSED_PREFIX
            ),
        };
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::NotRequired,
                vec![blocker],
                vec![],
                CandidatePreference::SoleEligible,
                None,
            )]),
            ReadinessFreshness::Current,
        );
        assert!(summary.explanation.contains("Track 02 audio.bin"));
        assert_eq!(
            summary.primary_action,
            Some(ReadinessAction::RecheckReadiness)
        );
        assert_ne!(summary.headline, "The game file is unavailable");
        assert!(
            summary
                .findings
                .iter()
                .all(|finding| { finding.cause != diagnosis::Cause::GameFileUnavailable })
        );
    }

    #[test]
    fn multiple_safe_candidates_require_choice() {
        let summary = project(
            &plan(vec![
                candidate(
                    LaunchReadiness::Ready,
                    FirmwareReadiness::NotRequired,
                    vec![],
                    vec![],
                    CandidatePreference::Undetermined,
                    Some("/game.iso"),
                ),
                candidate(
                    LaunchReadiness::Ready,
                    FirmwareReadiness::NotRequired,
                    vec![],
                    vec![],
                    CandidatePreference::Undetermined,
                    Some("/game.iso"),
                ),
            ]),
            ReadinessFreshness::Current,
        );
        assert_eq!(
            summary.primary_action,
            Some(ReadinessAction::ChooseEmulator)
        );
    }

    /// Job #22: an installation refusal withdraws Ready and routes to Setup
    /// with its own reason; game-format refusals never do.
    #[test]
    fn installation_refusal_withdraws_ready_but_game_refusals_do_not() {
        use crate::launch_readiness_page::RetroArchRequestRefusal as R;
        use crate::retroarch_installation_setup::SelectionIssue;
        use archivefs_core::emulator_environment::retroarch::installation_selection::SelectedExecutableProblem as P;
        let ready = || {
            project(
                &plan(vec![candidate(
                    LaunchReadiness::Ready,
                    FirmwareReadiness::NotRequired,
                    vec![],
                    vec![],
                    CandidatePreference::SoleEligible,
                    Some("/game.iso"),
                )]),
                ReadinessFreshness::Current,
            )
        };
        let selection = |problem| {
            R::Installation(SelectionIssue::Unusable {
                path: "/chosen/retroarch".into(),
                problem,
            })
        };
        let mut messages = Vec::new();
        for refusal in [
            selection(P::Missing),
            selection(P::NotExecutable),
            selection(P::FlatpakLauncher),
            selection(P::UnsupportedProgram),
            selection(P::NotDiscovered),
            R::Installation(SelectionIssue::Unreadable("not a regular file".into())),
            R::ProgramChanged("/usr/games/retroarch".into()),
        ] {
            let mut summary = ready();
            assert_eq!(summary.status, ReadinessPresentationState::Ready);
            note_retroarch_installation_refusal(&mut summary, &refusal);
            assert_eq!(
                summary.status,
                ReadinessPresentationState::NeedsEmulator,
                "{refusal:?}"
            );
            assert_eq!(summary.primary_action, Some(ReadinessAction::SetUpEmulator));
            assert_eq!(summary.explanation, refusal.message());
            messages.push(refusal.message());
        }
        assert!(messages[0].contains("no longer there"));
        assert!(messages[2].contains("Flatpak"));
        let mut distinct = messages.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), messages.len(), "each reason is told apart");
        for refusal in [
            R::NotPlainFile,
            R::MultiDisc,
            R::NeedsMount,
            R::NotRetroArch,
        ] {
            assert!(!refusal.installation_problem());
            let mut summary = ready();
            note_retroarch_installation_refusal(&mut summary, &refusal);
            assert_eq!(
                summary.status,
                ReadinessPresentationState::Ready,
                "{refusal:?}"
            );
            assert!(!refusal.message().contains("RetroArch program you chose"));
        }
    }

    #[test]
    fn stale_freshness_overrides_ready_projection() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Ready,
                FirmwareReadiness::NotRequired,
                vec![],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Stale,
        );
        assert_eq!(summary.status, ReadinessPresentationState::Stale);
        assert_eq!(
            summary.primary_action,
            Some(ReadinessAction::RecheckReadiness)
        );
    }

    #[test]
    fn missing_emulator_projects_to_setup_action() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::NotRequired,
                vec![LaunchBlocker {
                    kind: LaunchBlockerKind::NoInstallationCandidate,
                    detail: "no profile".into(),
                }],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        assert_eq!(summary.status, ReadinessPresentationState::NeedsEmulator);
        assert_eq!(summary.primary_action, Some(ReadinessAction::SetUpEmulator));
    }

    #[test]
    fn a_dos_game_routes_play_to_the_dosbox_review_not_emulator_setup() {
        let mut input = plan(vec![]);
        let LaunchReadinessInput::Plan {
            plan: dos_plan,
            retroarch_scanned,
            ..
        } = &mut input
        else {
            unreachable!()
        };
        dos_plan.platform_id = Some("DOS".into());
        // RetroArch has no DOS lane, so an unscanned RetroArch must not hold
        // the DOS game in Checking.
        *retroarch_scanned = false;
        let summary = project(&input, ReadinessFreshness::Current);
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
        assert_eq!(summary.status, ReadinessPresentationState::Blocked);
        assert_eq!(summary.explanation, DOSBOX_REVIEW_EXPLANATION);
        let view = compact(&summary, false);
        assert!(view.play_enabled);
        assert_eq!(view.fix, None);
        assert_eq!(
            route_for_play(),
            Some(crate::gui_v2::routes::Route::Task {
                section: crate::gui_v2::routes::Section::Launch,
                game: 7,
            })
        );
    }

    fn route_for_play() -> Option<crate::gui_v2::routes::Route> {
        crate::gui_v2::native_workflows::route_for_action(ReadinessAction::Play, 7)
    }

    #[test]
    fn invalid_profile_is_an_emulator_setup_problem() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::NotRequired,
                vec![LaunchBlocker {
                    kind: LaunchBlockerKind::ProfileIneligible,
                    detail: "profile is not eligible".into(),
                }],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        assert_eq!(summary.primary_action, Some(ReadinessAction::SetUpEmulator));
    }

    #[test]
    fn identity_inputs_are_never_called_verified_by_the_summary() {
        let unknown = project(
            &LaunchReadinessInput::IdentityUnknown,
            ReadinessFreshness::Current,
        );
        assert_eq!(unknown.identity, IdentitySummary::NeedsReview);
        let conflicting = project(
            &LaunchReadinessInput::IdentityConflicting,
            ReadinessFreshness::Current,
        );
        assert_eq!(conflicting.identity, IdentitySummary::Mismatch);
    }

    #[test]
    fn optional_warning_preserves_play_action() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Ready,
                FirmwareReadiness::PresentUnverified,
                vec![],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        assert_eq!(
            summary.status,
            ReadinessPresentationState::ReadyWithWarnings
        );
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
    }

    fn text_rects(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn gather(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| gather(shape, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        output
            .shapes
            .iter()
            .for_each(|shape| gather(&shape.shape, &mut out));
        out
    }

    /// Lays the card out once, then clicks the centre of the button labelled
    /// `label` and reports what the card asked the page to do.
    fn click_card_button(
        summary: &GameReadinessSummary,
        label: &str,
    ) -> (Vec<String>, Option<ReadinessAction>) {
        click_rendered_button(summary, label, show)
    }

    /// The same, for the Library's compact card at a 1024x600-sized column.
    fn click_compact_button(
        summary: &GameReadinessSummary,
        label: &str,
    ) -> (Vec<String>, Option<ReadinessAction>) {
        click_rendered_button(summary, label, |ui, summary| {
            match show_compact(ui, &compact(summary, false), 240.0, 2, None) {
                Some(CompactChoice::Action(action)) => Some(action),
                _ => None,
            }
        })
    }

    fn click_rendered_button(
        summary: &GameReadinessSummary,
        label: &str,
        render: impl Fn(&mut egui::Ui, &GameReadinessSummary) -> Option<ReadinessAction>,
    ) -> (Vec<String>, Option<ReadinessAction>) {
        let context = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let run = |events: Vec<egui::Event>| {
            let mut action = None;
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        action = render(ui, summary);
                    });
                },
            );
            (output, action)
        };
        let (layout, _) = run(Vec::new());
        let rects = text_rects(&layout);
        let labels = rects.iter().map(|(text, _)| text.clone()).collect();
        let Some((_, rect)) = rects.iter().find(|(text, _)| text == label) else {
            return (labels, None);
        };
        let at = rect.center();
        let button = egui::PointerButton::Primary;
        let modifiers = egui::Modifiers::NONE;
        let (_, action) = run(vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button,
                pressed: true,
                modifiers,
            },
            egui::Event::PointerButton {
                pos: at,
                button,
                pressed: false,
                modifiers,
            },
        ]);
        (labels, action)
    }

    #[test]
    fn ready_card_offers_exactly_one_enabled_play() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Ready,
                FirmwareReadiness::NotRequired,
                vec![],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        let (labels, action) = click_card_button(&summary, "Play");
        assert_eq!(labels.iter().filter(|label| *label == "Play").count(), 1);
        assert_eq!(action, Some(ReadinessAction::Play));
    }

    #[test]
    fn blocked_card_keeps_play_visible_but_inert_and_routes_the_fix() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::NotRequired,
                vec![LaunchBlocker {
                    kind: LaunchBlockerKind::NoInstallationCandidate,
                    detail: "no profile".into(),
                }],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        let (labels, play) = click_card_button(&summary, "Play");
        assert!(
            labels.iter().any(|label| label == "Play"),
            "Play stays visible"
        );
        assert_eq!(play, None, "a blocked game must not launch");
        let (_, fix) = click_card_button(&summary, "Set up emulator");
        assert_eq!(fix, Some(ReadinessAction::SetUpEmulator));
    }

    #[test]
    fn checking_card_shows_inert_play_and_no_other_action() {
        let summary = project(
            &LaunchReadinessInput::EvidenceNotLoaded,
            ReadinessFreshness::Current,
        );
        let (labels, play) = click_card_button(&summary, "Play");
        assert!(labels.iter().any(|label| label == "Play"));
        assert_eq!(play, None);
    }

    #[test]
    fn compact_ready_card_offers_one_enabled_play() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Ready,
                FirmwareReadiness::NotRequired,
                vec![],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        let (labels, action) = click_compact_button(&summary, "Play");
        assert_eq!(labels.iter().filter(|label| *label == "Play").count(), 1);
        assert!(labels.iter().any(|label| label.ends_with("Ready to play")));
        assert_eq!(action, Some(ReadinessAction::Play));
    }

    #[test]
    fn compact_blocked_card_names_the_reason_and_routes_the_fix_not_play() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::Missing,
                vec![LaunchBlocker {
                    kind: LaunchBlockerKind::RequiredFirmwareMissing,
                    detail: "missing BIOS".into(),
                }],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        let (labels, play) = click_compact_button(&summary, "Play");
        assert!(
            labels.iter().any(|label| label == "Play"),
            "Play stays visible"
        );
        assert_eq!(play, None, "a blocked game must not launch");
        assert!(
            labels
                .iter()
                .any(|label| label.ends_with("Firmware needed"))
        );
        let finding = summary.findings.first().expect("a firmware finding");
        assert!(
            labels.iter().any(|label| label.starts_with(finding.title)),
            "the real reason is shown: {labels:?}"
        );
        let (_, fix) = click_compact_button(&summary, "Check BIOS / Firmware");
        assert_eq!(fix, Some(ReadinessAction::CheckFirmware));
    }

    #[test]
    fn compact_checking_card_keeps_play_open_to_the_launch_preflight() {
        let summary = project(
            &LaunchReadinessInput::EvidenceNotLoaded,
            ReadinessFreshness::Current,
        );
        let (labels, play) = click_compact_button(&summary, "Play");
        assert!(labels.iter().any(|label| label.ends_with("Checking")));
        assert!(!labels.iter().any(|label| label.ends_with("Ready to play")));
        assert!(labels.iter().any(|label| label == PLAY_RUNS_THE_CHECK));
        // A pending check never disables Play: Play opens Launch, whose
        // preflight still decides.
        assert_eq!(play, Some(ReadinessAction::Play));
    }

    #[test]
    fn a_stale_result_keeps_play_and_offers_recheck_as_secondary() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Ready,
                FirmwareReadiness::NotRequired,
                vec![],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Stale,
        );
        let view = compact(&summary, false);
        assert!(view.pending);
        assert!(view.play_enabled);
        assert_ne!(view.badge, status_label(ReadinessPresentationState::Ready));
    }

    #[test]
    fn identity_waiting_on_the_checksum_is_not_shown_as_a_refusal() {
        let summary = project(
            &LaunchReadinessInput::IdentityUnknown,
            ReadinessFreshness::Current,
        );
        assert_eq!(
            summary.status,
            ReadinessPresentationState::NeedsIdentityReview
        );
        let deferred = compact(&summary, true);
        assert_eq!(deferred.badge, "Not fully checked");
        assert!(deferred.play_enabled && deferred.pending);
        assert_eq!(deferred.fix, Some(ReadinessAction::ReviewIdentity));
        // Once the checksum has run, the same answer is a current refusal.
        let current = compact(&summary, false);
        assert!(!current.play_enabled && !current.pending);
        assert_eq!(current.fix, Some(ReadinessAction::ReviewIdentity));
    }

    #[test]
    fn compact_card_points_to_game_details_for_further_problems() {
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::Missing,
                vec![
                    LaunchBlocker {
                        kind: LaunchBlockerKind::RequiredFirmwareMissing,
                        detail: "missing BIOS".into(),
                    },
                    LaunchBlocker {
                        kind: LaunchBlockerKind::NoInstallationCandidate,
                        detail: "no profile".into(),
                    },
                ],
                vec![],
                CandidatePreference::SoleEligible,
                Some("/game.iso"),
            )]),
            ReadinessFreshness::Current,
        );
        let (labels, _) = click_compact_button(&summary, "Play");
        if summary.findings.len() > 1 {
            assert!(
                labels
                    .iter()
                    .any(|label| label.ends_with("Game details lists them all.")),
                "{labels:?}"
            );
        } else {
            assert!(!labels.iter().any(|label| label.contains("more to fix")));
        }
    }

    #[test]
    fn unknown_firmware_is_not_shown_as_a_status() {
        let summary = project(
            &LaunchReadinessInput::EvidenceNotLoaded,
            ReadinessFreshness::Current,
        );
        let (labels, _) = click_card_button(&summary, "Play");
        assert!(
            !labels
                .iter()
                .any(|label| label.contains("Firmware state unknown"))
        );
    }

    fn duckstation_input(
        authority: crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority,
    ) -> LaunchReadinessInput {
        use archivefs_core::patch_manager::{
            DuckStationProfileDiscovery, DuckStationProfileDiscoveryRoots,
        };
        let mut input = plan(vec![candidate(
            LaunchReadiness::Ready,
            FirmwareReadiness::NotRequired,
            vec![],
            vec![],
            CandidatePreference::SoleEligible,
            Some("/game.iso"),
        )]);
        if let LaunchReadinessInput::Plan { duckstation, .. } = &mut input {
            *duckstation = Some(crate::launch_readiness_page::DuckStationLaunchContext {
                multidisc: None,
                discovery: DuckStationProfileDiscovery {
                    profiles: Vec::new(),
                    complete: true,
                },
                roots: DuckStationProfileDiscoveryRoots {
                    home: PathBuf::from("/h"),
                    xdg_config_home: PathBuf::from("/h/.config"),
                    xdg_data_home: PathBuf::from("/h/.local/share"),
                    xdg_config_home_explicit: false,
                    explicit_configuration_roots: Vec::new(),
                    portable_configuration_roots: Vec::new(),
                    explicit_executables: Vec::new(),
                    known_version_outputs: Default::default(),
                    appimage_directory: None,
                    path_override: Some(Vec::new()),
                },
                firmware_evidence: Vec::new(),
                verified_ps1_serial: None,
                authority,
            });
        }
        input
    }

    #[test]
    fn a_preferred_fallback_is_visible_in_launch_readiness_and_play_stays_available() {
        use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority as A;
        let summary = project(
            &duckstation_input(A::Authorised {
                executable: PathBuf::from("/apps/DuckStation.AppImage"),
                profile_root: PathBuf::from("/apps"),
                layout: archivefs_core::emulator_profile_resolver::ProfileLayout::Portable,
                mode: "PREFERRED",
                fallback: true,
                why: vec!["It is the only installation with a usable configuration.".into()],
                notes: Vec::new(),
            }),
            ReadinessFreshness::Current,
        );
        let text = summary.warnings.join("\n");
        assert!(
            text.contains("preferred DuckStation is unavailable"),
            "{text}"
        );
        assert!(text.contains("/apps/DuckStation.AppImage"), "{text}");
        assert!(
            text.contains("only installation with a usable configuration"),
            "{text}"
        );
    }

    #[test]
    fn a_preferred_fallback_survives_the_two_warning_display_limit() {
        use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority as A;
        let mut input = duckstation_input(A::Authorised {
            executable: PathBuf::from("/apps/DuckStation.AppImage"),
            profile_root: PathBuf::from("/apps"),
            layout: archivefs_core::emulator_profile_resolver::ProfileLayout::Portable,
            mode: "PREFERRED",
            fallback: true,
            why: vec!["It is the only installation with a usable configuration.".into()],
            notes: Vec::new(),
        });
        if let LaunchReadinessInput::Plan { plan, .. } = &mut input {
            let candidate = plan.candidates.first_mut().expect("candidate");
            candidate.warnings = vec![
                LaunchWarning::new(
                    LaunchWarningKind::OptionalFirmwareMissing,
                    "First general warning.",
                ),
                LaunchWarning::new(
                    LaunchWarningKind::OptionalFirmwareMissing,
                    "Second general warning.",
                ),
                LaunchWarning::new(
                    LaunchWarningKind::OptionalFirmwareMissing,
                    "Third general warning.",
                ),
            ];
        }
        let summary = project(&input, ReadinessFreshness::Current);
        // Exactly what the summary draws: its first two warnings.
        let shown: Vec<&String> = summary.warnings.iter().take(2).collect();
        assert!(
            shown[0].contains("preferred DuckStation is unavailable"),
            "{shown:?}"
        );
        let all = summary.warnings.join("\n");
        for general in ["First general", "Second general", "Third general"] {
            assert!(all.contains(general), "general warning kept: {all}");
        }
    }

    #[test]
    fn a_resolver_refusal_is_visible_in_launch_readiness() {
        use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority as A;
        let summary = project(
            &duckstation_input(A::Refused {
                reason: "Your forced DuckStation choice is unavailable.".into(),
            }),
            ReadinessFreshness::Current,
        );
        let text = summary.warnings.join("\n");
        assert!(text.contains("blocked by the shared resolver"), "{text}");
        assert!(
            text.contains("will not use any other installation"),
            "{text}"
        );
    }

    #[test]
    fn an_ordinary_resolver_selection_adds_no_launch_warning() {
        use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority as A;
        let summary = project(
            &duckstation_input(A::Authorised {
                executable: PathBuf::from("/apps/DuckStation.AppImage"),
                profile_root: PathBuf::from("/apps"),
                layout: archivefs_core::emulator_profile_resolver::ProfileLayout::Portable,
                mode: "AUTO",
                fallback: false,
                why: Vec::new(),
                notes: Vec::new(),
            }),
            ReadinessFreshness::Current,
        );
        assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
    }
}

#[cfg(test)]
mod dosbox_approval_tests {
    use super::*;
    #[test]
    fn local_dos_review_offers_play_without_claiming_verified_identity() {
        let summary = project(
            &LaunchReadinessInput::DosBoxReview,
            ReadinessFreshness::Current,
        );
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
        assert_eq!(summary.identity, IdentitySummary::NeedsReview);
        assert!(
            summary
                .explanation
                .contains("does not verify a game release")
        );
        assert!(compact(&summary, false).play_enabled);
    }
}
